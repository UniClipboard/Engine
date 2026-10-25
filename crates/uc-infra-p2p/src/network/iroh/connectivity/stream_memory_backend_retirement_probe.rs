//! 原意图退休后，相同设备和任务描述的新意图不接受旧登记或旧终态。
use super::{
    descriptor, item, progress_budget, terminal, Authority, MemoryPair, UnusedSnapshotProvider,
};
use crate::network::iroh::transfer_progress_v2::{TransferProgressInbound, TransferProgressWire};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use tokio::sync::{broadcast::error::TryRecvError, Semaphore};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    ProgressBound, ProgressClaimRefusal, ProgressProjectionAdmission, ProgressProjectionKey,
    ProgressProjectionRegistry, ProgressReceiveError, ProgressSourceTask,
    ProgressTaskAuthorityPort, ProgressTerminalNotice, ProgressWirePort, TransferProgressProducer,
};
use uc_core::file_transfer::{ProgressIdentity, ProgressItem};
use uc_core::ids::DeviceId;
use uc_infra_local::SystemElapsedClock;

struct HeldChallengeAuthority {
    authority: Authority,
    reads: AtomicUsize,
    hold_at: AtomicUsize,
    entered: Semaphore,
    released: Mutex<bool>,
    changed: Condvar,
}
impl HeldChallengeAuthority {
    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.changed.notify_all();
    }
}
struct ReleaseRead(Arc<HeldChallengeAuthority>);
impl Drop for ReleaseRead {
    fn drop(&mut self) {
        self.0.release();
    }
}
impl ProgressTaskAuthorityPort for HeldChallengeAuthority {
    fn current_task(
        &self,
        peer: &DeviceId,
        source: ProgressIdentity,
    ) -> Option<ProgressSourceTask> {
        if self.reads.fetch_add(1, Ordering::SeqCst) == self.hold_at.load(Ordering::SeqCst) {
            self.entered.add_permits(1);
            let mut released = self.released.lock().unwrap();
            while !*released {
                released = self.changed.wait(released).unwrap();
            }
        }
        self.authority.current_task(peer, source)
    }
    fn claim_terminal(
        &self,
        _: &DeviceId,
        _: ProgressIdentity,
        _: ProgressIdentity,
        _: ProgressItem,
    ) -> Result<ProgressSourceTask, ProgressClaimRefusal> {
        panic!("完整登记和原帧重放不领取本地终态");
    }
}

#[derive(Clone, Copy)]
enum Retirement {
    Source,
    Peer,
    Clear,
    Close,
}

async fn register(
    pair: &MemoryPair,
    owner: &ProgressProjectionRegistry,
    producer: &TransferProgressProducer,
    cancel: &CancellationToken,
) -> anyhow::Result<ProgressBound> {
    let serve = async {
        TransferProgressInbound::serve(
            pair.at_sender.accept_bi().await?,
            &pair.at_sender,
            1,
            progress_budget(),
            cancel,
            owner,
            &UnusedSnapshotProvider,
        )
        .await?;
        anyhow::Ok(())
    };
    let send = async {
        anyhow::Ok(
            producer
                .register_current(&pair.sender_id, descriptor().source, cancel)
                .await?,
        )
    };
    let (_, bound) = tokio::try_join!(serve, send)?;
    Ok(bound)
}

