use super::{descriptor, id, item, key, Clock, Current};
use crate::transfer::progress::{
    ProgressBound, ProgressClaimRefusal, ProgressExchangeDeadline, ProgressExchangeService,
    ProgressLiveRoundPort, ProgressProjectionKey, ProgressProjectionRegistry, ProgressReceiveError,
    ProgressReceiverPort, ProgressSnapshot, ProgressSnapshotRead, ProgressSnapshotRound,
    ProgressSnapshotRoundPort, ProgressSnapshotTarget, ProgressSourceTask,
    ProgressTaskAuthorityPort, ProgressTaskDescriptor, ProgressTerminalNotice,
    ProgressTerminalRoundPort, TransferProgressReconciler,
};
use async_trait::async_trait;
use futures::poll;
use std::error::Error;
use std::io;
use std::mem::discriminant;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{Notify, Semaphore};
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use uc_core::file_transfer::{
    ProgressActive, ProgressFailureReason, ProgressFreshnessView, ProgressIdentity, ProgressItem,
    ProgressItemState, ProgressPhase, ProgressSnapshotRefusal, ProgressTerminal,
    ProgressTerminalOutcome,
};
use uc_core::ids::DeviceId;

struct Query {
    entered: Notify,
    release: Semaphore,
    values: Mutex<Vec<ProgressItemState>>,
    revision: AtomicU64,
    fail: AtomicBool,
    remote: Mutex<ProgressTaskDescriptor>,
    absent: AtomicBool,
    reply: Mutex<Option<ProgressSnapshotRead>>,
}
impl ProgressSnapshotRoundPort for Query {
    fn read_snapshot(
        self: Arc<Self>,
        _: DeviceId,
        expected: ProgressSnapshotTarget,
        _: Arc<ProgressExchangeDeadline>,
        _: CancellationToken,
    ) -> ProgressSnapshotRound {
        Box::pin(async move {
            let values = self.values.lock().unwrap().clone();
            let remote = *self.remote.lock().unwrap();
            let revision = self.revision.load(Ordering::SeqCst);
            let absent = self.absent.load(Ordering::SeqCst);
            let reply = self.reply.lock().unwrap().take();
            assert_eq!(expected.source, remote.source);
            assert!(expected.expected.is_none_or(|expected| expected == remote));
            assert!(expected.items.iter().copied().eq([item(0), item(1)]));
            self.entered.notify_one();
            self.release.acquire().await.unwrap().forget();
            if self.fail.load(Ordering::SeqCst) {
                return Err(ProgressReceiveError::Exchange(Box::new(io::Error::from(
                    io::ErrorKind::TimedOut,
                ))));
            }
            if absent {
                return Ok(ProgressSnapshotRead::NotRetained);
            }
            if let Some(reply) = reply {
                return Ok(reply);
            }
            Ok(ProgressSnapshotRead::Complete(ProgressSnapshot {
                descriptor: remote,
                revision,
                items: values,
            }))
        })
    }
}
impl ProgressTaskAuthorityPort for Query {
    fn current_task(&self, _: &DeviceId, _: ProgressIdentity) -> Option<ProgressSourceTask> {
        panic!("query cannot read a local receive authority");
    }
    fn claim_terminal(
        &self,
        _: &DeviceId,
        _: ProgressIdentity,
        _: ProgressIdentity,
        _: ProgressItem,
    ) -> Result<ProgressSourceTask, ProgressClaimRefusal> {
        panic!("query cannot claim a notification");
    }
}
#[async_trait]
impl ProgressTerminalRoundPort for Query {
    async fn report_settled(
        &self,
        _: &DeviceId,
        _: ProgressTaskDescriptor,
        _: ProgressTerminal,
        _: &ProgressExchangeDeadline,
        _: &CancellationToken,
    ) -> Result<(), ProgressReceiveError> {
        panic!("query cannot send a terminal notice");
    }
}
#[async_trait]
impl ProgressLiveRoundPort for Query {
    async fn report_contact(
        &self,
        _: &DeviceId,
        _: ProgressTaskDescriptor,
        _: u64,
        _: &ProgressExchangeDeadline,
        _: &CancellationToken,
    ) -> Result<(), ProgressReceiveError> {
        panic!("query cannot send a contact");
    }
    async fn report_live(
        &self,
        _: &DeviceId,
        _: ProgressTaskDescriptor,
        _: ProgressActive,
        _: &ProgressExchangeDeadline,
        _: &CancellationToken,
    ) -> Result<(), ProgressReceiveError> {
        panic!("query cannot send live progress");
    }
}
struct Fixture {
    registry: ProgressProjectionRegistry,
    reconciler: Arc<TransferProgressReconciler>,
    query: Arc<Query>,
    key: ProgressProjectionKey,
    bound: Option<ProgressBound>,
    clock: Arc<Clock>,
    stop: CancellationToken,
    worker: JoinHandle<()>,
    projection_worker: JoinHandle<()>,
}
impl Fixture {
    async fn new(values: Vec<ProgressItemState>, registered: bool) -> Self {
        let key = key(1, 1);
        let clock = Arc::new(Clock(AtomicU64::new(0)));
        let (registry, projection_worker) = ProgressProjectionRegistry::new(clock.clone());
        registry.accept_intent(key, vec![item(0), item(1)]).unwrap();
        let bound = if registered {
            Some(
                registry
                    .register(&key.peer, 1, key.source, &mut Current(descriptor(key, 2)))
                    .await
                    .unwrap(),
            )
        } else {
            None
        };
        let query = Arc::new(Query {
            entered: Notify::new(),
            release: Semaphore::new(0),
            values: Mutex::new(values),
            revision: AtomicU64::new(1),
            fail: AtomicBool::new(false),
            remote: Mutex::new(descriptor(key, 2)),
            absent: AtomicBool::new(false),
            reply: Mutex::new(None),
        });
        let (service, worker) = ProgressExchangeService::new(
            clock.clone(),
            query.clone(),
            query.clone(),
            query.clone(),
            query.clone(),
        );
        let reconciler = Arc::new(TransferProgressReconciler::new(registry.clone(), service));
        let stop = CancellationToken::new();
        let worker = tokio::spawn(worker.run(stop.clone()));
        let projection_worker = tokio::spawn(projection_worker.run(stop.clone()));
        Self {
            registry,
            reconciler,
            query,
            key,
            bound,
            clock,
            stop,
            worker,
            projection_worker,
        }
    }
    async fn refresh(&self) -> crate::transfer::progress::ProgressQueryResult {
        self.query.release.add_permits(1);
        timeout(
            Duration::from_secs(2),
            self.reconciler.refresh(self.key, &CancellationToken::new()),
        )
        .await
        .unwrap()
    }
    async fn shutdown(self) {
        self.registry.close();
        self.stop.cancel();
        timeout(Duration::from_secs(2), self.worker)
            .await
            .unwrap()
            .unwrap();
        timeout(Duration::from_secs(2), self.projection_worker)
            .await
            .unwrap()
            .unwrap();
    }
}
fn terminal(index: u16) -> ProgressItemState {
    ProgressItemState::Terminal(ProgressTerminal {
        item: item(index),
        round: 0,
        sequence: 4,
        result_revision: 1,
        bytes: 10,
        total: Some(10),
        outcome: ProgressTerminalOutcome::Completed,
    })
}
fn active(index: u16, sequence: u64, bytes: u64) -> ProgressItemState {
    ProgressItemState::Active(ProgressActive {
        item: item(index),
        round: 0,
        sequence,
        bytes,
        total: Some(10),
        phase: ProgressPhase::Transferring,
    })
}
#[tokio::test]
async fn complete_reconciliation_recovers_lost_events_and_publishes_one_complete_snapshot() {
    let fixture = Fixture::new(vec![terminal(0), terminal(1)], true).await;
    let mut events = fixture.registry.subscribe();
    assert!(fixture
        .registry
        .snapshot(fixture.key)
        .unwrap()
        .items
        .iter()
        .all(|(_, state)| state.is_none()));
    assert!(fixture.refresh().await.is_ok());
    let recovered = fixture.registry.snapshot(fixture.key).unwrap();
    assert_eq!(recovered.summary.counts.completed, 2);
    assert_eq!(recovered.freshness.unwrap(), ProgressFreshnessView::Settled);
    assert!(events.try_recv().is_ok());
    assert!(events.try_recv().is_err());
    let version = recovered.version;
    assert!(fixture.refresh().await.is_ok());
    assert_eq!(
        fixture.registry.snapshot(fixture.key).unwrap().version,
        version
    );
    assert!(events.try_recv().is_err());
    fixture.shutdown().await;
}
#[tokio::test]
async fn conflicting_later_item_rejects_whole_snapshot_without_adopting_earlier_missing_item() {
    let mut conflict = terminal(1);
    if let ProgressItemState::Terminal(value) = &mut conflict {
        value.outcome = ProgressTerminalOutcome::Failed(ProgressFailureReason::Unknown);
    }
    let fixture = Fixture::new(vec![terminal(0), conflict], true).await;
    let ProgressItemState::Terminal(known) = terminal(1) else {
        unreachable!();
    };
    fixture
        .registry
        .adopt_terminal(
            &fixture.key.peer,
            1,
            ProgressTerminalNotice {
                binding: fixture.bound.unwrap().binding,
                source: fixture.key.source,
                attempt: fixture.bound.unwrap().descriptor.attempt,
                revision: fixture.bound.unwrap().descriptor.revision,
                result: known,
            },
        )
        .await
        .unwrap();
    let before = fixture.registry.snapshot(fixture.key).unwrap();
    let result = fixture.refresh().await;
    assert!(matches!(
        result.as_ref(),
        Err(ProgressReceiveError::Snapshot(
            ProgressSnapshotRefusal::StateConflict
        ))
    ));
    assert!(result.as_ref().as_ref().err().unwrap().source().is_some());
    let after = fixture.registry.snapshot(fixture.key).unwrap();
    assert_eq!(before.version, after.version);
    assert_eq!(before.items, after.items);
    assert!(after.items[0].1.is_none());
    fixture.shutdown().await;
}
#[tokio::test]
async fn delayed_query_cannot_repopulate_replaced_removed_or_closed_intent() {
    for action in 0..3 {
        let fixture = Fixture::new(vec![terminal(0), terminal(1)], true).await;
        let owner = fixture.reconciler.clone();
        let key = fixture.key;
        let querying =
            tokio::spawn(async move { owner.refresh(key, &CancellationToken::new()).await });
        timeout(Duration::from_secs(2), fixture.query.entered.notified())
            .await
            .unwrap();
        if action == 2 {
            fixture.registry.close();
        } else {
            fixture.registry.remove(key);
            if action == 0 {
                fixture
                    .registry
                    .accept_intent(key, vec![item(0), item(1)])
                    .unwrap();
            }
        }
        fixture.query.release.add_permits(1);
        let result = timeout(Duration::from_secs(2), querying)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(result.as_ref(), Err(ProgressReceiveError::Stale)));
        if action == 0 {
            assert!(fixture
                .registry
                .snapshot(key)
                .unwrap()
                .items
                .iter()
                .all(|(_, value)| value.is_none()));
        } else {
            assert!(fixture.registry.snapshot(key).is_none());
        }
        fixture.shutdown().await;
    }
}
#[tokio::test]
async fn repeated_snapshot_does_not_extend_contact_and_same_revision_cannot_change_its_content() {
    let fixture = Fixture::new(vec![active(0, 1, 1), active(1, 1, 1)], true).await;
    assert!(fixture.refresh().await.is_ok());
    fixture.clock.0.store(60, Ordering::SeqCst);
    assert!(fixture.refresh().await.is_ok());
    assert_eq!(
        fixture
            .registry
            .snapshot(fixture.key)
            .unwrap()
            .freshness
            .unwrap(),
        ProgressFreshnessView::Unconfirmed
    );
    fixture.query.values.lock().unwrap()[0] = active(0, 2, 2);
    let before = fixture.registry.snapshot(fixture.key).unwrap();
    let result = fixture.refresh().await;
    assert!(matches!(
        result.as_ref(),
        Err(ProgressReceiveError::Snapshot(
            ProgressSnapshotRefusal::StateConflict
        ))
    ));
    assert_eq!(
        fixture.registry.snapshot(fixture.key).unwrap().items,
        before.items
    );
    fixture.shutdown().await;
}
#[tokio::test]
async fn failed_reconciliation_keeps_typed_source_and_never_invents_missing_data_or_downloads() {
    let fixture = Fixture::new(vec![terminal(0), terminal(1)], true).await;
    fixture.query.fail.store(true, Ordering::SeqCst);
    let before = fixture.registry.snapshot(fixture.key).unwrap();
    let result = fixture.refresh().await;
    let error = result.as_ref().as_ref().err().unwrap();
    assert!(matches!(error, ProgressReceiveError::Exchange(_)));
    assert_eq!(
        error
            .source()
            .unwrap()
            .downcast_ref::<io::Error>()
            .unwrap()
            .kind(),
        io::ErrorKind::TimedOut
    );
    assert_eq!(
        fixture.registry.snapshot(fixture.key).unwrap().items,
        before.items
    );
    fixture.shutdown().await;
}

