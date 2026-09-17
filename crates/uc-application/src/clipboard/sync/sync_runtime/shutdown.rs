use std::sync::atomic::Ordering;
use std::sync::Arc;

use super::ClipboardSyncRuntime;
use crate::runtime_lifecycle::LifecycleError;

impl ClipboardSyncRuntime {
    pub async fn shutdown(self: &Arc<Self>) -> Result<(), Arc<LifecycleError>> {
        self.stopping.store(true, Ordering::Release);
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
        let inbound = self.inbound.lock().await.take();
        // 两个独立接收循环同时收到停止，不因其中一个失败跳过另一个。
        let (recovery, inbound) = tokio::join!(self.recovery.shutdown(), async {
            match inbound {
                Some(inbound) => inbound.shutdown().await,
                None => Ok(()),
            }
        });
        let _dispatch = self.delivery_gate.lock().await;
        let mut errors = Vec::new();
        if let Err(source) = recovery {
            errors.push(anyhow::Error::new(source).context("stop offline clipboard recovery"));
        }
        if let Err(source) = inbound {
            errors.push(anyhow::Error::new(source).context("stop inbound clipboard work"));
        }
        let result = LifecycleError::from_errors(errors).map_err(Arc::new);
        *cached = Some(result.clone());
        result
    }
}
