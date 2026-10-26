//! 当前尝试替换后，真实专属连接上的旧登记、通知与迟到精确ACK都不能复活旧事实。
use super::super::profile_probe::ProfilePair;
use super::{item, progress_budget, terminal, Authority, UnusedSnapshotProvider, SOURCE};
use crate::network::iroh::transfer_progress_v2::{TransferProgressInbound, TransferProgressWire};
use async_trait::async_trait;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast::error::TryRecvError, Notify, Semaphore};
use tokio::time::{timeout, Instant};
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    CurrentProgressTaskPort, ProgressBound, ProgressContactNotice, ProgressProjectionKey,
    ProgressProjectionRegistry, ProgressProjectionWorker, ProgressReceiveError,
    ProgressReceiverPort, ProgressTaskAuthorityPort, ProgressTerminalNotice,
    ProgressTerminalRoundPort, ProgressUpdateNotice, ProgressWirePort, TransferProgressProducer,
};
use uc_core::file_transfer::{
    ProgressActive, ProgressFreshnessView, ProgressIdentity, ProgressItemState, ProgressPhase,
};
use uc_core::ids::{DeviceId, EntryId};
use uc_infra_local::SystemElapsedClock;
use uuid::Uuid;

fn projection(
    pair: &ProfilePair,
) -> anyhow::Result<(
    ProgressProjectionRegistry,
    ProgressProjectionKey,
    ProgressProjectionWorker,
)> {
    let (registry, worker) = ProgressProjectionRegistry::new(Arc::new(SystemElapsedClock));
    let key = ProgressProjectionKey {
        peer: pair.receiving.peer(),
        source: SOURCE,
    };
    registry
        .begin_intent(EntryId::from(Uuid::from_bytes(SOURCE.bytes()).to_string()))
        .accept(key, vec![item()])?;
    Ok((registry, key, worker))
}

