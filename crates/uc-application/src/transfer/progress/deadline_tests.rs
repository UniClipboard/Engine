use super::{ElapsedClockError, ElapsedClockPort, ProgressExchangeDeadline, ProgressReceiveError};
use std::error::Error;
use std::io;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use uc_core::error_class::ErrorClass;

struct Clock(AtomicU64);
impl ElapsedClockPort for Clock {
    fn elapsed(&self) -> Result<Duration, ElapsedClockError> {
        let now = self.0.load(Ordering::SeqCst);
        if now == u64::MAX {
            Err(ElapsedClockError {
                source: Box::new(io::Error::other("synthetic clock failure")),
            })
        } else {
            Ok(Duration::from_secs(now))
        }
    }
}
#[tokio::test]
async fn elapsed_jump_is_checked_before_repolling_pending_network_work() {
    let clock = Arc::new(Clock(AtomicU64::new(0)));
    let deadline = ProgressExchangeDeadline::new(clock.clone(), Duration::from_secs(5)).unwrap();
    let entered = Arc::new(Notify::new());
    let resume = Arc::new(Notify::new());
    let effects = Arc::new(AtomicUsize::new(0));
    let work = {
        let entered = entered.clone();
        let resume = resume.clone();
        let effects = effects.clone();
        async move {
            effects.fetch_add(1, Ordering::SeqCst);
            entered.notify_one();
            resume.notified().await;
            effects.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    };
    let cancel = CancellationToken::new();
    let task = tokio::spawn(async move { deadline.network(work, &cancel).await });
    entered.notified().await;
    // 只模拟经过时间跳变；不把该逻辑测试当作物理系统挂起。
    clock.0.store(6, Ordering::SeqCst);
    resume.notify_one();
    assert!(task.await.unwrap().is_err());
    assert_eq!(effects.load(Ordering::SeqCst), 1);
}
#[test]
fn unreadable_clock_keeps_source_and_original_budget_stays_retired() {
    let clock = Arc::new(Clock(AtomicU64::new(0)));
    let deadline = ProgressExchangeDeadline::new(clock.clone(), Duration::from_secs(5)).unwrap();
    clock.0.store(u64::MAX, Ordering::SeqCst);
    let error = deadline.check(&CancellationToken::new()).unwrap_err();
    assert_eq!(error.class(), "elapsed_clock_unavailable");
    assert!(error.source().unwrap().source().unwrap().is::<io::Error>());
    clock.0.store(1, Ordering::SeqCst);
    assert!(deadline.check(&CancellationToken::new()).is_err());
}
