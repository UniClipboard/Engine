//! 核对在原监督器排队期间不得重新取得新会话；下一次显式核对仍可使用新会话。
use super::session_probe::stopped;
use super::{
    descriptor, item, progress_budget, terminal, Authority, MemoryPair, Snapshot,
    UnusedNotifications, UnusedSnapshotProvider, SOURCE,
};
use crate::network::iroh::transfer_progress_v2::{TransferProgressInbound, TransferProgressWire};
use async_trait::async_trait;
use futures_util::poll;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    ProgressClaimRefusal, ProgressExchangeDeadline, ProgressExchangeService, ProgressProjectionKey,
    ProgressProjectionRegistry, ProgressReceiveError, ProgressSnapshotRead, ProgressSourceTask,
    ProgressSubmission, ProgressTaskAuthorityPort, ProgressTaskDescriptor,
    ProgressTerminalRoundPort, TransferProgressProducer, TransferProgressReconciler,
    TransferProgressSnapshotReader,
};
use uc_core::file_transfer::{ProgressIdentity, ProgressItem, ProgressTerminal};
use uc_core::ids::DeviceId;
use uc_infra_local::SystemElapsedClock;

struct BlockingAuthority {
    peer: DeviceId,
    claimed: AtomicBool,
}
impl BlockingAuthority {
    fn task(&self) -> ProgressSourceTask {
        ProgressSourceTask {
            descriptor: ProgressTaskDescriptor {
                source: ProgressIdentity::new([8; 16]),
                item_count: 1,
                ..descriptor()
            },
            items: vec![item(0)].into(),
            notification_closed: Arc::new(if self.claimed.load(Ordering::SeqCst) {
                HashSet::from([item(0)])
            } else {
                HashSet::new()
            }),
        }
    }
}
impl ProgressTaskAuthorityPort for BlockingAuthority {
    fn current_task(
        &self,
        peer: &DeviceId,
        source: ProgressIdentity,
    ) -> Option<ProgressSourceTask> {
        (peer == &self.peer && source == self.task().descriptor.source).then(|| self.task())
    }
    fn claim_terminal(
        &self,
        peer: &DeviceId,
        source: ProgressIdentity,
        attempt: ProgressIdentity,
        item: ProgressItem,
    ) -> Result<ProgressSourceTask, ProgressClaimRefusal> {
        let current = self
            .current_task(peer, source)
            .ok_or(ProgressClaimRefusal::NotRetained)?;
        if attempt != current.descriptor.attempt || !current.items.contains(&item) {
            return Err(ProgressClaimRefusal::InvalidItem);
        }
        if self.claimed.swap(true, Ordering::SeqCst) {
            return Err(ProgressClaimRefusal::AlreadyObserved);
        }
        Ok(self.task())
    }
}
struct BlockingRound {
    entered: Semaphore,
    finish: Semaphore,
}
#[async_trait]
impl ProgressTerminalRoundPort for BlockingRound {
    async fn report_settled(
        &self,
        _: &DeviceId,
        _: ProgressTaskDescriptor,
        _: ProgressTerminal,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<(), ProgressReceiveError> {
        deadline.check(cancel)?;
        self.entered.add_permits(1);
        // 一项无关本机工作占据同对端实际额度；只有测试明确结束它才交回许可。
        self.finish.acquire().await.unwrap().forget();
        Ok(())
    }
}
async fn serve_query_after_actual_dial(
    pair: &MemoryPair,
    registry: &ProgressProjectionRegistry,
    snapshot: &Snapshot,
    cancel: &CancellationToken,
) -> anyhow::Result<()> {
    // 零拨号预期下不启动native accept；不会为测试自身制造未结束读取。
    while pair.sender_dials.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    for _ in 0..2 {
        TransferProgressInbound::serve(
            pair.at_receiver.accept_bi().await?,
            &pair.at_receiver,
            1,
            progress_budget(),
            cancel,
            registry,
            snapshot,
        )
        .await?;
    }
    Ok(())
}
#[tokio::test]
async fn queued_query_keeps_original_session_and_a_later_explicit_query_can_use_the_new_session(
) -> anyhow::Result<()> {
    timeout(Duration::from_secs(15), async {
        let pair = MemoryPair::new().await?;
        let clock = Arc::new(SystemElapsedClock);
        let (registry, projection_worker) = ProgressProjectionRegistry::new(clock.clone());
        let key = ProgressProjectionKey { peer: pair.receiver_id, source: SOURCE };
        registry.accept_intent(key, (0..65).map(item).collect())?;
        let cancel = CancellationToken::new();
        let projections = tokio::spawn(projection_worker.run(cancel.clone()));
        let producer = TransferProgressProducer::new(pair.receiver.clone(), Arc::new(TransferProgressWire), Arc::new(Authority { peer: pair.sender_id }), clock.clone());
        let register = async { producer.register_current(&pair.sender_id, SOURCE, &cancel).await?; anyhow::Ok(()) };
        let serve = async { TransferProgressInbound::serve(pair.at_sender.accept_bi().await?, &pair.at_sender, 1, progress_budget(), &cancel, &registry, &UnusedSnapshotProvider).await?; anyhow::Ok(()) };
        tokio::try_join!(register, serve)?;
        let before = registry.snapshot(key).unwrap();
        let blocking = Arc::new(BlockingRound { entered: Semaphore::new(0), finish: Semaphore::new(0) });
        let authority = Arc::new(BlockingAuthority { peer: pair.receiver_id, claimed: AtomicBool::new(false) });
        let task = authority.task();
        let (service, worker) = ProgressExchangeService::new(clock, authority, blocking.clone(), Arc::new(TransferProgressSnapshotReader::new(pair.sender.clone(), Arc::new(TransferProgressWire))), Arc::new(UnusedNotifications));
        assert_eq!(service.submit_settled(pair.receiver_id, task.descriptor.source, task.descriptor.attempt, terminal(0))?, ProgressSubmission::Retained);
        let reconciler = TransferProgressReconciler::new(registry.clone(), service);
        let queries = tokio::spawn(worker.run(cancel.clone()));
        timeout(Duration::from_secs(2), blocking.entered.acquire()).await??.forget();
        let mut old = Box::pin(reconciler.refresh(key, &cancel));
        assert!(poll!(old.as_mut()).is_pending());
        assert_eq!(pair.sender_dials.load(Ordering::SeqCst), 0);
        pair.sender.disconnect_all().await?;
        blocking.finish.add_permits(1);
        let snapshot = Snapshot { peer: pair.sender_id, pages: AtomicUsize::new(0) };
        let mut serve = Box::pin(serve_query_after_actual_dial(&pair, &registry, &snapshot, &cancel));
        let old_result = tokio::select! {
            biased;
            result = old.as_mut() => result,
            served = serve.as_mut() => { served?; old.as_mut().await },
        };
        drop(serve);
        drop(old);
        let old_dials = pair.sender_dials.load(Ordering::SeqCst);
        let untouched = registry.snapshot(key).unwrap();
        let mut new_succeeded = false;
        if old_dials == 0 {
            let mut new = Box::pin(reconciler.refresh(key, &cancel));
            let mut serve = Box::pin(serve_query_after_actual_dial(&pair, &registry, &snapshot, &cancel));
            let result = tokio::select! {
                biased;
                result = new.as_mut() => result,
                served = serve.as_mut() => { served?; new.as_mut().await },
            };
            new_succeeded = matches!(result.as_ref(), Ok(ProgressSnapshotRead::Complete(_)));
        }
        let after = registry.snapshot(key).unwrap();
        registry.close();
        cancel.cancel();
        projections.await?;
        queries.await?;
        pair.close().await?;
        assert_eq!(old_dials, 0, "a queued old query dialed under the restarted session");
        assert!(matches!(old_result.as_ref(), Err(ProgressReceiveError::Connection(error)) if stopped(error)));
        assert_eq!(untouched.version, before.version);
        assert_eq!(untouched.items, before.items);
        assert!(new_succeeded, "a new explicit query must remain usable after session restart");
        assert_eq!(snapshot.pages.load(Ordering::SeqCst), 2);
        assert_eq!(after.summary.counts.completed, 65);
        Ok::<_, anyhow::Error>(())
    }).await??;
    Ok(())
}