async fn serve(
    pair: &ProfilePair,
    owner: &dyn ProgressReceiverPort,
) -> Result<(), ProgressReceiveError> {
    TransferProgressInbound::serve(
        pair.receiving.accept_bi().await?,
        &pair.receiving,
        1,
        progress_budget(),
        &CancellationToken::new(),
        owner,
        &UnusedSnapshotProvider,
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn original_attempt_registration_updates_and_terminals_cannot_replace_current_native_projection(
) -> anyhow::Result<()> {
    timeout(Duration::from_secs(15), async {
        let pair = ProfilePair::new().await?;
        let (registry, key, _worker) = projection(&pair)?;
        let authority = Arc::new(Authority::default());
        let producer = TransferProgressProducer::new(
            pair.outbound.clone(),
            Arc::new(TransferProgressWire),
            authority.clone(),
            Arc::new(UnusedSnapshotProvider),
            Arc::new(SystemElapsedClock),
        );
        let stop = CancellationToken::new();
        let (registered, served) = tokio::join!(
            producer.register_current(&pair.peer, SOURCE, &stop),
            serve(&pair, &registry)
        );
        served?;
        let old = registered?;
        authority.0.store(2, Ordering::SeqCst);
        let (registered, served) = tokio::join!(
            producer.register_current(&pair.peer, SOURCE, &stop),
            serve(&pair, &registry)
        );
        served?;
        let current = registered?;
        assert_ne!(old.descriptor.attempt, current.descriptor.attempt);
        assert_ne!(old.binding, current.binding);
        let connection = pair
            .outbound
            .request(pair.peer)
            .connect(Instant::now() + Duration::from_secs(5), &stop)
            .await?;
        let active = ProgressUpdateNotice {
            binding: current.binding,
            source: SOURCE,
            attempt: current.descriptor.attempt,
            revision: current.descriptor.revision,
            update: ProgressActive {
                item: item(),
                round: 0,
                sequence: 1,
                bytes: 2,
                total: Some(10),
                phase: ProgressPhase::Transferring,
            },
        };
        let budget = progress_budget();
        let mut stream = connection.open_bi(budget.instant()).await?;
        let (sent, served) = tokio::join!(
            TransferProgressWire.update(&mut stream, &connection, active, &budget, &stop),
            serve(&pair, &registry)
        );
        sent?;
        served?;
        drop(stream);
        let before = registry.snapshot(key).unwrap();
        assert_eq!(
            before.items[0].1,
            Some(ProgressItemState::Active(active.update))
        );
        let mut events = registry.subscribe();
        // 旧动作重新完成整个挑战，仍只能描述旧尝试；不得把当前binding或事实回退。
        let stale = TransferProgressProducer::new(
            pair.outbound.clone(),
            Arc::new(TransferProgressWire),
            Arc::new(Authority::default()),
            Arc::new(UnusedSnapshotProvider),
            Arc::new(SystemElapsedClock),
        );
        let (registered, served) = tokio::join!(
            stale.register_current(&pair.peer, SOURCE, &stop),
            serve(&pair, &registry)
        );
        assert!(matches!(registered, Err(ProgressReceiveError::Stale)));
        assert!(matches!(served, Err(ProgressReceiveError::Stale)));
        let budget = progress_budget();
        let mut stream = connection.open_bi(budget.instant()).await?;
        let old_update = ProgressUpdateNotice {
            binding: old.binding,
            attempt: old.descriptor.attempt,
            revision: old.descriptor.revision,
            update: ProgressActive {
                bytes: 9,
                sequence: 999,
                ..active.update
            },
            ..active
        };
        let (_sent, served) = tokio::join!(
            TransferProgressWire.update(&mut stream, &connection, old_update, &budget, &stop),
            serve(&pair, &registry)
        );
        assert!(matches!(served, Err(ProgressReceiveError::Stale)));
        drop(stream);
        let budget = progress_budget();
        let mut stream = connection.open_bi(budget.instant()).await?;
        let old_terminal = ProgressTerminalNotice {
            binding: old.binding,
            source: SOURCE,
            attempt: old.descriptor.attempt,
            revision: old.descriptor.revision,
            result: terminal(10),
        };
        let (sent, served) = tokio::join!(
            TransferProgressWire.terminal(&mut stream, &connection, old_terminal, &budget, &stop),
            serve(&pair, &registry)
        );
        assert!(matches!(sent, Err(ProgressReceiveError::Stale)));
        assert!(matches!(served, Err(ProgressReceiveError::Stale)));
        drop(stream);
        let after = registry.snapshot(key).unwrap();
        assert!(after.bound == before.bound && after.task == before.task);
        assert_eq!(after.items, before.items);
        assert_eq!(after.summary, before.summary);
        assert_eq!(after.version, before.version);
        assert!(matches!(events.try_recv(), Err(TryRecvError::Empty)));
        // 拒绝旧动作后，当前尝试仍经原生产发送／采用链取得自己的精确ACK。
        let budget = progress_budget();
        let serving = async {
            serve(&pair, &registry).await?;
            serve(&pair, &registry).await
        };
        let (sent, served) = tokio::join!(
            producer.report_settled(&pair.peer, current.descriptor, terminal(10), &budget, &stop),
            serving
        );
        sent?;
        served?;
        let final_view = registry.snapshot(key).unwrap();
        assert_eq!(final_view.task.unwrap().attempt, current.descriptor.attempt);
        assert_eq!(
            final_view.items[0].1,
            Some(ProgressItemState::Terminal(terminal(10)))
        );
        assert_eq!(
            final_view.freshness.unwrap(),
            ProgressFreshnessView::Settled
        );
        drop(connection);
        pair.close().await?;
        anyhow::Ok(())
    })
    .await??;
    Ok(())
}

struct AdoptedAck {
    registry: ProgressProjectionRegistry,
    entered: Notify,
    release: Semaphore,
}
#[async_trait]
impl ProgressReceiverPort for AdoptedAck {
    async fn register(
        &self,
        peer: &DeviceId,
        generation: u64,
        source: ProgressIdentity,
        exchange: &mut dyn CurrentProgressTaskPort,
    ) -> Result<ProgressBound, ProgressReceiveError> {
        self.registry
            .register(peer, generation, source, exchange)
            .await
    }
    async fn adopt_contact(
        &self,
        peer: &DeviceId,
        generation: u64,
        notice: ProgressContactNotice,
    ) -> Result<(), ProgressReceiveError> {
        self.registry.adopt_contact(peer, generation, notice).await
    }
    async fn adopt_update(
        &self,
        peer: &DeviceId,
        generation: u64,
        notice: ProgressUpdateNotice,
    ) -> Result<(), ProgressReceiveError> {
        self.registry.adopt_update(peer, generation, notice).await
    }
    async fn adopt_terminal(
        &self,
        peer: &DeviceId,
        generation: u64,
        notice: ProgressTerminalNotice,
    ) -> Result<ProgressTerminalNotice, ProgressReceiveError> {
        let adopted = self
            .registry
            .adopt_terminal(peer, generation, notice)
            .await?;
        self.entered.notify_one();
        self.release.acquire().await.unwrap().forget();
        Ok(adopted)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exact_native_ack_after_attempt_replacement_cannot_confirm_old_action_or_poison_new_result(
) -> anyhow::Result<()> {
    timeout(Duration::from_secs(15), async {
        let pair = ProfilePair::new().await?;
        let (registry, key, _worker) = projection(&pair)?;
        let owner = AdoptedAck {
            registry: registry.clone(),
            entered: Notify::new(),
            release: Semaphore::new(0),
        };
        let authority = Arc::new(Authority::default());
        let producer = TransferProgressProducer::new(
            pair.outbound.clone(),
            Arc::new(TransferProgressWire),
            authority.clone(),
            Arc::new(UnusedSnapshotProvider),
            Arc::new(SystemElapsedClock),
        );
        let old = authority
            .current_task(&pair.peer, SOURCE)
            .unwrap()
            .descriptor;
        let stop = CancellationToken::new();
        let budget = progress_budget();
        let serving = async {
            serve(&pair, &owner).await?;
            serve(&pair, &owner).await
        };
        let replace = async {
            owner.entered.notified().await;
            assert_eq!(
                registry.snapshot(key).unwrap().items[0].1,
                Some(ProgressItemState::Terminal(terminal(10)))
            );
            authority.0.store(2, Ordering::SeqCst);
            owner.release.add_permits(1);
        };
        let (sent, served, ()) = tokio::join!(
            producer.report_settled(&pair.peer, old, terminal(10), &budget, &stop),
            serving,
            replace
        );
        served?;
        assert!(
            matches!(sent, Err(ProgressReceiveError::Stale)),
            "原生精确ACK返回后必须再次核对当前权威"
        );
        let current = authority.current_task(&pair.peer, SOURCE).unwrap();
        assert_ne!(current.descriptor.attempt, old.attempt);
        assert!(current.notification_closed.is_empty());
        let budget = progress_budget();
        let serving = async {
            serve(&pair, &registry).await?;
            serve(&pair, &registry).await
        };
        let (sent, served) = tokio::join!(
            producer.report_settled(&pair.peer, current.descriptor, terminal(7), &budget, &stop),
            serving
        );
        sent?;
        served?;
        let final_view = registry.snapshot(key).unwrap();
        assert_eq!(final_view.task.unwrap().attempt, current.descriptor.attempt);
        assert_eq!(
            final_view.items[0].1,
            Some(ProgressItemState::Terminal(terminal(7)))
        );
        assert_eq!(
            final_view.freshness.unwrap(),
            ProgressFreshnessView::Settled
        );
        pair.close().await?;
        anyhow::Ok(())
    })
    .await??;
    Ok(())
}
