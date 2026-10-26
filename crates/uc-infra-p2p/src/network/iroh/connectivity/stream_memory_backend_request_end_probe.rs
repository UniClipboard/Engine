//! 完整帧已到达而FIN仍在途时，旧准入不能授权请求结束后的业务采用或读取。
use super::UnusedSnapshotProvider;
use super::{denied, descriptor, item, progress_budget, Authority, MemoryPair, Snapshot};
use crate::network::iroh::transfer_progress_v2::{TransferProgressInbound, TransferProgressWire};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::{timeout, Instant};
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    ElapsedClockError, ElapsedClockPort, ProgressProjectionKey, ProgressProjectionRegistry,
    ProgressReceiveError, TransferProgressProducer,
};
use uc_core::file_transfer::ProgressFreshnessView;
use uc_core::ids::EntryId;
use uc_infra_local::SystemElapsedClock;
use uc_sync_protocol::transfer_progress::v2::{
    write_frame, ActiveProgress, Envelope, ItemKind, LogicalItem, Message, Phase, TaskScope,
    TerminalOutcome, TerminalResult, WireId,
};
use uuid::Uuid;

struct Clock(AtomicU64);
impl ElapsedClockPort for Clock {
    fn elapsed(&self) -> Result<Duration, ElapsedClockError> {
        Ok(Duration::from_secs(self.0.load(Ordering::SeqCst)))
    }
}
#[derive(Clone, Copy, Debug)]
enum Operation {
    Query,
    Terminal,
    Update,
    Contact,
}
async fn ended_request_keeps_current_authority(operation: Operation) -> anyhow::Result<()> {
    timeout(Duration::from_secs(15), async {
        let pair = MemoryPair::new().await?;
        let clock = Arc::new(Clock(AtomicU64::new(0)));
        let (registry, _worker) = ProgressProjectionRegistry::new(clock.clone());
        let key = ProgressProjectionKey {
            peer: pair.receiver_id,
            source: descriptor().source,
        };
        registry
            .begin_intent(EntryId::from(
                Uuid::from_bytes((key).source.bytes()).to_string(),
            ))
            .accept(key, (0..65).map(item).collect())?;
        let snapshot = Snapshot {
            peer: pair.receiver_id,
            pages: AtomicUsize::new(0),
        };
        let cancel = CancellationToken::new();
        let producer = TransferProgressProducer::new(
            pair.receiver.clone(),
            Arc::new(TransferProgressWire),
            Arc::new(Authority {
                peer: pair.sender_id,
            }),
            Arc::new(UnusedSnapshotProvider),
            Arc::new(SystemElapsedClock),
        );
        let receive = async {
            TransferProgressInbound::serve(
                pair.at_sender.accept_bi().await?,
                &pair.at_sender,
                1,
                progress_budget(),
                &cancel,
                &registry,
                &snapshot,
            )
            .await?;
            anyhow::Ok(())
        };
        let (_, bound) = tokio::try_join!(receive, async {
            Ok::<_, anyhow::Error>(
                producer
                    .register_current(&pair.sender_id, descriptor().source, &cancel)
                    .await?,
            )
        })?;
        clock.0.store(61, Ordering::SeqCst);
        let before = registry.snapshot(key).unwrap();
        assert_eq!(
            *before.freshness.as_ref().unwrap(),
            ProgressFreshnessView::Unconfirmed
        );
        let binding = WireId(bound.binding.bytes());
        let scope = TaskScope {
            source_entry: WireId(descriptor().source.bytes()),
            attempt: WireId(descriptor().attempt.bytes()),
        };
        let logical = LogicalItem {
            kind: ItemKind::File,
            index: 0,
        };
        let message = match operation {
            Operation::Query => Message::Query {
                challenge: WireId([9; 16]),
                source_entry: scope.source_entry,
                expected_attempt: Some(scope.attempt),
                offset: 0,
                snapshot_revision: None,
            },
            Operation::Terminal => Message::Terminal {
                binding,
                scope,
                owner_revision: descriptor().revision,
                result: TerminalResult {
                    item: logical,
                    round: 0,
                    sequence: 1,
                    result_revision: 1,
                    bytes: 16,
                    total: Some(16),
                    outcome: TerminalOutcome::Completed,
                },
            },
            Operation::Update => Message::Update {
                binding,
                scope,
                owner_revision: descriptor().revision,
                progress: ActiveProgress {
                    item: logical,
                    round: 0,
                    sequence: 1,
                    bytes: 8,
                    total: Some(16),
                    phase: Phase::Transferring,
                },
            },
            Operation::Contact => Message::Contact {
                binding,
                scope,
                owner_revision: descriptor().revision,
                sequence: 1,
            },
        };
        let connection = pair
            .receiver
            .request(pair.sender_id)
            .connect(Instant::now() + Duration::from_secs(5), &cancel)
            .await?;
        let mut stream = connection
            .open_bi(Instant::now() + Duration::from_secs(5))
            .await?;
        write_frame(
            &mut stream.send,
            &Envelope {
                message,
                trace_context: None,
            },
        )
        .await?;
        let inbound = pair.at_sender.accept_bi().await?;
        let mut serving = Box::pin(TransferProgressInbound::serve(
            inbound,
            &pair.at_sender,
            1,
            progress_budget(),
            &cancel,
            &registry,
            &snapshot,
        ));
        // 完整帧可以读取，但原交换尚无FIN；原真实codec进入结束边界等待。
        assert!(futures_util::poll!(serving.as_mut()).is_pending());
        assert_eq!(snapshot.pages.load(Ordering::SeqCst), 0);
        pair.allowed.store(false, Ordering::SeqCst);
        stream.send.finish()?;
        let result = serving.as_mut().await;
        drop(serving);
        let after = registry.snapshot(key).unwrap();
        let reads = snapshot.pages.load(Ordering::SeqCst);
        drop(stream);
        drop(connection);
        registry.close();
        pair.close().await?;
        assert_eq!(
            reads, 0,
            "{operation:?} began an Application read after revocation during request EOF"
        );
        assert_eq!(
            after.items, before.items,
            "{operation:?} changed real projected facts after revocation during request EOF"
        );
        assert_eq!(
            after.version, before.version,
            "{operation:?} refreshed real contact after revocation during request EOF"
        );
        assert_eq!(
            *after.freshness.as_ref().unwrap(),
            ProgressFreshnessView::Unconfirmed
        );
        assert!(matches!(result, Err(ProgressReceiveError::Connection(error)) if denied(&error)));
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    Ok(())
}
#[tokio::test]
async fn query_eof_after_revocation_does_not_begin_application_snapshot_read() -> anyhow::Result<()>
{
    ended_request_keeps_current_authority(Operation::Query).await
}
#[tokio::test]
async fn terminal_eof_after_revocation_does_not_adopt_real_result() -> anyhow::Result<()> {
    ended_request_keeps_current_authority(Operation::Terminal).await
}
#[tokio::test]
async fn update_eof_after_revocation_does_not_adopt_real_progress() -> anyhow::Result<()> {
    ended_request_keeps_current_authority(Operation::Update).await
}
#[tokio::test]
async fn contact_eof_after_revocation_does_not_refresh_real_freshness() -> anyhow::Result<()> {
    ended_request_keeps_current_authority(Operation::Contact).await
}
