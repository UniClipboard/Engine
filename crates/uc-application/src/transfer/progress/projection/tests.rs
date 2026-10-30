use super::super::{
    CurrentProgressTaskPort, ElapsedClockError, ElapsedClockPort, ProgressReceiveError,
    ProgressReceiverPort, ProgressTaskDescriptor, ProgressUpdateNotice,
};
use super::*;
use async_trait::async_trait;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, PoisonError};
use std::time::Duration;
use tokio::sync::{broadcast, Notify, Semaphore};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use uc_core::file_transfer::{
    ProgressActive, ProgressFreshnessView, ProgressIdentity, ProgressItem, ProgressItemKind,
    ProgressPhase,
};
use uc_core::ids::{DeviceId, EntryId};
use uuid::Uuid;

struct Clock(AtomicU64);
impl ElapsedClockPort for Clock {
    fn elapsed(&self) -> Result<Duration, ElapsedClockError> {
        Ok(Duration::from_secs(self.0.load(Ordering::SeqCst)))
    }
}
fn id(n: u16) -> ProgressIdentity {
    let mut bytes = [0; 16];
    bytes[14..].copy_from_slice(&n.to_be_bytes());
    ProgressIdentity::new(bytes)
}
fn key(peer: u16, source: u16) -> ProgressProjectionKey {
    ProgressProjectionKey {
        peer: DeviceId::new(format!("peer-{peer}")),
        source: id(source),
    }
}
fn item(index: u16) -> ProgressItem {
    ProgressItem {
        kind: ProgressItemKind::File,
        index,
    }
}
fn descriptor(key: ProgressProjectionKey, count: u16) -> ProgressTaskDescriptor {
    ProgressTaskDescriptor {
        producer: id(1000),
        revision: 1,
        source: key.source,
        attempt: id(1001),
        item_count: count,
    }
}
struct Current(ProgressTaskDescriptor);
#[async_trait]
impl CurrentProgressTaskPort for Current {
    async fn challenge(
        &mut self,
        _: ProgressIdentity,
    ) -> Result<ProgressTaskDescriptor, ProgressReceiveError> {
        Ok(self.0)
    }
}

