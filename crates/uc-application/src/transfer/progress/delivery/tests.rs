use super::super::ProgressTerminalDelivery;
use super::*;
use crate::connectivity::ConnectivityError;
use crate::transfer::progress::{
    ElapsedClockError, ElapsedClockPort, ProgressClaimRefusal, ProgressExchangeDeadline,
    ProgressReceiveError, ProgressSourceCatalog, ProgressSourceTask, ProgressSubmission,
    ProgressTaskAuthorityPort, ProgressTaskDescriptor, ProgressTerminalRoundPort,
};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, Semaphore};
use tokio::time::{timeout, Duration};
use uc_core::file_transfer::{
    ProgressIdentity, ProgressItem, ProgressItemKind, ProgressTerminal, ProgressTerminalOutcome,
};
use uc_core::ids::EntryId;

struct Clock(AtomicU64);
impl ElapsedClockPort for Clock {
    fn elapsed(&self) -> Result<Duration, ElapsedClockError> {
        let now = self.0.load(Ordering::SeqCst);
        if now == u64::MAX {
            Err(ElapsedClockError {
                source: Box::new(std::io::Error::other("synthetic clock failure")),
            })
        } else {
            Ok(Duration::from_secs(now))
        }
    }
}
struct Authority(Mutex<ProgressSourceCatalog>);
impl ProgressTaskAuthorityPort for Authority {
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
fn id(n: u8) -> ProgressIdentity {
    ProgressIdentity::new([n; 16])
}
fn peer(n: u8) -> DeviceId {
    DeviceId::new(format!("peer-{n}"))
}
fn item(n: u16) -> ProgressItem {
    ProgressItem {
        kind: ProgressItemKind::File,
        index: n,
    }
}
fn result(n: u16) -> ProgressTerminal {
    ProgressTerminal {
        item: item(n),
        round: 0,
        sequence: 1,
        result_revision: 1,
        bytes: 10,
        total: Some(10),
        outcome: ProgressTerminalOutcome::Completed,
    }
}
fn authority(plans: &[(u8, u16)]) -> Arc<Authority> {
    let mut catalog = ProgressSourceCatalog::default();
    for (p, count) in plans {
        catalog.accept(
            &EntryId::from(format!("local-{p}")),
            peer(*p),
            id(*p),
            id(99),
            (0..*count).map(item).collect(),
        );
    }
    Arc::new(Authority(Mutex::new(catalog)))
}
fn submit(delivery: &ProgressTerminalDelivery, p: u8, n: u16) -> ProgressSubmission {
    delivery
        .submit_settled(peer(p), id(p), id(99), result(n))
        .unwrap()
}
struct Offline(mpsc::Sender<DeviceId>);
#[async_trait]
impl ProgressTerminalRoundPort for Offline {
    async fn report_settled(
        &self,
        peer: &DeviceId,
        _: ProgressTaskDescriptor,
        _: ProgressTerminal,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<(), ProgressReceiveError> {
        deadline.check(cancel)?;
        self.0.try_send(*peer).unwrap();
        Err(ConnectivityError::Offline.into())
    }
}
struct Held {
    events: mpsc::Sender<DeviceId>,
    gates: HashMap<DeviceId, Arc<Semaphore>>,
}
#[async_trait]
impl ProgressTerminalRoundPort for Held {
    async fn report_settled(
        &self,
        peer: &DeviceId,
        _: ProgressTaskDescriptor,
        _: ProgressTerminal,
        _: &ProgressExchangeDeadline,
        _: &CancellationToken,
    ) -> Result<(), ProgressReceiveError> {
        self.events.try_send(*peer).unwrap();
        // 模拟已经开始、不能由网络取消遗弃的本机读取；必须等实际完成。
        self.gates[peer].acquire().await.unwrap().forget();
        Ok(())
    }
}

#[test]
fn thousand_item_actual_admission_is_bounded_and_evicted_results_cannot_reenter() {
    let source = authority(&[(1, 1024), (2, 1)]);
    let clock = Arc::new(Clock(AtomicU64::new(0)));
    let (tx, mut rx) = mpsc::channel(64);
    let (delivery, _worker) = ProgressTerminalDelivery::new(clock, source, Arc::new(Offline(tx)));
    assert_eq!(submit(&delivery, 2, 0), ProgressSubmission::Retained);
    for n in 0..1024 {
        assert_eq!(submit(&delivery, 1, n), ProgressSubmission::Retained);
    }
    let state = delivery.inner.state.lock().unwrap();
    assert_eq!(state.entries.len(), 65);
    assert_eq!(state.ledger.len(), 65);
    assert_eq!(state.queue.queued(), 0);
    assert_eq!(state.active.len(), 0);
    assert!(state
        .entries
        .keys()
        .filter(|key| key.peer == peer(1))
        .all(|key| key.item.index >= 960));
    drop(state);
    assert_eq!(
        submit(&delivery, 1, 0),
        ProgressSubmission::Refused(ProgressClaimRefusal::AlreadyObserved)
    );
    assert!(rx.try_recv().is_err());
}
#[test]
fn global_actual_retention_releases_oldest_without_unbounded_history() {
    let source = authority(&[(1, 64), (2, 64), (3, 64), (4, 64), (5, 64)]);
    let clock = Arc::new(Clock(AtomicU64::new(0)));
    let (tx, _) = mpsc::channel(64);
    let (delivery, _worker) = ProgressTerminalDelivery::new(clock, source, Arc::new(Offline(tx)));
    for p in 1..=5 {
        for n in 0..64 {
            submit(&delivery, p, n);
        }
    }
    let state = delivery.inner.state.lock().unwrap();
    assert_eq!(state.entries.len(), 256);
    assert_eq!(state.ledger.len(), 256);
    assert!(state.entries.keys().all(|key| key.peer != peer(1)));
    assert!(state.peers.len() <= 5);
}
#[tokio::test]
async fn actual_rounds_follow_all_eight_original_slots_and_duplicate_never_restarts() {
    let source = authority(&[(1, 1)]);
    let clock = Arc::new(Clock(AtomicU64::new(0)));
    let (tx, mut rx) = mpsc::channel(64);
    let (delivery, worker) =
        ProgressTerminalDelivery::new(clock.clone(), source, Arc::new(Offline(tx)));
    submit(&delivery, 1, 0);
    let stop = CancellationToken::new();
    let running = tokio::spawn(worker.run(stop.clone()));
    for at in [0, 2, 10, 30, 60, 120, 300, 540] {
        clock.0.store(at, Ordering::SeqCst);
        delivery.inner.wake.notify_one();
        assert_eq!(
            timeout(Duration::from_secs(2), rx.recv()).await.unwrap(),
            Some(peer(1))
        );
    }
    clock.0.store(600, Ordering::SeqCst);
    delivery.inner.wake.notify_one();
    assert_eq!(
        submit(&delivery, 1, 0),
        ProgressSubmission::Refused(ProgressClaimRefusal::AlreadyObserved)
    );
    for _ in 0..20 {
        delivery.network_recovered(None);
    }
    assert!(timeout(Duration::from_millis(30), rx.recv()).await.is_err());
    stop.cancel();
    timeout(Duration::from_secs(2), running)
        .await
        .unwrap()
        .unwrap();
}
#[tokio::test]
async fn eviction_and_close_hold_all_actual_work_slots_until_real_completion() {
    let source = authority(&[(1, 70), (2, 1), (3, 1), (4, 1), (5, 1)]);
    let clock = Arc::new(Clock(AtomicU64::new(0)));
    let (tx, mut rx) = mpsc::channel(64);
    let gates: HashMap<_, _> = (1..=5)
        .map(|p| (peer(p), Arc::new(Semaphore::new(0))))
        .collect();
    let (delivery, worker) = ProgressTerminalDelivery::new(
        clock,
        source,
        Arc::new(Held {
            events: tx,
            gates: gates.clone(),
        }),
    );
    for p in 1..=4 {
        submit(&delivery, p, 0);
    }
    let stop = CancellationToken::new();
    let running = tokio::spawn(worker.run(stop.clone()));
    for _ in 0..4 {
        timeout(Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
    }
    for n in 1..=64 {
        submit(&delivery, 1, n);
    }
    submit(&delivery, 5, 0);
    {
        let state = delivery.inner.state.lock().unwrap();
        assert_eq!(state.active.len(), 4);
        assert_eq!(state.queue.active(), 4);
    }
    assert!(timeout(Duration::from_millis(30), rx.recv()).await.is_err());
    gates[&peer(2)].add_permits(1);
    assert_eq!(
        timeout(Duration::from_secs(2), rx.recv()).await.unwrap(),
        Some(peer(5))
    );
    stop.cancel();
    timeout(Duration::from_secs(2), async {
        while !delivery.inner.state.lock().unwrap().closed {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(!running.is_finished());
    assert_eq!(submit(&delivery, 3, 0), ProgressSubmission::Closed);
    for p in [1, 3, 4, 5] {
        gates[&peer(p)].add_permits(1);
    }
    timeout(Duration::from_secs(2), running)
        .await
        .unwrap()
        .unwrap();
    let state = delivery.inner.state.lock().unwrap();
    assert_eq!(state.active.len(), 0);
    assert_eq!(state.queue.active(), 0);
    assert!(state.entries.is_empty());
}
#[tokio::test]
async fn clock_fault_retires_all_old_intents_and_recovery_cannot_readmit_them() {
    let source = authority(&[(1, 2)]);
    let clock = Arc::new(Clock(AtomicU64::new(0)));
    let (tx, mut rx) = mpsc::channel(64);
    let gates = HashMap::from([(peer(1), Arc::new(Semaphore::new(0)))]);
    let (delivery, worker) = ProgressTerminalDelivery::new(
        clock.clone(),
        source,
        Arc::new(Held {
            events: tx,
            gates: gates.clone(),
        }),
    );
    submit(&delivery, 1, 0);
    submit(&delivery, 1, 1);
    let stop = CancellationToken::new();
    let running = tokio::spawn(worker.run(stop.clone()));
    timeout(Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .unwrap();
    clock.0.store(u64::MAX, Ordering::SeqCst);
    delivery.inner.wake.notify_one();
    timeout(Duration::from_secs(2), async {
        while !delivery.inner.state.lock().unwrap().entries.is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    clock.0.store(1, Ordering::SeqCst);
    assert_eq!(
        submit(&delivery, 1, 1),
        ProgressSubmission::Refused(ProgressClaimRefusal::AlreadyObserved)
    );
    stop.cancel();
    gates[&peer(1)].add_permits(1);
    timeout(Duration::from_secs(2), running)
        .await
        .unwrap()
        .unwrap();
}

#[test]
fn stale_worker_sample_cannot_retire_a_newer_submission_or_epoch() {
    let task = ProgressTaskDescriptor {
        producer: id(1),
        revision: 1,
        source: id(2),
        attempt: id(3),
        item_count: 3,
    };
    let mut state = State::default();
    let a = Key::new(peer(1), task, result(0));
    let b = Key::new(peer(1), task, result(1));
    assert!(state.admit(
        a,
        task,
        result(0),
        Sample {
            now: Duration::from_secs(100),
            epoch: 0
        }
    ));
    assert!(state.admit(
        b,
        task,
        result(1),
        Sample {
            now: Duration::from_secs(102),
            epoch: 0
        }
    ));
    state.dispatch(Sample {
        now: Duration::from_secs(100),
        epoch: 0,
    });
    assert_eq!(state.entries.len(), 2);
    let c = Key::new(peer(2), task, result(2));
    assert!(state.admit(
        c,
        task,
        result(2),
        Sample {
            now: Duration::from_secs(1),
            epoch: 1
        }
    ));
    state.refresh(Some(Sample {
        now: Duration::from_secs(103),
        epoch: 0,
    }));
    state.retire_before(Some(1));
    assert_eq!(state.entries.len(), 1);
    assert!(state.entries.contains_key(&c));
}
#[test]
fn late_old_generated_record_cannot_escape_capacity_when_it_evicts_itself() {
    let task = ProgressTaskDescriptor {
        producer: id(1),
        revision: 1,
        source: id(2),
        attempt: id(3),
        item_count: 65,
    };
    let mut state = State::default();
    for n in 0..64 {
        assert!(state.admit(
            Key::new(peer(1), task, result(n)),
            task,
            result(n),
            Sample {
                now: Duration::from_secs(100),
                epoch: 0
            }
        ));
    }
    assert!(!state.admit(
        Key::new(peer(1), task, result(64)),
        task,
        result(64),
        Sample {
            now: Duration::from_secs(99),
            epoch: 0
        }
    ));
    assert_eq!(state.entries.len(), 64);
    assert_eq!(state.ledger.len(), 64);
}

struct UntilCancelled(mpsc::Sender<DeviceId>);
#[async_trait]
impl ProgressTerminalRoundPort for UntilCancelled {
    async fn report_settled(
        &self,
        peer: &DeviceId,
        _: ProgressTaskDescriptor,
        _: ProgressTerminal,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<(), ProgressReceiveError> {
        deadline.check(cancel)?;
        self.0.try_send(*peer).unwrap();
        cancel.cancelled().await;
        Err(ConnectivityError::Stopped.into())
    }
}
#[tokio::test]
async fn round_timeout_cancels_only_that_round_and_next_slot_keeps_original_result() {
    let clock = Arc::new(Clock(AtomicU64::new(0)));
    let (tx, mut rx) = mpsc::channel(64);
    let (delivery, worker) = ProgressTerminalDelivery::new(
        clock.clone(),
        authority(&[(1, 1)]),
        Arc::new(UntilCancelled(tx)),
    );
    submit(&delivery, 1, 0);
    let stop = CancellationToken::new();
    let running = tokio::spawn(worker.run(stop.clone()));
    assert_eq!(
        timeout(Duration::from_secs(2), rx.recv()).await.unwrap(),
        Some(peer(1))
    );
    clock.0.store(5, Ordering::SeqCst);
    delivery.inner.wake.notify_one();
    assert_eq!(
        timeout(Duration::from_secs(2), rx.recv()).await.unwrap(),
        Some(peer(1))
    );
    {
        let state = delivery.inner.state.lock().unwrap();
        assert_eq!(state.active.len(), 1);
        let entry = state.entries.values().next().unwrap();
        assert!(!entry.cancel.is_cancelled());
        assert_eq!(entry.generated, Duration::ZERO);
        assert_eq!(entry.result, result(0));
    }
    assert!(timeout(Duration::from_millis(30), rx.recv()).await.is_err());
    stop.cancel();
    timeout(Duration::from_secs(2), running)
        .await
        .unwrap()
        .unwrap();
}
#[test]
fn fault_observed_inside_one_round_invalidates_all_old_clock_leases_after_recovery() {
    let raw = Arc::new(Clock(AtomicU64::new(0)));
    let shared = Arc::new(super::super::clock::Clock::new(
        raw.clone(),
        Arc::new(tokio::sync::Notify::new()),
    ));
    let epoch = shared.sample().unwrap().epoch;
    let first = shared.lease(epoch);
    let second = shared.lease(epoch);
    raw.0.store(u64::MAX, Ordering::SeqCst);
    assert!(first.elapsed().is_err());
    raw.0.store(1, Ordering::SeqCst);
    assert!(second.elapsed().is_err());
    assert!(first.elapsed().is_err());
    let recovered = shared.sample().unwrap();
    assert_ne!(recovered.epoch, epoch);
    assert_eq!(
        shared.lease(recovered.epoch).elapsed().unwrap(),
        Duration::from_secs(1)
    );
}
#[tokio::test]
async fn many_actual_eligible_peers_keep_exchange_queue_and_live_futures_bounded() {
    let clock = Arc::new(Clock(AtomicU64::new(0)));
    let plans: Vec<_> = (1..=100).map(|p| (p, 1)).collect();
    let (tx, mut rx) = mpsc::channel(128);
    let gates: HashMap<_, _> = (1..=100)
        .map(|p| (peer(p), Arc::new(Semaphore::new(0))))
        .collect();
    let (delivery, worker) = ProgressTerminalDelivery::new(
        clock,
        authority(&plans),
        Arc::new(Held {
            events: tx,
            gates: gates.clone(),
        }),
    );
    for p in 1..=100 {
        assert_eq!(submit(&delivery, p, 0), ProgressSubmission::Retained);
    }
    let stop = CancellationToken::new();
    let running = tokio::spawn(worker.run(stop.clone()));
    for _ in 0..4 {
        timeout(Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
    }
    {
        let state = delivery.inner.state.lock().unwrap();
        assert_eq!(state.entries.len(), 100);
        assert_eq!(state.active.len(), 4);
        assert_eq!(state.queue.active(), 4);
        assert!(state.queue.queued() <= 64);
        assert_eq!(
            state
                .active
                .keys()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            4
        );
    }
    assert!(timeout(Duration::from_millis(30), rx.recv()).await.is_err());
    stop.cancel();
    for gate in gates.values() {
        gate.add_permits(1);
    }
    timeout(Duration::from_secs(2), running)
        .await
        .unwrap()
        .unwrap();
    assert!(rx.try_recv().is_err());
}