#[tokio::test]
async fn new_same_key_intent_never_shares_old_inflight_query_even_when_remote_descriptor_matches() {
    for registered in [true, false] {
        let fixture = Fixture::new(vec![active(0, 1, 1), active(1, 1, 1)], registered).await;
        let owner = fixture.reconciler.clone();
        let key = fixture.key;
        let old = tokio::spawn(async move { owner.refresh(key, &CancellationToken::new()).await });
        timeout(Duration::from_secs(2), fixture.query.entered.notified())
            .await
            .unwrap();
        fixture.registry.remove(key);
        fixture
            .registry
            .accept_intent(key, vec![item(0), item(1)])
            .unwrap();
        if registered {
            fixture
                .registry
                .register(&key.peer, 1, key.source, &mut Current(descriptor(key, 2)))
                .await
                .unwrap();
        }
        *fixture.query.values.lock().unwrap() = vec![terminal(0), terminal(1)];
        let cancel = CancellationToken::new();
        let mut new = Box::pin(fixture.reconciler.refresh(key, &cancel));
        assert!(poll!(new.as_mut()).is_pending());
        fixture.query.release.add_permits(1);
        let old = timeout(Duration::from_secs(2), old).await.unwrap().unwrap();
        assert!(matches!(old.as_ref(), Err(ProgressReceiveError::Stale)));
        timeout(Duration::from_secs(2), fixture.query.entered.notified())
            .await
            .expect("new intent must start a new actual query");
        fixture.query.release.add_permits(1);
        assert!(timeout(Duration::from_secs(2), new).await.unwrap().is_ok());
        assert_eq!(
            fixture
                .registry
                .snapshot(key)
                .unwrap()
                .summary
                .counts
                .completed,
            2
        );
        fixture.shutdown().await;
    }
}

