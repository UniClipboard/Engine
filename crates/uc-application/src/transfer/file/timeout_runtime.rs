use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::{JoinError, JoinHandle};
use tokio::time::timeout;

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

    pub(crate) async fn shutdown(mut self) -> Result<(), JoinError> {
        let _ = self.cancel.send(true);
        match timeout(Duration::from_secs(1), &mut self.handle).await {
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

    #[tokio::test]
    async fn shutdown_preserves_early_task_failure() {
        let (cancel, _) = watch::channel(false);
        let runtime = FileTransferTimeoutRuntime {
            cancel,
            handle: tokio::spawn(async { panic!("PRIVATE_TASK_FAILURE") }),
        };
        assert!(runtime.shutdown().await.unwrap_err().is_panic());
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
        assert!(runtime.shutdown().await.unwrap_err().is_cancelled());
        assert_eq!(Arc::strong_count(&resource), 1);
    }
}
