//! 读取实际接收用例失败清理后的结果，再经过真实受管Query与完整分页读取。
use super::super::profile_probe::ProfilePair;
use super::{decode_item, decode_task, progress_budget, Authority, AuthorityProcess, SOURCE};
use crate::network::iroh::transfer_progress_v2::{TransferProgressInbound, TransferProgressWire};
use anyhow::ensure;
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    ProgressPageReply, ProgressReceiveError, ProgressSnapshotPage, ProgressSnapshotProviderPort,
    ProgressSnapshotQuery, ProgressSnapshotRead, ProgressSnapshotRoundPort,
    TransferProgressReceiver, TransferProgressSnapshotReader,
};
use uc_core::file_transfer::{
    ProgressFailureReason, ProgressItemState, ProgressTerminal, ProgressTerminalOutcome,
};
use uc_core::ids::DeviceId;
use uc_infra_local::SystemElapsedClock;

struct SettledSnapshot<'a>(&'a Authority);
#[async_trait]
impl ProgressSnapshotProviderPort for SettledSnapshot<'_> {
    async fn snapshot_page(
        &self,
        peer: &DeviceId,
        query: ProgressSnapshotQuery,
    ) -> Result<ProgressPageReply, ProgressReceiveError> {
        let page = self
            .0
            .request(json!({"op":"snapshot", "peer":peer.as_str(),
                "source":query.source.bytes(),"attempt":query.attempt.map(|attempt|attempt.bytes()),
                "offset":query.offset,"revision":query.revision,
            }))
            .map_err(|source| {
                ProgressReceiveError::Exchange(
                    source.context("read actual settled query fixture").into(),
                )
            })?;
        if page["not_retained"] == true {
            return Ok(ProgressPageReply::NotRetained);
        }
        let current = decode_task(page["task"].clone()).unwrap();
        let items = page["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| {
                assert_ne!(
                    item["active"], true,
                    "fixture must use actual settled receive results"
                );
                let reason = match item["failed"].as_str().unwrap() {
                    "NetworkUnavailable" => ProgressFailureReason::NetworkUnavailable,
                    "TimedOut" => ProgressFailureReason::TimedOut,
                    "AccessDenied" => ProgressFailureReason::AccessDenied,
                    "StorageUnavailable" => ProgressFailureReason::StorageUnavailable,
                    "IntegrityCheckFailed" => ProgressFailureReason::IntegrityCheckFailed,
                    "Interrupted" => ProgressFailureReason::Interrupted,
                    "Unknown" => ProgressFailureReason::Unknown,
                    _ => panic!("actual query fixture returned an unknown reason"),
                };
                ProgressItemState::Terminal(ProgressTerminal {
                    item: decode_item(item),
                    round: item["round"].as_u64().unwrap() as u32,
                    sequence: item["sequence"].as_u64().unwrap(),
                    result_revision: item["result_revision"].as_u64().unwrap(),
                    bytes: item["bytes"].as_u64().unwrap(),
                    total: item["total"].as_u64(),
                    outcome: ProgressTerminalOutcome::Failed(reason),
                })
            })
            .collect();
        Ok(ProgressPageReply::Current(ProgressSnapshotPage {
            descriptor: current.descriptor,
            revision: page["revision"].as_u64().unwrap(),
            offset: query.offset,
            total: current.descriptor.item_count,
            items,
        }))
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "由run-progress-registration-proof提供当前Application fixture二进制"]
async fn actual_failed_receive_results_cross_sixteen_real_inbound_query_pages() -> anyhow::Result<()>
{
    timeout(Duration::from_secs(30), async {
        let authority = AuthorityProcess::start()?;
        let task = authority.accept(false, 1024)?;
        let fixture = ProfilePair::new().await?;
        let receiver = TransferProgressReceiver::new(fixture.receiving.peer(), SOURCE, task.items.to_vec(), Arc::new(SystemElapsedClock))?;
        let provider = SettledSnapshot(authority.authority.as_ref());
        let stop = CancellationToken::new();
        let serving = async {
            for _ in 0..16 {
                TransferProgressInbound::serve(fixture.receiving.accept_bi().await?, &fixture.receiving, 1,
                    progress_budget(), &stop, &receiver, &provider).await?;
            }
            anyhow::Ok(())
        };
        let reading = async {
            let reader = TransferProgressSnapshotReader::new(fixture.outbound.clone(), Arc::new(TransferProgressWire));
            reader.read_snapshot(&fixture.peer, task, &progress_budget(), &stop).await
        };
        let (read, _) = tokio::try_join!(
            async { reading.await.map_err(anyhow::Error::from) },
            serving,
        )?;
        let ProgressSnapshotRead::Complete(snapshot) = read else { anyhow::bail!("actual complete query required"); };
        ensure!(snapshot.items.len() == 1024, "every actual original-plan item must be returned");
        ensure!(snapshot.items.iter().all(|item| matches!(item, ProgressItemState::Terminal(value) if matches!(value.outcome, ProgressTerminalOutcome::Failed(_)))), "actual failed materialization cannot become completed");
        fixture.close().await?;
        anyhow::Ok(())
    }).await??;
    Ok(())
}
