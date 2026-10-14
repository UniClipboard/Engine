//! 真实本地工作保留世代许可；取消等待者不能丢弃已经开始的准备动作。
use super::super::session_generation::begin_session_local_work;
use async_trait::async_trait;
use std::panic::{catch_unwind, AssertUnwindSafe};
use tokio::runtime::Handle;
use tracing::dispatcher::{get_default, with_default};
use uc_application::deps::{
    ActivePullLocalWorkPort, ActivePullPreparationError, ActivePullPreparationStop, ActivePullWork,
};
use uc_observability_contract::diagnostics::connectivity::LocalWorkContext;
use uc_observability_contract::diagnostics::{record_task_join_failure, DiagnosticTaskKind};
use uc_observability_contract::{error_source::io_error_kind, uc_warn};

#[derive(Debug, thiserror::Error)]
enum LocalWorkFailure {
    #[error("active pull local work generation is retired")]
    GenerationRetired,
    #[error("active pull local work panicked")]
    Panicked,
}
pub struct SessionActivePullLocalWork;
#[async_trait]
impl ActivePullLocalWorkPort for SessionActivePullLocalWork {
    async fn execute(
        &self,
        work: ActivePullWork,
        stop: ActivePullPreparationStop,
    ) -> Result<Vec<u8>, ActivePullPreparationError> {
        let local = match begin_session_local_work() {
            Some(local) => local,
            None => {
                uc_warn!(
                    error_kind = "active_pull_generation_retired",
                    "active pull local work cannot start in retired generation"
                );
                return Err(ActivePullPreparationError::Internal(Box::new(
                    LocalWorkFailure::GenerationRetired,
                )));
            }
        };
        let runtime = Handle::current();
        let context = LocalWorkContext::capture();
        let dispatch = get_default(Clone::clone);
        tokio::task::spawn_blocking(move || {
            with_default(&dispatch, || {
                let _local = local;
                // 排进阻塞池仍可停止；真正执行后由准备 owner 在完整动作之间检查。
                stop.check()?;
                let result =
                    catch_unwind(AssertUnwindSafe(|| context.run(|| runtime.block_on(work))));
                let result = match result {
                    Ok(result) => result,
                    // discarded-source[no-information]: panic 载荷不是 Error，可能含业务正文，只记录固定分类。
                    Err(_) => Err(ActivePullPreparationError::Internal(Box::new(
                        LocalWorkFailure::Panicked,
                    ))),
                };
                if let Err(ActivePullPreparationError::Internal(source)) = &result {
                    uc_warn!(
                        error_kind = "serve_internal",
                        io_error_kind = io_error_kind(source.as_ref()),
                        "active pull local work failed"
                    );
                }
                result
            })
        })
        .await
        .map_err(|source| {
            record_task_join_failure(DiagnosticTaskKind::ActiveClipboardWorker);
            ActivePullPreparationError::Internal(Box::new(source))
        })?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::{poll_fn, Future};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Arc,
    };
    use std::task::Poll;
    use tokio::sync::oneshot;

    struct DropQuota(Arc<AtomicUsize>);
    impl Drop for DropQuota {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    /// 阻塞池只排队、尚未执行时取消，不进入准备 owner，并归还捕获的原额度。
    #[test]
    fn cancelled_queued_work_never_begins_preparation() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .max_blocking_threads(1)
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (release, barrier) = mpsc::channel();
            let (entered, started) = oneshot::channel();
            let occupying = tokio::task::spawn_blocking(move || {
                entered.send(()).unwrap();
                barrier.recv().unwrap();
            });
            started.await.unwrap();
            let ran = Arc::new(AtomicUsize::new(0));
            let dropped = Arc::new(AtomicUsize::new(0));
            let quota = DropQuota(dropped.clone());
            let started_work = ran.clone();
            let work: ActivePullWork = Box::pin(async move {
                let _quota = quota;
                started_work.fetch_add(1, Ordering::SeqCst);
                Ok(vec![1])
            });
            let stop = ActivePullPreparationStop::default();
            let executor = SessionActivePullLocalWork;
            let mut execution = Box::pin(executor.execute(work, stop.clone()));
            assert!(poll_fn(|cx| Poll::Ready(execution.as_mut().poll(cx).is_pending())).await);
            stop.cancel();
            release.send(()).unwrap();
            assert!(matches!(
                execution.await,
                Err(ActivePullPreparationError::Cancelled(_))
            ));
            occupying.await.unwrap();
            assert_eq!(ran.load(Ordering::SeqCst), 0);
            assert_eq!(dropped.load(Ordering::SeqCst), 1);
        });
    }
    /// 等待者已取消，已开始动作的实际 IO 失败仍由原工作桥接记录一次且脱敏。
    #[tokio::test]
    async fn begun_work_failure_remains_visible_after_waiter_is_dropped() {
        let logs = uc_testkit::log_capture::CapturedLogs::default();
        let _guard = logs.install();
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let dropped = Arc::new(AtomicUsize::new(0));
        let quota = DropQuota(dropped.clone());
        let began = started.clone();
        let barrier = release.clone();
        let work: ActivePullWork = Box::pin(async move {
            let _quota = quota;
            began.notify_one();
            barrier.acquire().await.unwrap().forget();
            Err(ActivePullPreparationError::Internal(Box::new(
                std::io::Error::other("PRIVATE_WORK_AFTER_DROP"),
            )))
        });
        let executor = SessionActivePullLocalWork;
        let stop = ActivePullPreparationStop::default();
        let mut execution = Box::pin(executor.execute(work, stop.clone()));
        assert!(poll_fn(|cx| Poll::Ready(execution.as_mut().poll(cx).is_pending())).await);
        started.notified().await;
        stop.cancel();
        drop(execution);
        assert_eq!(dropped.load(Ordering::SeqCst), 0);
        release.add_permits(1);
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while logs.count("active pull local work failed") == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(logs.count("active pull local work failed"), 1);
        assert!(logs.output().contains("io_error_kind=Other"));
        assert!(!logs.output().contains("PRIVATE_WORK_AFTER_DROP"));
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
    }
}
