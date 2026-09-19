use std::sync::Arc;

use super::SpaceFacade;
use crate::runtime_lifecycle::LifecycleError;

impl SpaceFacade {
    /// 完整停止连接与成员维护，等待方离开不取消收尾，重复确认保留原始结果。
    pub async fn on_shutdown(self: &Arc<Self>) -> Result<(), Arc<LifecycleError>> {
        let owner = Arc::clone(self);
        tokio::spawn(async move { owner.finish_shutdown().await })
            .await
            .map_err(|source| {
                Arc::new(LifecycleError {
                    primary: source.into(),
                    additional: Vec::new(),
                })
            })?
    }

    async fn finish_shutdown(&self) -> Result<(), Arc<LifecycleError>> {
        let mut cached = self.shutdown_result.lock().await;
        if let Some(result) = cached.as_ref() {
            return result.clone();
        }
        let recovery = self.session_recovery.shutdown().await;
        let application = self.application.lock().await.take();
        let (connections, application) = tokio::join!(self.connections.shutdown(), async move {
            match application {
                Some(application) => application.shutdown().await,
                None => Ok(()),
            }
        },);
        let mut errors = Vec::new();
        if let Err(source) = recovery {
            errors.push(anyhow::Error::new(source).context("stop session recovery"));
        }
        if let Err(source) = connections {
            errors.push(anyhow::Error::new(source).context("stop peer connections"));
        }
        if let Err(source) = application {
            errors.push(source.context("stop membership maintenance"));
        }
        let result = LifecycleError::from_errors(errors).map_err(Arc::new);
        *cached = Some(result.clone());
        result
    }
}
