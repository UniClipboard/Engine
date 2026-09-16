use std::sync::Arc;

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::coordinator::SearchCoordinatorDeps;
use super::{SearchCoordinator, SearchFacade, SearchShutdownError};

pub(super) struct SearchRuntime {
    facade: Arc<SearchFacade>,
    cancel: CancellationToken,
    task: Option<JoinHandle<anyhow::Result<()>>>,
}

impl SearchRuntime {
    pub(super) fn start(deps: SearchCoordinatorDeps) -> Self {
        let search_index = Arc::clone(&deps.search_index);
        let coordinator = Arc::new(SearchCoordinator::new(deps));
        let facade = Arc::new(SearchFacade::with_runtime(
            search_index,
            Arc::clone(&coordinator),
        ));
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move { coordinator.start(task_cancel).await });
        Self {
            facade,
            cancel,
            task: Some(task),
        }
    }

    pub(super) fn facade(&self) -> Arc<SearchFacade> {
        Arc::clone(&self.facade)
    }

    pub(super) async fn shutdown(mut self) -> Result<(), SearchShutdownError> {
        self.cancel.cancel();
        let Some(task) = self.task.take() else {
            return Ok(());
        };
        match task.await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(source)) => Err(SearchShutdownError::Coordinator { source }),
            Err(source) => Err(SearchShutdownError::Task { source }),
        }
    }
}

impl Drop for SearchRuntime {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
