//! 通过原worker观察实际轮次，受控port故意等待本机工作结束才返回。
use super::{authority, peer, result, submit, Clock, UnusedLive, UnusedSnapshot};
use crate::connectivity::ConnectivityError;
use crate::transfer::progress::runtime::observe_reachability;
use crate::transfer::progress::{
    ProgressExchangeDeadline, ProgressExchangeService, ProgressReceiveError,
    ProgressTaskDescriptor, ProgressTerminalRoundPort,
};
use async_trait::async_trait;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::time::{timeout, Duration};
use tokio_util::sync::CancellationToken;
use uc_core::file_transfer::ProgressTerminal;
use uc_core::ids::DeviceId;
use uc_core::ports::{PeerReachabilityChanged, ReachabilityState};

struct Call {
    at: u64,
    result: ProgressTerminal,
    cancel: CancellationToken,
    finish: oneshot::Sender<()>,
}
struct Round {
    clock: Arc<Clock>,
    started: mpsc::Sender<Call>,
}
#[async_trait]
impl ProgressTerminalRoundPort for Round {
    async fn report_settled(
        &self,
        target: &DeviceId,
        _: ProgressTaskDescriptor,
        result: ProgressTerminal,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<(), ProgressReceiveError> {
        assert_eq!(*target, peer(1));
        deadline.check(cancel)?;
        let (finish, finished) = oneshot::channel();
        self.started
            .send(Call {
                at: self.clock.0.load(Ordering::SeqCst),
                result,
                cancel: cancel.clone(),
                finish,
            })
            .await
            .unwrap();
        // 模拟已经启动且不能安全中断的本机收尾，不能收到取消就丢下future。
        finished.await.unwrap();
        Err(ConnectivityError::Offline.into())
    }
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
async fn advance(service: &ProgressExchangeService, clock: &Clock, at: u64, active: usize) {
    clock.0.store(at, Ordering::SeqCst);
    service.inner.wake.notify_one();
    until(|| {
        let state = service.inner.state.lock().unwrap();
        state
            .observed
            .is_some_and(|sample| sample.now == Duration::from_secs(at))
            && state.active.len() == active
    })
    .await;
}
async fn next(
    receiver: &mut mpsc::Receiver<Call>,
    service: &ProgressExchangeService,
    at: u64,
    deadline: u64,
) -> Call {
    let call = timeout(Duration::from_secs(2), receiver.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(call.at, at);
    assert_eq!(call.result, result(0));
    let state = service.inner.state.lock().unwrap();
    assert_eq!(state.active.len(), 1);
    assert_eq!(
        state.active.get(&peer(1)).unwrap().2,
        Duration::from_secs(deadline)
    );
    let entry = state.entries.values().next().unwrap();
    assert_eq!(entry.generated, Duration::ZERO);
    assert_eq!(entry.result, result(0));
    call
}
#[tokio::test]
async fn actual_worker_merges_recovery_and_missed_slots_without_overlap_or_budget_renewal() {
    let clock = Arc::new(Clock(AtomicU64::new(0)));
    let (started, mut calls) = mpsc::channel(8);
    let (service, worker) = ProgressExchangeService::new(
        clock.clone(),
        authority(&[(1, 1)]),
        Arc::new(Round {
            clock: clock.clone(),
            started,
        }),
        Arc::new(UnusedSnapshot),
        Arc::new(UnusedLive),
    );
    submit(&service, 1, 0);
    let stop = CancellationToken::new();
    let running = tokio::spawn(worker.run(stop.clone()));
    let first = next(&mut calls, &service, 0, 5).await;
    advance(&service, &clock, 2, 1).await;
    assert!(!first.cancel.is_cancelled());
    assert!(timeout(Duration::from_millis(30), calls.recv())
        .await
        .is_err());
    advance(&service, &clock, 5, 1).await;
    assert!(first.cancel.is_cancelled());
    assert!(timeout(Duration::from_millis(30), calls.recv())
        .await
        .is_err());
    first.finish.send(()).unwrap();
    let second = next(&mut calls, &service, 5, 10).await;
    second.finish.send(()).unwrap();
    until(|| service.inner.state.lock().unwrap().active.is_empty()).await;
    clock.0.store(6, Ordering::SeqCst);
    for _ in 0..20 {
        service.network_recovered(Some(peer(1)));
    }
    advance(&service, &clock, 6, 0).await;
    assert!(timeout(Duration::from_millis(30), calls.recv())
        .await
        .is_err());
    clock.0.store(7, Ordering::SeqCst);
    service.inner.wake.notify_one();
    let third = next(&mut calls, &service, 7, 12).await;
    third.finish.send(()).unwrap();
    until(|| service.inner.state.lock().unwrap().active.is_empty()).await;
    assert!(timeout(Duration::from_millis(30), calls.recv())
        .await
        .is_err());
    clock.0.store(130, Ordering::SeqCst);
    service.inner.wake.notify_one();
    let fourth = next(&mut calls, &service, 130, 135).await;
    fourth.finish.send(()).unwrap();
    until(|| service.inner.state.lock().unwrap().active.is_empty()).await;
    advance(&service, &clock, 131, 0).await;
    assert!(timeout(Duration::from_millis(30), calls.recv())
        .await
        .is_err());
    clock.0.store(595, Ordering::SeqCst);
    service.inner.wake.notify_one();
    let fifth = next(&mut calls, &service, 595, 600).await;
    advance(&service, &clock, 600, 1).await;
    assert!(fifth.cancel.is_cancelled());
    assert!(service.inner.state.lock().unwrap().entries.is_empty());
    for _ in 0..20 {
        service.network_recovered(None);
    }
    assert!(timeout(Duration::from_millis(30), calls.recv())
        .await
        .is_err());
    fifth.finish.send(()).unwrap();
    until(|| service.inner.state.lock().unwrap().active.is_empty()).await;
    stop.cancel();
    timeout(Duration::from_secs(2), running)
        .await
        .unwrap()
        .unwrap();
    drop(service);
    assert!(calls.recv().await.is_none());
}

#[tokio::test]
async fn production_reachability_observer_consumes_original_retry_slots_without_renewing_retention()
{
    for lagged in [false, true] {
        let clock = Arc::new(Clock(AtomicU64::new(0)));
        let (started, mut calls) = mpsc::channel(8);
        let (service, worker) = ProgressExchangeService::new(
            clock.clone(),
            authority(&[(1, 1)]),
            Arc::new(Round {
                clock: clock.clone(),
                started,
            }),
            Arc::new(UnusedSnapshot),
            Arc::new(UnusedLive),
        );
        submit(&service, 1, 0);
        let stop = CancellationToken::new();
        let running = tokio::spawn(worker.run(stop.clone()));
        next(&mut calls, &service, 0, 5)
            .await
            .finish
            .send(())
            .unwrap();
        until(|| service.inner.state.lock().unwrap().active.is_empty()).await;
        clock.0.store(2, Ordering::SeqCst);
        service.inner.wake.notify_one();
        next(&mut calls, &service, 2, 7)
            .await
            .finish
            .send(())
            .unwrap();
        until(|| service.inner.state.lock().unwrap().active.is_empty()).await;
        advance(&service, &clock, 4, 0).await;
        let (events, receiver) = broadcast::channel(if lagged { 1 } else { 32 });
        let event = |device, state| PeerReachabilityChanged {
            device_id: peer(device),
            state,
            at: chrono::Utc::now(),
        };
        if lagged {
            // 在观察者启动前溢出队列；最新离线值不能抹去一次保守核对机会。
            for _ in 0..4 {
                events.send(event(1, ReachabilityState::Offline)).unwrap();
            }
        }
        let observing = tokio::spawn(observe_reachability(
            receiver,
            service.clone(),
            stop.clone(),
        ));
        if !lagged {
            events.send(event(1, ReachabilityState::Offline)).unwrap();
            events.send(event(1, ReachabilityState::Unknown)).unwrap();
            events.send(event(2, ReachabilityState::Online)).unwrap();
            assert!(timeout(Duration::from_millis(30), calls.recv())
                .await
                .is_err());
            events.send(event(1, ReachabilityState::Online)).unwrap();
        }
        let third = next(&mut calls, &service, 4, 9).await;
        for _ in 0..20 {
            events.send(event(1, ReachabilityState::Online)).unwrap();
        }
        advance(&service, &clock, 5, 1).await;
        assert!(!third.cancel.is_cancelled());
        assert!(timeout(Duration::from_millis(30), calls.recv())
            .await
            .is_err());
        // 恢复机会不能延长原t0+600秒，更不能丢下已启动的本地结算。
        advance(&service, &clock, 600, 1).await;
        assert!(third.cancel.is_cancelled());
        assert!(service.inner.state.lock().unwrap().entries.is_empty());
        for _ in 0..20 {
            events.send(event(1, ReachabilityState::Online)).unwrap();
        }
        assert!(timeout(Duration::from_millis(30), calls.recv())
            .await
            .is_err());
        third.finish.send(()).unwrap();
        until(|| service.inner.state.lock().unwrap().active.is_empty()).await;
        stop.cancel();
        timeout(Duration::from_secs(2), observing)
            .await
            .unwrap()
            .unwrap();
        timeout(Duration::from_secs(2), running)
            .await
            .unwrap()
            .unwrap();
    }
}
