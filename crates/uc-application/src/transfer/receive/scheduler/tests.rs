use super::{DownloadPermit, DownloadScheduler, ScheduleError, ScheduleRefusal};
use futures::poll;
use std::error::Error;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;
use tokio::sync::{mpsc, Barrier};
use tokio::task::JoinSet;
use tokio::time::{advance, Instant};
use tokio_util::sync::CancellationToken;
use uc_core::ids::DeviceId;

type Waiting<'a> = Pin<Box<dyn Future<Output = Result<DownloadPermit, ScheduleError>> + Send + 'a>>;
fn waiting<'a>(
    scheduler: &'a DownloadScheduler,
    peer: &str,
    cancel: &'a CancellationToken,
) -> Waiting<'a> {
    Box::pin(scheduler.acquire(DeviceId::new(peer), cancel))
}
fn refusal(result: Result<DownloadPermit, ScheduleError>, expected: ScheduleRefusal) {
    let Err(error) = result else {
        panic!("unexpected download admission");
    };
    assert_eq!(error.0, expected);
    assert!(error.source().is_some());
}

#[tokio::test(start_paused = true)]
async fn active_limits_and_round_robin_preserve_each_peer_order() {
    let scheduler = DownloadScheduler::default();
    let cancel = CancellationToken::new();
    let a1 = waiting(&scheduler, "a", &cancel).await.expect("a1");
    let a2 = waiting(&scheduler, "a", &cancel).await.expect("a2");
    let b1 = waiting(&scheduler, "b", &cancel).await.expect("b1");
    let b2 = waiting(&scheduler, "b", &cancel).await.expect("b2");
    let mut a3 = waiting(&scheduler, "a", &cancel);
    let mut a4 = waiting(&scheduler, "a", &cancel);
    let mut b3 = waiting(&scheduler, "b", &cancel);
    let mut c1 = waiting(&scheduler, "c", &cancel);
    for task in [&mut a3, &mut a4, &mut b3, &mut c1] {
        assert!(poll!(task.as_mut()).is_pending());
    }
    drop(a1);
    let a3 = a3.await.expect("first a waiter");
    assert!(poll!(a4.as_mut()).is_pending());
    drop(a2);
    // B 已满，轮转跳过 B，C 获得下一名额，A4 不能抢占。
    let c1 = c1.await.expect("next eligible peer");
    assert!(poll!(a4.as_mut()).is_pending());
    assert!(poll!(b3.as_mut()).is_pending());
    drop(b1);
    let a4 = a4.await.expect("a4 after c1");
    drop(c1);
    let b3 = b3.await.expect("b3 next rotation");
    drop((a3, a4, b2, b3));
}

#[tokio::test(start_paused = true)]
async fn queue_limits_are_atomic_and_cancelled_waiters_leave_no_hidden_queue() {
    let scheduler = DownloadScheduler::default();
    let cancel = CancellationToken::new();
    let mut active = Vec::new();
    for peer in ["a", "a", "b", "b"] {
        active.push(waiting(&scheduler, peer, &cancel).await.expect("active"));
    }
    let mut queued = Vec::new();
    for peer in ["a", "b", "c", "d"] {
        for _ in 0..16 {
            let mut task = waiting(&scheduler, peer, &cancel);
            assert!(poll!(task.as_mut()).is_pending());
            queued.push(task);
        }
        refusal(
            waiting(&scheduler, peer, &cancel).await,
            ScheduleRefusal::Busy,
        );
    }
    refusal(
        waiting(&scheduler, "e", &cancel).await,
        ScheduleRefusal::Busy,
    );
    drop(queued.remove(0));
    let mut replacement = waiting(&scheduler, "e", &cancel);
    assert!(poll!(replacement.as_mut()).is_pending());
    refusal(
        waiting(&scheduler, "e", &cancel).await,
        ScheduleRefusal::Busy,
    );
    drop(replacement);
    drop(queued);
    drop(active);
    let fresh = waiting(&scheduler, "e", &cancel).await.expect("fresh");
    drop(fresh);
}

#[tokio::test(start_paused = true)]
async fn exact_queue_deadline_wins_over_a_simultaneous_capacity_release() {
    let scheduler = DownloadScheduler::default();
    let cancel = CancellationToken::new();
    let a1 = waiting(&scheduler, "a", &cancel).await.expect("a1");
    let a2 = waiting(&scheduler, "a", &cancel).await.expect("a2");
    let mut queued = waiting(&scheduler, "a", &cancel);
    assert!(poll!(queued.as_mut()).is_pending());
    advance(Duration::from_secs(59)).await;
    assert!(poll!(queued.as_mut()).is_pending());
    advance(Duration::from_secs(1)).await;
    drop(a1);
    refusal(queued.await, ScheduleRefusal::QueueExpired);
    drop(a2);
}

#[tokio::test(start_paused = true)]
async fn expired_unpolled_waiters_do_not_occupy_queue_capacity_or_restart_their_deadline() {
    let scheduler = DownloadScheduler::default();
    let cancel = CancellationToken::new();
    let a1 = waiting(&scheduler, "a", &cancel).await.expect("a1");
    let a2 = waiting(&scheduler, "a", &cancel).await.expect("a2");
    let mut queued = Vec::new();
    for _ in 0..16 {
        let mut task = waiting(&scheduler, "a", &cancel);
        assert!(poll!(task.as_mut()).is_pending());
        queued.push(task);
    }
    advance(Duration::from_secs(60)).await;
    let mut fresh = waiting(&scheduler, "a", &cancel);
    assert!(poll!(fresh.as_mut()).is_pending());
    for task in queued {
        refusal(task.await, ScheduleRefusal::QueueExpired);
    }
    drop(a1);
    let fresh = fresh.await.expect("fresh waiter retained its own deadline");
    drop((fresh, a2));
}

