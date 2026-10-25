//! 原接收负责人失败清理后的完整事实进入旧批次通知，不把最后一项或FIN当作业务确认。
use super::super::profile_probe::LegacyPair;
use super::query_provider_probe::SettledSnapshot;
use super::{progress_budget, AuthorityProcess, SOURCE};
use crate::network::iroh::transfer_progress_v2::TransferProgressWire;
use anyhow::ensure;
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::time::{timeout, Instant};
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    ConnectionProtocol, ProgressPageReply, ProgressReceiveError, ProgressSnapshotProviderPort,
    ProgressSnapshotQuery, ProgressTerminalRoundPort, TransferProgressProducer,
};
use uc_core::file_transfer::{OutboundProgressStatus, ProgressItemState, ProgressTerminalOutcome};
use uc_core::ids::DeviceId;
use uc_infra_local::SystemElapsedClock;
use uc_sync_protocol::transfer_progress::{read_frame, ProgressFrame};

struct CountedSource {
    source: SettledSnapshot,
    pages: AtomicUsize,
}
#[async_trait]
impl ProgressSnapshotProviderPort for CountedSource {
    async fn snapshot_page(
        &self,
        peer: &DeviceId,
        query: ProgressSnapshotQuery,
    ) -> Result<ProgressPageReply, ProgressReceiveError> {
        self.pages.fetch_add(1, Ordering::SeqCst);
        self.source.snapshot_page(peer, query).await
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "由run-progress-registration-proof提供当前Application fixture二进制"]
async fn actual_failed_receive_results_form_one_complete_legacy_summary_without_ack(
) -> anyhow::Result<()> {
    timeout(Duration::from_secs(30), async {
        let authority = AuthorityProcess::start()?;
        let task = authority.accept(false, 1024)?;
        let mut pair = LegacyPair::new().await?;
        let peer = pair.peer;
        let source = Arc::new(CountedSource {
            source: SettledSnapshot(authority.authority.clone()),
            pages: AtomicUsize::new(0),
        });
        let mut offset = 0;
        let mut revision = None;
        let mut bytes = 0;
        let mut total = Some(0_u64);
        let mut last = None;
        loop {
            // 独立读回原实际事实作为预期；计数只覆盖随后完整通知负责人的实际分页。
            let ProgressPageReply::Current(page) = source
                .source
                .snapshot_page(
                    &peer,
                    ProgressSnapshotQuery {
                        source: SOURCE,
                        attempt: Some(task.descriptor.attempt),
                        offset,
                        revision,
                    },
                )
                .await?
            else {
                anyhow::bail!("actual current failed facts required");
            };
            ensure!(
                page.descriptor == task.descriptor,
                "actual attempt must remain current"
            );
            for value in &page.items {
                let ProgressItemState::Terminal(value) = value else {
                    anyhow::bail!("actual cleanup must settle before terminal notification");
                };
                ensure!(
                    matches!(value.outcome, ProgressTerminalOutcome::Failed(_)),
                    "actual failures cannot become completion"
                );
                bytes += value.bytes;
                total = total.zip(value.total).map(|(sum, value)| sum + value);
                last = Some(*value);
            }
            offset += page.items.len() as u16;
            revision = Some(page.revision);
            if offset == page.total {
                break;
            }
        }
        ensure!(
            offset == 1024,
            "the expected source facts must cover the whole original plan"
        );
        let producer = TransferProgressProducer::new(
            pair.outbound.clone(),
            Arc::new(TransferProgressWire),
            authority.authority.clone(),
            source.clone(),
            Arc::new(SystemElapsedClock),
        );
        let stop = CancellationToken::new();
        let deadline = progress_budget();
        let result = last.ok_or_else(|| anyhow::anyhow!("actual final item required"))?;
        let sending = producer.report_settled(&peer, task.descriptor, result, &deadline, &stop);
        let receiving = async {
            let (lease, _) = timeout(Duration::from_secs(5), pair.incoming.recv())
                .await?
                .ok_or_else(|| anyhow::anyhow!("actual legacy profile connection required"))?;
            ensure!(
                lease.protocol() == ConnectionProtocol::TransferProgressV1,
                "legacy must come from actual negotiation"
            );
            let mut stream = lease
                .accept_uni(Instant::now() + Duration::from_secs(5), &stop)
                .await?;
            let frame = read_frame(&mut stream.stream).await?;
            let mut extra = [0];
            ensure!(
                stream.stream.read(&mut extra).await? == 0,
                "one legacy frame and FIN required"
            );
            Ok::<_, anyhow::Error>(frame)
        };
        let (sent, received) = tokio::join!(sending, receiving);
        let pages = source.pages.load(Ordering::SeqCst);
        pair.close().await?;
        let received = received?;
        ensure!(
            received
                == ProgressFrame {
                    transfer_id_bytes: SOURCE.bytes(),
                    bytes_transferred: bytes,
                    total_bytes: total.filter(|total| *total != 0),
                    status: OutboundProgressStatus::Failed
                },
            "legacy frame must match every real failed fact"
        );
        ensure!(
            pages == 16,
            "the complete action must verify all original-plan pages"
        );
        ensure!(
            matches!(sent, Err(ProgressReceiveError::Unsupported)),
            "legacy FIN cannot provide a business adoption ACK"
        );
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    Ok(())
}
