use async_trait::async_trait;
use tokio::runtime::Handle;
use uc_application::deps::{GroupUpdateLocalWorkPort, GroupUpdateReceiveOutcome, GroupUpdateWork};
use uc_core::membership::KeyEpochError;
use uc_observability_contract::diagnostics::connectivity::LocalWorkContext;

use super::super::session_generation::begin_session_local_work;

pub struct SessionGroupUpdateLocalWork;
#[async_trait]
impl GroupUpdateLocalWorkPort for SessionGroupUpdateLocalWork {
    async fn execute(
        &self,
        work: GroupUpdateWork,
    ) -> Result<GroupUpdateReceiveOutcome, anyhow::Error> {
        let local = begin_session_local_work().ok_or(KeyEpochError::SpaceNotReady)?;
        let runtime = Handle::current();
        let context = LocalWorkContext::capture();
        tokio::task::spawn_blocking(move || {
            // 本地工作、应用锁和会话许可随实际 future 保留到完成，不随网络等待方取消。
            let _local = local;
            context.run(|| runtime.block_on(work))
        })
        .await
        .map_err(anyhow::Error::new)?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use tokio::sync::Notify;

    struct Completion(Arc<AtomicBool>);
    impl Drop for Completion {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelling_network_waiter_keeps_started_local_work_until_completion() {
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let done = Arc::new(AtomicBool::new(false));
        let work_started = started.clone();
        let work_release = release.clone();
        let work_done = done.clone();
        let waiter = tokio::spawn(async move {
            SessionGroupUpdateLocalWork
                .execute(Box::pin(async move {
                    let _completion = Completion(work_done);
                    work_started.notify_one();
                    work_release.notified().await;
                    Ok(GroupUpdateReceiveOutcome::Applied)
                }))
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), started.notified())
            .await
            .unwrap();
        waiter.abort();
        assert!(matches!(waiter.await, Err(source) if source.is_cancelled()));
        assert!(
            !done.load(Ordering::SeqCst),
            "cancellation must not abandon started work"
        );
        release.notify_one();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while !done.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
}