async fn retired_intent_cannot_be_repopulated(retirement: Retirement) -> anyhow::Result<()> {
    timeout(Duration::from_secs(15), async {
        let pair = MemoryPair::new().await?;
        let clock = Arc::new(SystemElapsedClock);
        let (owner, worker) = ProgressProjectionRegistry::new(clock.clone());
        let stop = CancellationToken::new();
        let mut running = tokio::spawn(worker.run(stop.clone()));
        let key = ProgressProjectionKey {
            peer: pair.receiver_id,
            source: descriptor().source,
        };
        owner.accept_intent(key, (0..65).map(item).collect())?;
        let authority = Arc::new(HeldChallengeAuthority {
            authority: Authority {
                peer: pair.sender_id,
            },
            reads: AtomicUsize::new(0),
            hold_at: AtomicUsize::new(usize::MAX),
            entered: Semaphore::new(0),
            released: Mutex::new(false),
            changed: Condvar::new(),
        });
        let _release = ReleaseRead(authority.clone());
        let producer = Arc::new(TransferProgressProducer::new(
            pair.receiver.clone(),
            Arc::new(TransferProgressWire),
            authority.clone(),
            Arc::new(UnusedSnapshotProvider),
            clock.clone(),
        ));
        let old = register(&pair, &owner, &producer, &stop).await?;
        // 下一次调用的初读仍正常；只暂停真实Challenge之后的当前事实读取。
        authority
            .hold_at
            .store(authority.reads.load(Ordering::SeqCst) + 1, Ordering::SeqCst);
        let sending = {
            let producer = producer.clone();
            let cancel = stop.clone();
            let target = pair.sender_id;
            tokio::spawn(async move {
                producer
                    .register_current(&target, descriptor().source, &cancel)
                    .await
            })
        };
        let mut pending = Box::pin(async {
            TransferProgressInbound::serve(
                pair.at_sender.accept_bi().await?,
                &pair.at_sender,
                1,
                progress_budget(),
                &stop,
                &owner,
                &UnusedSnapshotProvider,
            )
            .await
        });
        tokio::select! {
            _ = &mut pending => panic!("原登记必须等待已进入的当前事实读取"),
            entered = authority.entered.acquire() => entered?.forget(),
        }
        let (current, replacement_worker) = match retirement {
            Retirement::Source => {
                owner.remove_source(key.source);
                (owner.clone(), None)
            }
            Retirement::Peer => {
                owner.remove_peer(key.peer);
                (owner.clone(), None)
            }
            Retirement::Clear => {
                owner.clear();
                (owner.clone(), None)
            }
            Retirement::Close => {
                owner.close();
                assert!(matches!(
                    owner.accept_intent(key, (0..65).map(item).collect())?,
                    ProgressProjectionAdmission::Closed
                ));
                assert!(timeout(Duration::from_millis(20), &mut running)
                    .await
                    .is_err());
                let (next, worker) = ProgressProjectionRegistry::new(clock);
                (next, Some(worker))
            }
        };
        assert!(owner.snapshot(key).is_none());
        assert!(matches!(
            current.accept_intent(key, (0..65).map(item).collect())?,
            ProgressProjectionAdmission::Retained
        ));
        let empty = current.snapshot(key).unwrap();
        assert!(empty.bound.is_none());
        assert!(empty.items.iter().all(|(_, state)| state.is_none()));
        authority.release();
        let (served, sent) = tokio::join!(&mut pending, sending);
        drop(pending);
        assert!(matches!(served, Err(ProgressReceiveError::UnknownTask)));
        assert!(matches!(sent?, Err(ProgressReceiveError::UnknownTask)));
        assert!(current.snapshot(key).unwrap().bound.is_none());
        let fresh = register(&pair, &current, &producer, &stop).await?;
        assert!(fresh.binding != old.binding);
        assert!(fresh.descriptor == old.descriptor);
        let before = current.snapshot(key).unwrap();
        let mut events = current.subscribe();
        for (binding, stale) in [(old.binding, true), (fresh.binding, false)] {
            let notice = ProgressTerminalNotice {
                binding,
                source: descriptor().source,
                attempt: descriptor().attempt,
                revision: descriptor().revision,
                result: terminal(0),
            };
            let serve = async {
                TransferProgressInbound::serve(
                    pair.at_sender.accept_bi().await?,
                    &pair.at_sender,
                    1,
                    progress_budget(),
                    &stop,
                    &current,
                    &UnusedSnapshotProvider,
                )
                .await
            };
            let send = async {
                let budget = progress_budget();
                let connection = pair
                    .receiver
                    .request(pair.sender_id)
                    .connect(budget.instant(), &stop)
                    .await?;
                let mut stream = connection.open_bi(budget.instant()).await?;
                TransferProgressWire
                    .terminal(&mut stream, &connection, notice, &budget, &stop)
                    .await
            };
            let (served, sent) = tokio::join!(serve, send);
            if stale {
                assert!(matches!(served, Err(ProgressReceiveError::Stale)));
                assert!(matches!(sent, Err(ProgressReceiveError::Stale)));
                let after = current.snapshot(key).unwrap();
                assert_eq!(after.version, before.version);
                assert_eq!(after.items, before.items);
                assert!(after.bound == before.bound);
                assert!(matches!(events.try_recv(), Err(TryRecvError::Empty)));
            } else {
                served?;
                assert_eq!(sent?, notice);
                assert_eq!(current.snapshot(key).unwrap().summary.counts.completed, 1);
                assert!(events.try_recv().is_ok());
            }
        }
        assert_eq!(pair.receiver_dials.load(Ordering::SeqCst), 1);
        current.close();
        owner.close();
        stop.cancel();
        running.await?;
        drop(replacement_worker);
        pair.close().await?;
        anyhow::Ok(())
    })
    .await??;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deleted_source_rejects_delayed_registration_and_old_terminal_after_recreation(
) -> anyhow::Result<()> {
    retired_intent_cannot_be_repopulated(Retirement::Source).await
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retired_peer_projection_rejects_delayed_registration_and_old_terminal_after_recreation(
) -> anyhow::Result<()> {
    retired_intent_cannot_be_repopulated(Retirement::Peer).await
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cleared_projections_reject_delayed_registration_and_old_terminal_after_recreation(
) -> anyhow::Result<()> {
    retired_intent_cannot_be_repopulated(Retirement::Clear).await
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closed_projection_owner_waits_for_actual_registration_and_rejects_old_terminal_in_new_owner(
) -> anyhow::Result<()> {
    retired_intent_cannot_be_repopulated(Retirement::Close).await
}