#[tokio::test(start_paused = true)]
async fn cancelled_or_dropped_handoff_releases_only_its_own_reservation() {
    let scheduler = DownloadScheduler::default();
    let live = CancellationToken::new();
    let a1 = waiting(&scheduler, "a", &live).await.expect("a1");
    let a2 = waiting(&scheduler, "a", &live).await.expect("a2");
    let cancelled = CancellationToken::new();
    let mut abandoned = waiting(&scheduler, "a", &cancelled);
    let mut following = waiting(&scheduler, "a", &live);
    assert!(poll!(abandoned.as_mut()).is_pending());
    assert!(poll!(following.as_mut()).is_pending());
    drop(a1);
    // 名额已交接但调用方尚未 poll；取消不能返回许可并启动自身工作。
    cancelled.cancel();
    refusal(abandoned.await, ScheduleRefusal::Cancelled);
    let following = following.await.expect("following waiter starts once");
    let mut abandoned = waiting(&scheduler, "a", &live);
    let mut following2 = waiting(&scheduler, "a", &live);
    assert!(poll!(abandoned.as_mut()).is_pending());
    assert!(poll!(following2.as_mut()).is_pending());
    drop(a2);
    drop(abandoned);
    let following2 = following2.await.expect("dropped handoff returned its slot");
    drop((following, following2));
}

#[tokio::test(start_paused = true)]
async fn activation_clock_is_the_actual_grant_time_not_late_poll_time() {
    let scheduler = DownloadScheduler::default();
    let cancel = CancellationToken::new();
    let a1 = waiting(&scheduler, "a", &cancel).await.expect("a1");
    let a2 = waiting(&scheduler, "a", &cancel).await.expect("a2");
    let mut queued = waiting(&scheduler, "a", &cancel);
    assert!(poll!(queued.as_mut()).is_pending());
    advance(Duration::from_secs(10)).await;
    let granted_at = Instant::now();
    drop(a1);
    advance(Duration::from_secs(55)).await;
    let queued = queued
        .await
        .expect("already activated before queue expiration");
    assert_eq!(queued.started_at, granted_at);
    drop((queued, a2));
}

#[tokio::test(start_paused = true)]
async fn close_seals_pending_and_new_intents_without_releasing_actual_work() {
    let scheduler = DownloadScheduler::default();
    let cancel = CancellationToken::new();
    let a1 = waiting(&scheduler, "a", &cancel).await.expect("a1");
    let a2 = waiting(&scheduler, "a", &cancel).await.expect("a2");
    let mut queued = waiting(&scheduler, "a", &cancel);
    assert!(matches!(poll!(queued.as_mut()), Poll::Pending));
    scheduler.close();
    scheduler.close();
    refusal(queued.await, ScheduleRefusal::Closed);
    refusal(
        waiting(&scheduler, "b", &cancel).await,
        ScheduleRefusal::Closed,
    );
    // 实际任务仍持有两份许可；关闭不会用删除注册来伪造结算。
    drop((a1, a2));
    refusal(
        waiting(&scheduler, "a", &cancel).await,
        ScheduleRefusal::Closed,
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn simultaneous_enqueue_and_cancellation_never_accept_more_than_the_waiting_capacity() {
    let scheduler = DownloadScheduler::default();
    let live = CancellationToken::new();
    let mut active = Vec::new();
    for peer in ["active-a", "active-a", "active-b", "active-b"] {
        active.push(waiting(&scheduler, peer, &live).await.expect("active"));
    }
    let cancel = CancellationToken::new();
    let barrier = Arc::new(Barrier::new(81));
    let (accepted, mut results) = mpsc::channel(80);
    let mut tasks = JoinSet::new();
    for index in 0..80 {
        let scheduler = scheduler.clone();
        let barrier = barrier.clone();
        let cancel = cancel.clone();
        let accepted = accepted.clone();
        tasks.spawn(async move {
            barrier.wait().await;
            let peer = format!("queued-{}", index % 8);
            let mut admission = waiting(&scheduler, &peer, &cancel);
            match poll!(admission.as_mut()) {
                Poll::Pending => {
                    accepted.send(true).await.expect("accepted result");
                    refusal(admission.await, ScheduleRefusal::Cancelled);
                }
                Poll::Ready(result) => {
                    refusal(result, ScheduleRefusal::Busy);
                    accepted.send(false).await.expect("busy result");
                }
            }
        });
    }
    barrier.wait().await;
    let mut queued = 0;
    for _ in 0..80 {
        queued += usize::from(results.recv().await.expect("admission result"));
    }
    assert_eq!(queued, 64);
    cancel.cancel();
    while let Some(task) = tasks.join_next().await {
        task.expect("admission task");
    }
    drop(active);
    drop(
        waiting(&scheduler, "fresh", &live)
            .await
            .expect("no leaked waiter or slot"),
    );
}