#[tokio::test]
async fn initial_query_discovers_actual_scope_without_granting_a_notification_binding() {
    let fixture = Fixture::new(vec![terminal(0), terminal(1)], false).await;
    let initial = fixture.registry.snapshot(fixture.key).unwrap();
    assert!(initial.bound.is_none() && initial.task.is_none());
    assert!(fixture.refresh().await.is_ok());
    let verified = fixture.registry.snapshot(fixture.key).unwrap();
    assert!(verified.bound.is_none());
    assert!(verified.task == Some(descriptor(fixture.key, 2)));
    assert_eq!(verified.summary.counts.completed, 2);
    let ProgressItemState::Terminal(result) = terminal(0) else {
        unreachable!();
    };
    assert!(matches!(
        fixture
            .registry
            .adopt_terminal(
                &fixture.key.peer,
                1,
                ProgressTerminalNotice {
                    binding: id(999),
                    source: fixture.key.source,
                    attempt: descriptor(fixture.key, 2).attempt,
                    revision: 1,
                    result,
                }
            )
            .await,
        Err(ProgressReceiveError::Stale)
    ));
    fixture
        .registry
        .register(
            &fixture.key.peer,
            1,
            fixture.key.source,
            &mut Current(descriptor(fixture.key, 2)),
        )
        .await
        .unwrap();
    let registered = fixture.registry.snapshot(fixture.key).unwrap();
    assert!(registered.bound.is_some());
    assert_eq!(registered.items, verified.items);
    assert_eq!(registered.summary.counts.completed, 2);
    fixture.shutdown().await;
}
#[tokio::test]
async fn initial_missing_query_keeps_the_original_waiting_intent_without_inventing_a_task() {
    let fixture = Fixture::new(vec![terminal(0), terminal(1)], false).await;
    fixture.query.absent.store(true, Ordering::SeqCst);
    assert!(matches!(
        fixture.refresh().await.as_ref(),
        Ok(ProgressSnapshotRead::NotRetained)
    ));
    let current = fixture.registry.snapshot(fixture.key).unwrap();
    assert!(current.task.is_none() && current.bound.is_none());
    assert!(current.items.iter().all(|(_, item)| item.is_none()));
    assert_eq!(current.freshness.unwrap(), ProgressFreshnessView::Waiting);
    fixture.shutdown().await;
}
#[tokio::test]
async fn initial_query_cannot_replace_a_different_scope_registered_while_it_was_waiting() {
    for same in [true, false] {
        let fixture = Fixture::new(vec![terminal(0), terminal(1)], false).await;
        let owner = fixture.reconciler.clone();
        let key = fixture.key;
        let querying =
            tokio::spawn(async move { owner.refresh(key, &CancellationToken::new()).await });
        timeout(Duration::from_secs(2), fixture.query.entered.notified())
            .await
            .unwrap();
        let current = if same {
            descriptor(key, 2)
        } else {
            ProgressTaskDescriptor {
                attempt: id(1011),
                revision: 2,
                ..descriptor(key, 2)
            }
        };
        fixture
            .registry
            .register(&key.peer, 1, key.source, &mut Current(current))
            .await
            .unwrap();
        fixture.query.release.add_permits(1);
        let reply = timeout(Duration::from_secs(2), querying)
            .await
            .unwrap()
            .unwrap();
        let snapshot = fixture.registry.snapshot(key).unwrap();
        assert!(snapshot.task == Some(current));
        if same {
            assert!(reply.is_ok());
            assert_eq!(snapshot.summary.counts.completed, 2);
        } else {
            assert!(matches!(reply.as_ref(), Err(ProgressReceiveError::Stale)));
            assert!(snapshot.items.iter().all(|(_, value)| value.is_none()));
        }
        fixture.shutdown().await;
    }
}

