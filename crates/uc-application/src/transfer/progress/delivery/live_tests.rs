use super::*;
use crate::transfer::progress::{
    ElapsedClockError, ElapsedClockPort, ProgressClaimRefusal, ProgressExchangeDeadline,
    ProgressExchangeService, ProgressLiveRoundPort, ProgressReceiveError, ProgressSnapshotRead,
    ProgressSnapshotRoundPort, ProgressSnapshotTarget, ProgressSourceCatalog, ProgressSourceTask,
    ProgressTaskAuthorityPort, ProgressTaskDescriptor, ProgressTerminalRoundPort,
};
use async_trait::async_trait;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};
use tokio::sync::{mpsc, Semaphore};
use tokio::time::{timeout, Duration};
use tokio_util::sync::CancellationToken;
use uc_core::file_transfer::{
    ProgressActive, ProgressIdentity, ProgressItem, ProgressItemKind, ProgressPhase,
    ProgressTerminal, ProgressTerminalOutcome,
};
use uc_core::ids::{DeviceId, EntryId};

struct Millis(AtomicU64);
impl ElapsedClockPort for Millis {
    fn elapsed(&self) -> Result<Duration, ElapsedClockError> {
        Ok(Duration::from_millis(self.0.load(Ordering::SeqCst)))
    }
}
fn value(index: u16, sequence: u64) -> ProgressActive {
    ProgressActive {
        item: ProgressItem {
            kind: ProgressItemKind::File,
            index,
        },
        round: 0,
        sequence,
        bytes: sequence,
        total: None,
        phase: ProgressPhase::Transferring,
    }
}
struct Round {
    tx: mpsc::Sender<(DeviceId, Option<ProgressActive>)>,
    gate: Option<Arc<Semaphore>>,
}
#[async_trait]
impl ProgressLiveRoundPort for Round {
    async fn report_contact(
        &self,
        _: &DeviceId,
        _: ProgressTaskDescriptor,
        _: u64,
        _: &ProgressExchangeDeadline,
        _: &CancellationToken,
    ) -> Result<(), ProgressReceiveError> {
        panic!("fixture does not reach contact interval")
    }
    async fn report_live(
        &self,
        peer: &DeviceId,
        _: ProgressTaskDescriptor,
        value: ProgressActive,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<(), ProgressReceiveError> {
        deadline.check(cancel)?;
        self.tx.try_send((*peer, Some(value))).unwrap();
        if let Some(gate) = &self.gate {
            gate.acquire().await.unwrap().forget();
        }
        Ok(())
    }
}
#[async_trait]
impl ProgressTerminalRoundPort for Round {
    async fn report_settled(
        &self,
        peer: &DeviceId,
        _: ProgressTaskDescriptor,
        _: ProgressTerminal,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<(), ProgressReceiveError> {
        deadline.check(cancel)?;
        self.tx.try_send((*peer, None)).unwrap();
        Ok(())
    }
}
async fn event(
    rx: &mut mpsc::Receiver<(DeviceId, Option<ProgressActive>)>,
) -> (DeviceId, Option<ProgressActive>) {
    timeout(Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .unwrap()
}
async fn until(mut condition: impl FnMut() -> bool) {
    timeout(Duration::from_secs(2), async {
        while !condition() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn latest_values_share_one_task_rate_and_actual_completion_cannot_create_a_send_burst() {
    let clock = Arc::new(Millis(AtomicU64::new(0)));
    let (tx, mut rx) = mpsc::channel(128);
    let gate = Arc::new(Semaphore::new(0));
    let round = Arc::new(Round {
        tx,
        gate: Some(gate.clone()),
    });
    let (service, worker) = ProgressExchangeService::new(
        clock.clone(),
        authority(&[(1, 2)]),
        round.clone(),
        Arc::new(UnusedSnapshot),
        round,
    );
    service
        .submit_live(peer(1), id(1), id(99), value(0, 1))
        .unwrap();
    let stop = CancellationToken::new();
    let running = tokio::spawn(worker.run(stop.clone()));
    assert_eq!(event(&mut rx).await.1, Some(value(0, 1)));
    for seq in 2..=100 {
        service
            .submit_live(peer(1), id(1), id(99), value(0, seq))
            .unwrap();
    }
    service
        .submit_live(peer(1), id(1), id(99), value(1, 1))
        .unwrap();
    clock.0.store(5000, Ordering::SeqCst);
    service.inner.wake.notify_one();
    assert!(timeout(Duration::from_millis(30), rx.recv()).await.is_err());
    gate.add_permits(1);
    until(|| service.inner.state.lock().unwrap().active.is_empty()).await;
    clock.0.store(5199, Ordering::SeqCst);
    service.inner.wake.notify_one();
    assert!(timeout(Duration::from_millis(30), rx.recv()).await.is_err());
    clock.0.store(5200, Ordering::SeqCst);
    service.inner.wake.notify_one();
    let second = event(&mut rx).await.1.unwrap();
    assert_eq!(second, value(0, 100));
    gate.add_permits(1);
    until(|| service.inner.state.lock().unwrap().active.is_empty()).await;
    assert!(timeout(Duration::from_millis(30), rx.recv()).await.is_err());
    clock.0.store(5400, Ordering::SeqCst);
    service.inner.wake.notify_one();
    assert_eq!(event(&mut rx).await.1, Some(value(1, 1)));
    stop.cancel();
    assert!(!running.is_finished());
    gate.add_permits(1);
    timeout(Duration::from_secs(2), running)
        .await
        .unwrap()
        .unwrap();
}
#[tokio::test]
async fn terminal_removes_pending_activity_and_waits_for_actual_live_work_without_rate_delay() {
    let clock = Arc::new(Millis(AtomicU64::new(0)));
    let (tx, mut rx) = mpsc::channel(128);
    let gate = Arc::new(Semaphore::new(0));
    let round = Arc::new(Round {
        tx,
        gate: Some(gate.clone()),
    });
    let (service, worker) = ProgressExchangeService::new(
        clock,
        authority(&[(1, 1)]),
        round.clone(),
        Arc::new(UnusedSnapshot),
        round,
    );
    service
        .submit_live(peer(1), id(1), id(99), value(0, 1))
        .unwrap();
    let stop = CancellationToken::new();
    let running = tokio::spawn(worker.run(stop.clone()));
    event(&mut rx).await;
    service
        .submit_live(peer(1), id(1), id(99), value(0, 2))
        .unwrap();
    let result = ProgressTerminal {
        item: value(0, 1).item,
        round: 0,
        sequence: 1,
        result_revision: 1,
        bytes: 2,
        total: Some(2),
        outcome: ProgressTerminalOutcome::Completed,
    };
    service
        .submit_settled(peer(1), id(1), id(99), result)
        .unwrap();
    assert_eq!(service.inner.state.lock().unwrap().live.len(), 0);
    assert_eq!(
        service
            .submit_live(peer(1), id(1), id(99), value(0, 3))
            .unwrap(),
        ProgressSubmission::Refused(ProgressClaimRefusal::AlreadyObserved)
    );
    assert!(timeout(Duration::from_millis(30), rx.recv()).await.is_err());
    assert_eq!(service.inner.state.lock().unwrap().active.len(), 1);
    gate.add_permits(1);
    assert_eq!(event(&mut rx).await, (peer(1), None));
    assert!(timeout(Duration::from_millis(30), rx.recv()).await.is_err());
    stop.cancel();
    timeout(Duration::from_secs(2), running)
        .await
        .unwrap()
        .unwrap();
}
struct Many(Mutex<ProgressSourceCatalog>);
impl ProgressTaskAuthorityPort for Many {
    fn current_task(
        &self,
        peer: &DeviceId,
        source: ProgressIdentity,
    ) -> Option<ProgressSourceTask> {
        self.0.lock().unwrap().current(peer, source)
    }
    fn claim_terminal(
        &self,
        peer: &DeviceId,
        source: ProgressIdentity,
        attempt: ProgressIdentity,
        item: ProgressItem,
    ) -> Result<ProgressSourceTask, ProgressClaimRefusal> {
        self.0
            .lock()
            .unwrap()
            .claim_terminal(peer, source, attempt, item)
    }
}
fn source(peer: u8, n: u8) -> ProgressIdentity {
    let mut bytes = [peer; 16];
    bytes[0] = n;
    ProgressIdentity::new(bytes)
}
#[test]
fn maximum_source_plans_keep_independent_latest_capacity_and_bounded_nonresetting_history() {
    let authority = Arc::new(Many(Mutex::new(ProgressSourceCatalog::default())));
    let plan: Vec<_> = (0..1024).map(|index| value(index, 1).item).collect();
    for p in 1..=4 {
        for n in 0..64 {
            authority.0.lock().unwrap().accept(
                &EntryId::from(format!("local-{p}-{n}")),
                peer(p),
                source(p, n),
                id(99),
                plan.clone(),
            );
        }
    }
    let (tx, _) = mpsc::channel(128);
    let round = Arc::new(Round { tx, gate: None });
    let (service, _worker) = ProgressExchangeService::new(
        Arc::new(Millis(AtomicU64::new(0))),
        authority.clone(),
        round.clone(),
        Arc::new(UnusedSnapshot),
        round,
    );
    for p in 1..=4 {
        for n in 0..64 {
            for item in 0..1024 {
                assert_eq!(
                    service
                        .submit_live(peer(p), source(p, n), id(99), value(item, 1))
                        .unwrap(),
                    ProgressSubmission::Retained
                );
            }
        }
    }
    {
        let state = service.inner.state.lock().unwrap();
        assert_eq!(state.live.len(), 256);
        assert_eq!(state.live_order.len(), 256);
        assert_eq!(state.live_tasks.len(), 256);
        assert_eq!(
            state
                .live_tasks
                .values()
                .map(|task| task.seen.len())
                .sum::<usize>(),
            256 * 1024
        );
        assert_eq!(state.entries.len(), 0);
        assert_eq!(state.queue.queued(), 0);
    }
    // 被中间缓存淘汰的项仍保留已见序号，不重新排队或淘汰其他端。
    assert_eq!(
        service
            .submit_live(peer(1), source(1, 63), id(99), value(0, 1))
            .unwrap(),
        ProgressSubmission::Unchanged
    );
    authority.0.lock().unwrap().accept(
        &EntryId::from("new-current"),
        peer(1),
        source(1, 64),
        id(99),
        plan,
    );
    assert_eq!(
        service
            .submit_live(peer(1), source(1, 64), id(99), value(0, 1))
            .unwrap(),
        ProgressSubmission::Retained
    );
    let state = service.inner.state.lock().unwrap();
    assert_eq!(state.live_tasks.len(), 256);
    assert_eq!(state.live.len(), 256);
    assert!(state
        .live_tasks
        .values()
        .all(|task| task.seen.len() <= 1024));
}

fn id(n: u8) -> ProgressIdentity {
    ProgressIdentity::new([n; 16])
}
fn peer(n: u8) -> DeviceId {
    DeviceId::new(format!("peer-{n}"))
}
fn authority(plans: &[(u8, u16)]) -> Arc<Many> {
    let mut catalog = ProgressSourceCatalog::default();
    for (p, count) in plans {
        catalog.accept(
            &EntryId::from(format!("local-{p}")),
            peer(*p),
            id(*p),
            id(99),
            (0..*count).map(|index| value(index, 1).item).collect(),
        );
    }
    Arc::new(Many(Mutex::new(catalog)))
}
struct UnusedSnapshot;
#[async_trait]
impl ProgressSnapshotRoundPort for UnusedSnapshot {
    async fn read_snapshot(
        &self,
        _: &DeviceId,
        _: ProgressSnapshotTarget,
        _: &ProgressExchangeDeadline,
        _: &CancellationToken,
    ) -> Result<ProgressSnapshotRead, ProgressReceiveError> {
        panic!("activity fixture does not query")
    }
}

#[path = "contact_tests.rs"]
mod contact_tests;

#[path = "capacity_tests.rs"]
mod capacity_tests;
