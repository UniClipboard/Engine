use super::super::super::ProgressQueryResult;
use super::UnusedLive;
use super::{authority, id, peer, submit, Clock};
use super::{
    CancellationToken, DeviceId, Ordering, ProgressExchangeDeadline, ProgressExchangeService,
    ProgressReceiveError, ProgressSnapshotRead, ProgressSnapshotRoundPort, ProgressSnapshotTarget,
    ProgressSourceTask, ProgressTaskDescriptor, ProgressTerminal, ProgressTerminalRoundPort,
};
use super::{ElapsedClockError, ElapsedClockPort, Sample};
use crate::connectivity::ConnectivityError;
use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::io;
use std::sync::atomic::AtomicU64;
use std::sync::{atomic::AtomicBool, Condvar};
use std::sync::{atomic::AtomicUsize, Arc, Mutex};
use tokio::sync::Notify;
use tokio::sync::{mpsc, Semaphore};
use tokio::time::{timeout, Duration};
use uc_core::error_class::ErrorClass;
use uc_core::file_transfer::{ProgressIdentity, ProgressItem, ProgressItemKind};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Kind {
    Terminal,
    Snapshot,
}
struct Held {
    events: mpsc::Sender<(DeviceId, Kind)>,
    gates: HashMap<DeviceId, Arc<Semaphore>>,
    active: Mutex<HashSet<DeviceId>>,
    peak: AtomicUsize,
    tokens: Mutex<HashMap<(DeviceId, Kind), CancellationToken>>,
}
impl Held {
    async fn enter(&self, peer: DeviceId, kind: Kind, cancel: CancellationToken) {
        {
            let mut active = self.active.lock().unwrap();
            assert!(active.insert(peer));
            self.peak.fetch_max(active.len(), Ordering::SeqCst);
            assert!(active.len() <= 4);
        }
        self.tokens.lock().unwrap().insert((peer, kind), cancel);
        self.events.try_send((peer, kind)).unwrap();
        // 已开始的不可安全中断本机能力：收到取消后仍等待实际完成。
        self.gates[&peer].acquire().await.unwrap().forget();
        assert!(self.active.lock().unwrap().remove(&peer));
    }
}
#[async_trait]
impl ProgressTerminalRoundPort for Held {
    async fn report_settled(
        &self,
        peer: &DeviceId,
        _: ProgressTaskDescriptor,
        _: ProgressTerminal,
        _: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<(), ProgressReceiveError> {
        self.enter(*peer, Kind::Terminal, cancel.clone()).await;
        Ok(())
    }
}
#[async_trait]
impl ProgressSnapshotRoundPort for Held {
    async fn read_snapshot(
        &self,
        peer: &DeviceId,
        _: ProgressSnapshotTarget,
        _: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<ProgressSnapshotRead, ProgressReceiveError> {
        self.enter(*peer, Kind::Snapshot, cancel.clone()).await;
        Ok(ProgressSnapshotRead::NotRetained)
    }
}
fn held() -> (Arc<Held>, mpsc::Receiver<(DeviceId, Kind)>) {
    let (events, rx) = mpsc::channel(128);
    (
        Arc::new(Held {
            events,
            gates: (1..=12)
                .map(|p| (peer(p), Arc::new(Semaphore::new(0))))
                .collect(),
            active: Mutex::new(HashSet::new()),
            peak: AtomicUsize::new(0),
            tokens: Mutex::new(HashMap::new()),
        }),
        rx,
    )
}
fn task(p: u8, n: u16) -> ProgressSourceTask {
    let mut source = [p; 16];
    source[0..2].copy_from_slice(&n.to_be_bytes());
    ProgressSourceTask {
        notification_closed: Default::default(),
        descriptor: ProgressTaskDescriptor {
            producer: id(90),
            revision: 1,
            source: ProgressIdentity::new(source),
            attempt: id(99),
            item_count: 1,
        },
        items: Arc::from([ProgressItem {
            kind: ProgressItemKind::File,
            index: 0,
        }]),
    }
}
async fn event(rx: &mut mpsc::Receiver<(DeviceId, Kind)>) -> (DeviceId, Kind) {
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
fn query(
    service: &ProgressExchangeService,
    p: u8,
    n: u16,
    cancel: CancellationToken,
    started: Arc<AtomicUsize>,
) -> tokio::task::JoinHandle<ProgressQueryResult> {
    let service = service.clone();
    tokio::spawn(async move {
        started.fetch_add(1, Ordering::SeqCst);
        service
            .read_snapshot(peer(p), task(p, n).into(), id(p), &cancel)
            .await
    })
}
#[tokio::test]
async fn mixed_actual_work_shares_four_slots_duplicates_share_one_read_and_cancellation_is_local() {
    let (round, mut rx) = held();
    let clock = Arc::new(Clock(AtomicU64::new(0)));
    let (service, worker) = ProgressExchangeService::new(
        clock,
        authority(&[(1, 1), (2, 1), (5, 1)]),
        round.clone(),
        round.clone(),
        Arc::new(UnusedLive),
    );
    submit(&service, 1, 0);
    submit(&service, 2, 0);
    let started = Arc::new(AtomicUsize::new(0));
    let cancel3 = CancellationToken::new();
    let cancel4 = CancellationToken::new();
    let first = query(&service, 3, 0, cancel3.clone(), started.clone());
    let fourth = query(&service, 4, 0, cancel4.clone(), started.clone());
    let mut duplicates = Vec::new();
    for _ in 0..15 {
        duplicates.push(query(
            &service,
            3,
            0,
            CancellationToken::new(),
            started.clone(),
        ));
    }
    until(|| started.load(Ordering::SeqCst) == 17).await;
    let stop = CancellationToken::new();
    let running = tokio::spawn(worker.run(stop.clone()));
    let mut seen = HashSet::new();
    for _ in 0..4 {
        seen.insert(event(&mut rx).await);
    }
    assert_eq!(
        seen,
        HashSet::from([
            (peer(1), Kind::Terminal),
            (peer(2), Kind::Terminal),
            (peer(3), Kind::Snapshot),
            (peer(4), Kind::Snapshot)
        ])
    );
    {
        let state = service.inner.state.lock().unwrap();
        assert_eq!(state.active.len(), 4);
        assert_eq!(state.queue.active(), 4);
        assert_eq!(state.queries.len(), 2);
    }
    let overflow = timeout(
        Duration::from_secs(2),
        service.read_snapshot(peer(3), task(3, 0).into(), id(3), &CancellationToken::new()),
    )
    .await
    .unwrap();
    assert!(matches!(&*overflow, Ok(ProgressSnapshotRead::Busy)));
    cancel3.cancel();
    assert!(matches!(&*first.await.unwrap(), Err(_)));
    assert!(!round.tokens.lock().unwrap()[&(peer(3), Kind::Snapshot)].is_cancelled());
    submit(&service, 5, 0);
    assert!(timeout(Duration::from_millis(30), rx.recv()).await.is_err());
    round.gates[&peer(3)].add_permits(1);
    let mut result = None;
    for waiting in duplicates {
        let reply = waiting.await.unwrap();
        assert!(matches!(&*reply, Ok(ProgressSnapshotRead::NotRetained)));
        if let Some(previous) = result.as_ref() {
            assert!(Arc::ptr_eq(previous, &reply));
        } else {
            result = Some(reply);
        }
    }
    assert_eq!(event(&mut rx).await, (peer(5), Kind::Terminal));
    cancel4.cancel();
    assert!(matches!(&*fourth.await.unwrap(), Err(_)));
    until(|| round.tokens.lock().unwrap()[&(peer(4), Kind::Snapshot)].is_cancelled()).await;
    assert_eq!(service.inner.state.lock().unwrap().active.len(), 4);
    stop.cancel();
    until(|| service.inner.state.lock().unwrap().closed).await;
    assert!(!running.is_finished());
    for p in [1, 2, 4, 5] {
        round.gates[&peer(p)].add_permits(1);
    }
    timeout(Duration::from_secs(2), running)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(round.peak.load(Ordering::SeqCst), 4);
}
#[tokio::test]
async fn queries_use_exactly_the_existing_global_and_peer_waiting_capacity_without_a_second_queue()
{
    let (round, mut rx) = held();
    let clock = Arc::new(Clock(AtomicU64::new(0)));
    let (service, worker) = ProgressExchangeService::new(
        clock,
        authority(&[(1, 1), (2, 1), (3, 1), (4, 1), (10, 1)]),
        round.clone(),
        round.clone(),
        Arc::new(UnusedLive),
    );
    for p in 1..=4 {
        submit(&service, p, 0);
    }
    let stop = CancellationToken::new();
    let running = tokio::spawn(worker.run(stop.clone()));
    for _ in 0..4 {
        event(&mut rx).await;
    }
    let started = Arc::new(AtomicUsize::new(0));
    let mut pending = Vec::new();
    for n in 0..16 {
        pending.push(query(
            &service,
            5,
            n,
            CancellationToken::new(),
            started.clone(),
        ));
    }
    until(|| started.load(Ordering::SeqCst) == 16).await;
    assert!(matches!(
        &*service
            .read_snapshot(
                peer(5),
                task(5, 16).into(),
                id(5),
                &CancellationToken::new()
            )
            .await,
        Ok(ProgressSnapshotRead::Busy)
    ));
    for p in 6..=8 {
        for n in 0..16 {
            pending.push(query(
                &service,
                p,
                n,
                CancellationToken::new(),
                started.clone(),
            ));
        }
    }
    until(|| started.load(Ordering::SeqCst) == 64).await;
    assert!(matches!(
        &*service
            .read_snapshot(peer(9), task(9, 0).into(), id(9), &CancellationToken::new())
            .await,
        Ok(ProgressSnapshotRead::Busy)
    ));
    submit(&service, 10, 0);
    {
        let state = service.inner.state.lock().unwrap();
        assert_eq!(state.queue.queued(), 64);
        assert_eq!(state.queries.len(), 64);
        assert_eq!(state.active.len(), 4);
    }
    assert!(timeout(Duration::from_millis(30), rx.recv()).await.is_err());
    stop.cancel();
    for task in pending {
        assert!(matches!(&*task.await.unwrap(), Err(_)));
    }
    assert!(!running.is_finished());
    for p in 1..=4 {
        round.gates[&peer(p)].add_permits(1);
    }
    timeout(Duration::from_secs(2), running)
        .await
        .unwrap()
        .unwrap();
    assert!(rx.try_recv().is_err());
    let state = service.inner.state.lock().unwrap();
    assert_eq!(state.queue.queued(), 0);
    assert_eq!(state.queue.active(), 0);
    assert_eq!(state.queries.len(), 0);
}
#[tokio::test]
async fn duplicate_query_does_not_restart_five_seconds_while_waiting_for_shared_work_capacity() {
    let (round, mut rx) = held();
    let clock = Arc::new(Clock(AtomicU64::new(0)));
    let (service, worker) = ProgressExchangeService::new(
        clock.clone(),
        authority(&[(1, 1), (2, 1), (3, 1), (4, 1)]),
        round.clone(),
        round.clone(),
        Arc::new(UnusedLive),
    );
    for p in 1..=4 {
        submit(&service, p, 0);
    }
    let stop = CancellationToken::new();
    let running = tokio::spawn(worker.run(stop.clone()));
    for _ in 0..4 {
        event(&mut rx).await;
    }
    let started = Arc::new(AtomicUsize::new(0));
    let first = query(&service, 5, 0, CancellationToken::new(), started.clone());
    until(|| started.load(Ordering::SeqCst) == 1).await;
    clock.0.store(4, Ordering::SeqCst);
    let second = query(&service, 5, 0, CancellationToken::new(), started.clone());
    until(|| started.load(Ordering::SeqCst) == 2).await;
    clock.0.store(6, Ordering::SeqCst);
    service.inner.wake.notify_one();
    let a = timeout(Duration::from_secs(2), first)
        .await
        .unwrap()
        .unwrap();
    let b = second.await.unwrap();
    assert!(matches!(
        &*b,
        Err(ProgressReceiveError::Connection(
            ConnectivityError::DeadlineExpired
        ))
    ));
    assert!(matches!(
        &*a,
        Err(ProgressReceiveError::Connection(
            ConnectivityError::DeadlineExpired
        ))
    ));
    {
        let state = service.inner.state.lock().unwrap();
        assert_eq!(state.queue.queued(), 0);
        assert_eq!(state.queries.len(), 0);
        assert_eq!(state.active.len(), 4);
    }
    assert!(rx.try_recv().is_err());
    stop.cancel();
    for p in 1..=4 {
        round.gates[&peer(p)].add_permits(1);
    }
    timeout(Duration::from_secs(2), running)
        .await
        .unwrap()
        .unwrap();
}
#[tokio::test]
async fn clock_error_keeps_its_source_and_active_query_cannot_be_revived_or_publish_late_success() {
    let (round, mut rx) = held();
    let clock = Arc::new(Clock(AtomicU64::new(0)));
    let (service, worker) = ProgressExchangeService::new(
        clock.clone(),
        authority(&[]),
        round.clone(),
        round.clone(),
        Arc::new(UnusedLive),
    );
    let started = Arc::new(AtomicUsize::new(0));
    let first = query(&service, 1, 0, CancellationToken::new(), started.clone());
    let stop = CancellationToken::new();
    let running = tokio::spawn(worker.run(stop.clone()));
    assert_eq!(event(&mut rx).await, (peer(1), Kind::Snapshot));
    clock.0.store(u64::MAX, Ordering::SeqCst);
    service.inner.wake.notify_one();
    let failed = timeout(Duration::from_secs(2), first)
        .await
        .unwrap()
        .unwrap();
    let Err(error) = &*failed else {
        panic!("clock failure cannot yield complete query");
    };
    assert_eq!(error.class(), "elapsed_clock_unavailable");
    let mut cause = error.source();
    let mut found = false;
    while let Some(source) = cause {
        found |= source.is::<io::Error>();
        cause = source.source();
    }
    assert!(found);
    clock.0.store(1, Ordering::SeqCst);
    let duplicate = service
        .read_snapshot(peer(1), task(1, 0).into(), id(1), &CancellationToken::new())
        .await;
    assert!(matches!(&*duplicate, Err(ProgressReceiveError::Clock(_))));
    assert_eq!(service.inner.state.lock().unwrap().active.len(), 1);
    stop.cancel();
    assert!(!running.is_finished());
    round.gates[&peer(1)].add_permits(1);
    timeout(Duration::from_secs(2), running)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(&*failed, Err(_)));
    assert!(rx.try_recv().is_err());
}
#[tokio::test]
async fn queued_terminal_has_priority_over_same_peer_queries_after_actual_work_ends() {
    let (round, mut rx) = held();
    let clock = Arc::new(Clock(AtomicU64::new(0)));
    let (service, worker) = ProgressExchangeService::new(
        clock,
        authority(&[(1, 1)]),
        round.clone(),
        round.clone(),
        Arc::new(UnusedLive),
    );
    let started = Arc::new(AtomicUsize::new(0));
    let first = query(&service, 1, 0, CancellationToken::new(), started.clone());
    let stop = CancellationToken::new();
    let running = tokio::spawn(worker.run(stop.clone()));
    assert_eq!(event(&mut rx).await, (peer(1), Kind::Snapshot));
    let second = query(&service, 1, 1, CancellationToken::new(), started.clone());
    until(|| started.load(Ordering::SeqCst) == 2).await;
    submit(&service, 1, 0);
    until(|| service.inner.state.lock().unwrap().queue.queued() == 2).await;
    round.gates[&peer(1)].add_permits(1);
    assert!(matches!(
        &*first.await.unwrap(),
        Ok(ProgressSnapshotRead::NotRetained)
    ));
    assert_eq!(event(&mut rx).await, (peer(1), Kind::Terminal));
    round.gates[&peer(1)].add_permits(1);
    assert_eq!(event(&mut rx).await, (peer(1), Kind::Snapshot));
    stop.cancel();
    assert!(matches!(&*second.await.unwrap(), Err(_)));
    round.gates[&peer(1)].add_permits(1);
    timeout(Duration::from_secs(2), running)
        .await
        .unwrap()
        .unwrap();
}

struct PausedClock {
    now: AtomicU64,
    calls: AtomicUsize,
    pause: AtomicBool,
    entered: Notify,
    release: (Mutex<bool>, Condvar),
}
struct ReleaseClock(Arc<PausedClock>);
impl Drop for ReleaseClock {
    fn drop(&mut self) {
        *self.0.release.0.lock().unwrap() = true;
        self.0.release.1.notify_all();
    }
}
impl ElapsedClockPort for PausedClock {
    fn elapsed(&self) -> Result<Duration, ElapsedClockError> {
        let observed = self.now.load(Ordering::SeqCst);
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.pause.swap(false, Ordering::SeqCst) {
            self.entered.notify_one();
            let mut released = self.release.0.lock().unwrap();
            while !*released {
                released = self.release.1.wait(released).unwrap();
            }
        }
        Ok(Duration::from_secs(observed))
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn a_delayed_earlier_observer_cannot_borrow_the_later_admissions_remaining_budget() {
    let clock = Arc::new(PausedClock {
        now: AtomicU64::new(0),
        calls: AtomicUsize::new(0),
        pause: AtomicBool::new(true),
        entered: Notify::new(),
        release: (Mutex::new(false), Condvar::new()),
    });
    let _release_on_failure = ReleaseClock(clock.clone());
    let (round, mut rx) = held();
    let (service, worker) = ProgressExchangeService::new(
        clock.clone(),
        authority(&[]),
        round.clone(),
        round.clone(),
        Arc::new(UnusedLive),
    );
    let original = {
        let service = service.clone();
        tokio::spawn(async move {
            service
                .read_snapshot(peer(1), task(1, 0).into(), id(1), &CancellationToken::new())
                .await
        })
    };
    clock.entered.notified().await;
    // 受控交错：较早调用已采样0但尚未取得状态锁，较后请求按2先完成准入。
    let mut newer = match service.inner.state.lock().unwrap().query(
        peer(1),
        task(1, 0).into(),
        Sample {
            now: Duration::from_secs(2),
            epoch: 0,
        },
        id(1),
    ) {
        Ok(receive) => receive,
        Err(_) => panic!("valid later admission"),
    };
    clock.now.store(2, Ordering::SeqCst);
    *clock.release.0.lock().unwrap() = true;
    clock.release.1.notify_all();
    until(|| clock.calls.load(Ordering::SeqCst) >= 3).await;
    let stop = CancellationToken::new();
    let running = tokio::spawn(worker.run(stop.clone()));
    assert_eq!(event(&mut rx).await, (peer(1), Kind::Snapshot));
    clock.now.store(6, Ordering::SeqCst);
    service.inner.wake.notify_one();
    let expired = timeout(Duration::from_secs(2), original)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        &*expired,
        Err(ProgressReceiveError::Connection(
            ConnectivityError::DeadlineExpired
        ))
    ));
    assert!(!round.tokens.lock().unwrap()[&(peer(1), Kind::Snapshot)].is_cancelled());
    round.gates[&peer(1)].add_permits(1);
    timeout(Duration::from_secs(2), newer.changed())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        &**newer.borrow().as_ref().unwrap(),
        Ok(ProgressSnapshotRead::NotRetained)
    ));
    stop.cancel();
    timeout(Duration::from_secs(2), running)
        .await
        .unwrap()
        .unwrap();
    assert!(rx.try_recv().is_err());
}