#[tokio::test]
async fn original_intent_starts_waiting_and_worker_emits_expiry_without_a_snapshot_poll() {
    let clock = Arc::new(Clock(AtomicU64::new(0)));
    let (registry, worker) = ProgressProjectionRegistry::new(clock.clone());
    let mut events = registry.subscribe();
    let key = key(1, 1);
    registry
        .begin_intent(EntryId::from(
            Uuid::from_bytes((key).source.bytes()).to_string(),
        ))
        .accept(key, vec![item(0)])
        .unwrap();
    assert_eq!(
        events.recv().await.unwrap(),
        ProgressProjectionEvent::Changed(key)
    );
    let initial = registry.snapshot(key).unwrap();
    assert!(initial.bound.is_none());
    assert!(initial.items[0].1.is_none());
    assert_eq!(initial.freshness.unwrap(), ProgressFreshnessView::Waiting);
    let stop = CancellationToken::new();
    let running = tokio::spawn(worker.run(stop.clone()));
    clock.0.store(59, Ordering::SeqCst);
    assert_eq!(
        registry
            .begin_intent(EntryId::from(
                Uuid::from_bytes((key).source.bytes()).to_string()
            ))
            .accept(key, vec![item(0)])
            .unwrap(),
        ProgressProjectionAdmission::Unchanged
    );
    assert!(timeout(Duration::from_millis(30), events.recv())
        .await
        .is_err());
    clock.0.store(60, Ordering::SeqCst);
    registry.inner.wake.notify_one();
    assert_eq!(
        timeout(Duration::from_secs(2), events.recv())
            .await
            .unwrap()
            .unwrap(),
        ProgressProjectionEvent::Changed(key)
    );
    assert_eq!(
        registry.snapshot(key).unwrap().freshness.unwrap(),
        ProgressFreshnessView::Unconfirmed
    );
    assert!(matches!(
        registry
            .begin_intent(EntryId::from(
                Uuid::from_bytes((key).source.bytes()).to_string()
            ))
            .accept(key, vec![item(1)]),
        Err(ProgressReceiveError::InvalidPlan)
    ));
    stop.cancel();
    running.await.unwrap();
    assert!(registry.snapshot(key).is_none());
    assert_eq!(
        registry
            .begin_intent(EntryId::from(
                Uuid::from_bytes((key).source.bytes()).to_string()
            ))
            .accept(key, vec![item(0)])
            .unwrap(),
        ProgressProjectionAdmission::Closed
    );
}
#[tokio::test]
async fn maximum_actual_projections_and_lagged_events_stay_bounded_and_eviction_clears_pinned_state(
) {
    let clock = Arc::new(Clock(AtomicU64::new(0)));
    let (registry, _worker) = ProgressProjectionRegistry::new(clock);
    let mut events = registry.subscribe();
    let plan: Vec<_> = (0..1024).map(item).collect();
    for p in 0..4 {
        for n in 0..64 {
            let key = key(p, n);
            registry
                .begin_intent(EntryId::from(
                    Uuid::from_bytes((key).source.bytes()).to_string(),
                ))
                .accept(key, plan.clone())
                .unwrap();
            let bound = registry
                .register(
                    &key.peer,
                    1,
                    key.source,
                    &mut Current(descriptor(key, 1024)),
                )
                .await
                .unwrap();
            for index in 0..1024 {
                registry
                    .adopt_update(
                        &key.peer,
                        1,
                        ProgressUpdateNotice {
                            binding: bound.binding,
                            source: key.source,
                            attempt: bound.descriptor.attempt,
                            revision: bound.descriptor.revision,
                            update: ProgressActive {
                                item: item(index),
                                round: 0,
                                sequence: 1,
                                bytes: 1,
                                total: None,
                                phase: ProgressPhase::Transferring,
                            },
                        },
                    )
                    .await
                    .unwrap();
            }
        }
    }
    assert!(matches!(
        events.try_recv(),
        Err(broadcast::error::TryRecvError::Lagged(_))
    ));
    let snapshots = registry.snapshots();
    assert_eq!(snapshots.len(), 256);
    assert_eq!(
        snapshots
            .iter()
            .map(|(_, snapshot)| snapshot.items.len())
            .sum::<usize>(),
        256 * 1024
    );
    assert!(snapshots
        .iter()
        .all(|(_, snapshot)| snapshot.items.iter().all(|(_, state)| state.is_some())));
    drop(snapshots);
    let old = key(0, 0);
    let pinned = registry.receiver(old).unwrap();
    assert_eq!(
        registry
            .begin_intent(EntryId::from(
                Uuid::from_bytes((old).source.bytes()).to_string()
            ))
            .accept(old, plan.clone())
            .unwrap(),
        ProgressProjectionAdmission::Unchanged
    );
    registry
        .begin_intent(EntryId::from(
            Uuid::from_bytes((key(0, 64)).source.bytes()).to_string(),
        ))
        .accept(key(0, 64), plan.clone())
        .unwrap();
    assert!(registry.snapshot(old).is_none());
    assert!(pinned.snapshot().items.is_empty());
    assert!(registry.snapshot(key(1, 0)).is_some());
    registry
        .begin_intent(EntryId::from(
            Uuid::from_bytes((key(4, 0)).source.bytes()).to_string(),
        ))
        .accept(key(4, 0), plan)
        .unwrap();
    assert!(registry.snapshot(key(0, 1)).is_none());
    assert_eq!(
        registry
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .slots
            .len(),
        256
    );
    registry.clear();
    assert!(registry.snapshots().is_empty());
    assert_eq!(
        registry
            .begin_intent(EntryId::from(
                Uuid::from_bytes((old).source.bytes()).to_string()
            ))
            .accept(old, vec![item(0)])
            .unwrap(),
        ProgressProjectionAdmission::Retained
    );
}
struct Held {
    descriptor: ProgressTaskDescriptor,
    entered: Arc<Notify>,
    release: Arc<Semaphore>,
}
#[async_trait]
impl CurrentProgressTaskPort for Held {
    async fn challenge(
        &mut self,
        _: ProgressIdentity,
    ) -> Result<ProgressTaskDescriptor, ProgressReceiveError> {
        self.entered.notify_one();
        self.release.acquire().await.unwrap().forget();
        Ok(self.descriptor)
    }
}
#[tokio::test]
async fn retired_pending_challenges_keep_four_one_admission_and_shutdown_waits_for_actual_return() {
    let (registry, worker) = ProgressProjectionRegistry::new(Arc::new(Clock(AtomicU64::new(0))));
    let stop = CancellationToken::new();
    let mut running = tokio::spawn(worker.run(stop.clone()));
    let releases: Vec<_> = (0..4).map(|_| Arc::new(Semaphore::new(0))).collect();
    let mut calls = Vec::new();
    for p in 0..4 {
        let key = key(p, 0);
        registry
            .begin_intent(EntryId::from(
                Uuid::from_bytes((key).source.bytes()).to_string(),
            ))
            .accept(key, vec![item(0)])
            .unwrap();
        let entered = Arc::new(Notify::new());
        let signal = entered.clone();
        let gate = releases[usize::from(p)].clone();
        let owner = registry.clone();
        calls.push(tokio::spawn(async move {
            owner
                .register(
                    &key.peer,
                    1,
                    key.source,
                    &mut Held {
                        descriptor: descriptor(key, 1),
                        entered: signal,
                        release: gate,
                    },
                )
                .await
        }));
        timeout(Duration::from_secs(2), entered.notified())
            .await
            .unwrap();
    }
    let extra = key(4, 0);
    registry
        .begin_intent(EntryId::from(
            Uuid::from_bytes((extra).source.bytes()).to_string(),
        ))
        .accept(extra, vec![item(0)])
        .unwrap();
    assert!(matches!(
        registry
            .register(
                &extra.peer,
                1,
                extra.source,
                &mut Current(descriptor(extra, 1))
            )
            .await,
        Err(ProgressReceiveError::Busy)
    ));
    // 全局释放一个其他端后，同端仍不得借用剩余额度重叠登记。
    releases[3].add_permits(1);
    assert!(calls.pop().unwrap().await.unwrap().is_ok());
    assert!(registry
        .register(
            &extra.peer,
            1,
            extra.source,
            &mut Current(descriptor(extra, 1))
        )
        .await
        .is_ok());
    registry.remove(key(0, 0));
    registry
        .begin_intent(EntryId::from(
            Uuid::from_bytes((key(0, 0)).source.bytes()).to_string(),
        ))
        .accept(key(0, 0), vec![item(0)])
        .unwrap();
    let same = key(0, 0);
    assert!(matches!(
        registry
            .register(
                &same.peer,
                1,
                same.source,
                &mut Current(descriptor(same, 1))
            )
            .await,
        Err(ProgressReceiveError::Busy)
    ));
    assert!(registry.snapshot(same).unwrap().bound.is_none());
    {
        let state = registry.inner.state.lock().unwrap();
        assert_eq!(state.registrations.active(), 3);
        assert_eq!(state.registrations.queued(), 0);
    }
    stop.cancel();
    assert!(timeout(Duration::from_millis(30), &mut running)
        .await
        .is_err());
    for release in &releases[..3] {
        release.add_permits(1);
    }
    for call in calls {
        assert!(matches!(
            call.await.unwrap(),
            Err(ProgressReceiveError::UnknownTask)
        ));
    }
    timeout(Duration::from_secs(2), running)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        registry.inner.state.lock().unwrap().registrations.active(),
        0
    );
}

