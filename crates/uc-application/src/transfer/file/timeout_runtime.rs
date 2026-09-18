use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::{JoinError, JoinHandle};
use tokio::time::{timeout_at, Instant};
use tracing::warn;

use super::facade::FileTransferFacade;
use crate::transfer::blob::facade::BlobTransferFacade;

pub(crate) struct FileTransferTimeoutRuntime {
    cancel: watch::Sender<bool>,
    handle: JoinHandle<()>,
}

impl FileTransferTimeoutRuntime {
    pub(crate) fn start(
        file_transfer: Arc<FileTransferFacade>,
        blob_transfer: Arc<BlobTransferFacade>,
    ) -> Self {
        let (cancel, receiver) = watch::channel(false);
        let handle = file_transfer.spawn_timeout_sweep(receiver, blob_transfer);
        Self { cancel, handle }
    }

    pub(crate) async fn shutdown(mut self, deadline: Option<Instant>) -> Result<(), JoinError> {
        let deadline = deadline.unwrap_or_else(|| Instant::now() + Duration::from_secs(1));
        let _ = self.cancel.send(true);
        match timeout_at(deadline, &mut self.handle).await {
            Ok(result) => result,
            Err(_) => {
                // 正在等待的磁盘线程不能靠取消异步等待结束，必须保留原任务到动作完成。
                warn!(
                    event = "task.shutdown_slow",
                    task = "file_transfer.timeout_sweep",
                    "file transfer timeout cleanup exceeded shutdown deadline"
                );
                self.handle.await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{watch, Arc, FileTransferTimeoutRuntime};
    use std::sync::Barrier;
    use std::time::Duration;
    use tokio::sync::{oneshot, Notify};
    use tokio::time::Instant;

    #[tokio::test(start_paused = true)]
    async fn expired_shared_deadline_keeps_the_current_action_owned() {
        let (cancel, mut receiver) = watch::channel(false);
        let (stopping, stopped) = oneshot::channel();
        let (release, released) = oneshot::channel();
        let runtime = FileTransferTimeoutRuntime {
            cancel,
            handle: tokio::spawn(async move {
                receiver.changed().await.unwrap();
                assert!(*receiver.borrow());
                stopping.send(()).unwrap();
                released.await.unwrap();
            }),
        };
        let started = Instant::now();
        let deadline = started + Duration::from_millis(10);
        tokio::time::advance(Duration::from_millis(20)).await;
        let closing = tokio::spawn(runtime.shutdown(Some(deadline)));
        stopped.await.unwrap();
        tokio::time::advance(Duration::from_secs(2)).await;
        assert!(!closing.is_finished());
        release.send(()).unwrap();
        closing.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn shutdown_preserves_early_task_failure() {
        let (cancel, _) = watch::channel(false);
        let runtime = FileTransferTimeoutRuntime {
            cancel,
            handle: tokio::spawn(async { panic!("PRIVATE_TASK_FAILURE") }),
        };
        assert!(runtime.shutdown(None).await.unwrap_err().is_panic());
    }

    #[tokio::test]
    async fn shutdown_waits_for_the_actual_blocking_thread_to_release_resources() {
        let (cancel, _) = watch::channel(false);
        let resource = Arc::new(());
        let held = Arc::clone(&resource);
        let release = Arc::new(Barrier::new(2));
        let worker_release = Arc::clone(&release);
        let started = Arc::new(Notify::new());
        let worker_started = Arc::clone(&started);
        let runtime = FileTransferTimeoutRuntime {
            cancel,
            handle: tokio::spawn(async move {
                tokio::task::spawn_blocking(move || {
                    let _held = held;
                    worker_started.notify_one();
                    worker_release.wait();
                })
                .await
                .unwrap();
            }),
        };
        started.notified().await;
        let mut closing = tokio::spawn(runtime.shutdown(Some(Instant::now())));
        let early = tokio::time::timeout(Duration::from_millis(10), &mut closing).await;
        let remained_pending = early.is_err();
        let still_held = Arc::strong_count(&resource) == 2;
        release.wait();
        match early {
            Ok(result) => result,
            Err(_) => closing.await,
        }
        .unwrap()
        .unwrap();
        assert!(remained_pending);
        assert!(still_held);
        assert_eq!(Arc::strong_count(&resource), 1);
    }
}
