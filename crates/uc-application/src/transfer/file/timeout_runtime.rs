use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::{JoinError, JoinHandle};
use tokio::time::{timeout_at, Instant};

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
                self.handle.abort();
                self.handle.await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{watch, Arc, FileTransferTimeoutRuntime};
    use std::time::Duration;
    use tokio::time::Instant;

    #[tokio::test(start_paused = true)]
    async fn expired_shared_deadline_does_not_grant_a_new_grace_period() {
        let (cancel, _) = watch::channel(false);
        let runtime = FileTransferTimeoutRuntime {
            cancel,
            handle: tokio::spawn(std::future::pending()),
        };
        let started = Instant::now();
        let deadline = started + Duration::from_millis(10);
        tokio::time::advance(Duration::from_millis(20)).await;
        assert!(runtime
            .shutdown(Some(deadline))
            .await
            .unwrap_err()
            .is_cancelled());
        assert!(started.elapsed() < Duration::from_millis(100));
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

    #[tokio::test(start_paused = true)]
    async fn shutdown_reports_forced_cancellation_after_resources_are_released() {
        let (cancel, _) = watch::channel(false);
        let resource = Arc::new(());
        let held = Arc::clone(&resource);
        let runtime = FileTransferTimeoutRuntime {
            cancel,
            handle: tokio::spawn(async move {
                let _held = held;
                std::future::pending::<()>().await;
            }),
        };
        assert!(runtime.shutdown(None).await.unwrap_err().is_cancelled());
        assert_eq!(Arc::strong_count(&resource), 1);
    }
}