#[tokio::test]
async fn source_deletion_and_peer_retirement_remove_only_their_authorized_scopes() {
    let (registry, _worker) = ProgressProjectionRegistry::new(Arc::new(Clock(AtomicU64::new(0))));
    for key in [key(0, 0), key(1, 0), key(1, 1), key(2, 2)] {
        registry
            .begin_intent(EntryId::from(
                Uuid::from_bytes((key).source.bytes()).to_string(),
            ))
            .accept(key, vec![item(0)])
            .unwrap();
    }
    registry.remove_source(id(0));
    assert!(registry.snapshot(key(0, 0)).is_none());
    assert!(registry.snapshot(key(1, 0)).is_none());
    assert_eq!(registry.snapshots().len(), 2);
    registry.remove_peer(key(1, 1).peer);
    assert!(registry.snapshot(key(1, 1)).is_none());
    assert!(registry.snapshot(key(2, 2)).is_some());
    assert_eq!(registry.snapshots().len(), 1);
}
struct CountedClock {
    now: AtomicU64,
    reads: AtomicUsize,
}
impl ElapsedClockPort for CountedClock {
    fn elapsed(&self) -> Result<Duration, ElapsedClockError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        Ok(Duration::from_secs(self.now.load(Ordering::SeqCst)))
    }
}
#[tokio::test]
async fn already_unconfirmed_views_stop_timer_polling_until_a_new_valid_notification() {
    let clock = Arc::new(CountedClock {
        now: AtomicU64::new(0),
        reads: AtomicUsize::new(0),
    });
    let (registry, worker) = ProgressProjectionRegistry::new(clock.clone());
    let key = key(1, 1);
    registry
        .begin_intent(EntryId::from(
            Uuid::from_bytes((key).source.bytes()).to_string(),
        ))
        .accept(key, vec![item(0)])
        .unwrap();
    let mut events = registry.subscribe();
    let stop = CancellationToken::new();
    let running = tokio::spawn(worker.run(stop.clone()));
    clock.now.store(60, Ordering::SeqCst);
    registry.inner.wake.notify_one();
    timeout(Duration::from_secs(2), events.recv())
        .await
        .unwrap()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(220)).await;
    let reads = clock.reads.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(220)).await;
    assert_eq!(clock.reads.load(Ordering::SeqCst), reads);
    let bound = registry
        .register(&key.peer, 1, key.source, &mut Current(descriptor(key, 1)))
        .await
        .unwrap();
    registry
        .adopt_update(
            &key.peer,
            1,
            ProgressUpdateNotice {
                binding: bound.binding,
                source: key.source,
                attempt: bound.descriptor.attempt,
                revision: bound.descriptor.revision,
                update: ProgressActive {
                    item: item(0),
                    round: 0,
                    sequence: 1,
                    bytes: 1,
                    total: None,
                    phase: ProgressPhase::Transferring,
                },
            },
        )
        .await
        .unwrap();
    assert_eq!(
        registry.snapshot(key).unwrap().freshness.unwrap(),
        ProgressFreshnessView::Live
    );
    while events.try_recv().is_ok() {}
    clock.now.store(120, Ordering::SeqCst);
    registry.inner.wake.notify_one();
    timeout(Duration::from_secs(2), events.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        registry.snapshot(key).unwrap().freshness.unwrap(),
        ProgressFreshnessView::Unconfirmed
    );
    stop.cancel();
    running.await.unwrap();
}

mod reconciliation;
mod unattributed;

mod intent;
