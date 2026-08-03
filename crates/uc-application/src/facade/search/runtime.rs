use std::sync::Arc;

use thiserror::Error;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::{SearchCoordinator, SearchFacade, SearchRuntimeDeps};

#[derive(Debug, Error)]
pub enum SearchRuntimeError {
    #[error("search coordinator failed: {0}")]
    Coordinator(String),
    #[error("search coordinator task failed: {0}")]
    Task(String),
}

pub struct SearchRuntime {
    facade: Arc<SearchFacade>,
    cancel: CancellationToken,
    task: Option<JoinHandle<anyhow::Result<()>>>,
}

impl SearchRuntime {
    pub fn start(deps: SearchRuntimeDeps) -> Self {
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

    pub fn facade(&self) -> Arc<SearchFacade> {
        Arc::clone(&self.facade)
    }

    pub async fn shutdown(mut self) -> Result<(), SearchRuntimeError> {
        self.cancel.cancel();
        let Some(task) = self.task.take() else {
            return Ok(());
        };
        match task.await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(SearchRuntimeError::Coordinator(error.to_string())),
            Err(error) => Err(SearchRuntimeError::Task(error.to_string())),
        }
    }
}

impl Drop for SearchRuntime {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
