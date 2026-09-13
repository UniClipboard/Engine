use tokio::runtime::Handle;
use tokio::sync::oneshot;

use super::{Engine, EventStream, StartupLifecycleInput, StartupProgressInput};
use crate::{EngineConfig, EngineError, EngineErrorCategory, HostCapabilities};

#[cfg(test)]
mod tests;

impl Engine {
    pub(super) async fn start_owned(
        config: EngineConfig,
        host: HostCapabilities,
        progress: StartupProgressInput,
        lifecycle: StartupLifecycleInput,
    ) -> Result<(Self, EventStream), EngineError> {
        let (sender, receiver) = oneshot::channel();
        tokio::spawn(async move {
            let result =
                Self::start_runtime(config, host, &progress, lifecycle.requests.clone()).await;
            let result = match result {
                Ok(parts) => Ok(StartupHandoff::new(parts, progress)),
                Err(error) => {
                    lifecycle.requests.fail_startup(error.clone());
                    progress.finish(&Err::<(), _>(error.clone()));
                    Err(error)
                }
            };
            // 发送成功也可能尚未被接收；交接守卫持续持有关闭责任直到调用方实际取走。
            let _ = sender.send(result);
        });
        receiver.await.map_err(|_| startup_task_failed())??.claim()
    }
}

struct StartupHandoff {
    parts: Option<(Engine, EventStream)>,
    progress: Option<StartupProgressInput>,
    executor: Handle,
}

impl StartupHandoff {
    fn new(parts: (Engine, EventStream), progress: StartupProgressInput) -> Self {
        Self {
            parts: Some(parts),
            progress: Some(progress),
            executor: Handle::current(),
        }
    }

    fn claim(mut self) -> Result<(Engine, EventStream), EngineError> {
        let parts = self.parts.take().ok_or_else(startup_task_failed)?;
        if let Some(progress) = self.progress.take() {
            progress.finish(&Ok::<(), EngineError>(()));
        }
        Ok(parts)
    }
}

impl Drop for StartupHandoff {
    fn drop(&mut self) {
        let Some((engine, events)) = self.parts.take() else {
            return;
        };
        let progress = self.progress.take();
        self.executor.spawn(async move {
            // 未交出的实例没有宿主等待预算，但仍走与显式关闭完全相同的收尾。
            let result = engine.shutdown_until_complete().await;
            drop(engine);
            drop(events);
            if let (Some(progress), Err(error)) = (progress.as_ref(), &result) {
                progress.finish(&Err::<(), _>(error.clone()));
            }
            // 只有真实关闭结束后，进度观察者才允许重试这次被放弃的启动。
            drop(progress);
        });
    }
}

fn startup_task_failed() -> EngineError {
    EngineError::new(1108, EngineErrorCategory::Internal, true)
}