fn non_snapshot_reply(index: usize, key: ProgressProjectionKey) -> ProgressSnapshotRead {
    match index {
        0 => ProgressSnapshotRead::NotRetained,
        1 => ProgressSnapshotRead::Unsupported,
        2 => ProgressSnapshotRead::AccessDenied,
        3 => ProgressSnapshotRead::Busy,
        4 => ProgressSnapshotRead::Superseded(ProgressTaskDescriptor {
            attempt: id(1011),
            revision: 2,
            ..descriptor(key, 2)
        }),
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn late_non_snapshot_reply_cannot_cross_a_registration_scope_change() {
    for registered in [true, false] {
        for reply in 0..5 {
            let fixture = Fixture::new(vec![terminal(0), terminal(1)], registered).await;
            *fixture.query.reply.lock().unwrap() = Some(non_snapshot_reply(reply, fixture.key));
            let owner = fixture.reconciler.clone();
            let key = fixture.key;
            let querying =
                tokio::spawn(async move { owner.refresh(key, &CancellationToken::new()).await });
            timeout(Duration::from_secs(2), fixture.query.entered.notified())
                .await
                .unwrap();
            let current = ProgressTaskDescriptor {
                attempt: id(1011),
                revision: 2,
                ..descriptor(key, 2)
            };
            fixture
                .registry
                .register(&key.peer, 1, key.source, &mut Current(current))
                .await
                .unwrap();
            let before = fixture.registry.snapshot(key).unwrap();
            fixture.query.release.add_permits(1);
            let reply = timeout(Duration::from_secs(2), querying)
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(reply.as_ref(), Err(ProgressReceiveError::Stale)));
            let after = fixture.registry.snapshot(key).unwrap();
            assert_eq!(after.version, before.version);
            assert_eq!(after.items, before.items);
            assert!(after.task == Some(current));
            fixture.shutdown().await;
        }
    }
}

#[tokio::test]
async fn unchanged_non_snapshot_replies_preserve_the_result_and_do_not_publish_facts() {
    for registered in [true, false] {
        for reply in 0..5 {
            let fixture = Fixture::new(vec![terminal(0), terminal(1)], registered).await;
            *fixture.query.reply.lock().unwrap() = Some(non_snapshot_reply(reply, fixture.key));
            let before = fixture.registry.snapshot(fixture.key).unwrap();
            let mut events = fixture.registry.subscribe();
            let response = fixture.refresh().await;
            let actual = response.as_ref().as_ref().unwrap();
            assert_eq!(
                discriminant(actual),
                discriminant(&non_snapshot_reply(reply, fixture.key))
            );
            if let ProgressSnapshotRead::Superseded(current) = actual {
                assert_eq!(current.attempt, id(1011));
                assert_eq!(current.revision, 2);
            }
            let after = fixture.registry.snapshot(fixture.key).unwrap();
            assert_eq!(after.version, before.version);
            assert_eq!(after.items, before.items);
            assert!(after.task == before.task && after.bound == before.bound);
            assert_eq!(after.freshness.unwrap(), before.freshness.unwrap());
            assert!(matches!(
                events.try_recv(),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty)
            ));
            fixture.shutdown().await;
        }
    }
}

#[tokio::test]
async fn late_non_snapshot_replies_cannot_answer_a_new_same_key_intent() {
    for reply in 0..5 {
        let fixture = Fixture::new(vec![terminal(0), terminal(1)], false).await;
        *fixture.query.reply.lock().unwrap() = Some(non_snapshot_reply(reply, fixture.key));
        let owner = fixture.reconciler.clone();
        let key = fixture.key;
        let querying =
            tokio::spawn(async move { owner.refresh(key, &CancellationToken::new()).await });
        timeout(Duration::from_secs(2), fixture.query.entered.notified())
            .await
            .unwrap();
        fixture.registry.remove(key);
        fixture
            .registry
            .accept_intent(key, vec![item(0), item(1)])
            .unwrap();
        fixture.query.release.add_permits(1);
        let response = timeout(Duration::from_secs(2), querying)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            response.as_ref(),
            Err(ProgressReceiveError::Stale)
        ));
        let current = fixture.registry.snapshot(key).unwrap();
        assert!(current.task.is_none() && current.bound.is_none());
        assert!(current.items.iter().all(|(_, state)| state.is_none()));
        fixture.shutdown().await;
    }
}
