use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use crate::{EngineError, EngineErrorCategory};
use tokio::sync::{watch, Mutex as AsyncMutex};
use uc_observability_contract::diagnostics::{record_task_join_failure, DiagnosticTaskKind};
use uc_observability_runtime::{EngineLogActivity, SignalResult};

/// 仅供端到端场景触发运行期构造后的登记失败。
#[cfg(feature = "dev-tools")]
pub(crate) static PROBE_BEGIN_FAILURE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[derive(Default)]
struct State {
    activity: Option<Arc<Mutex<EngineLogActivity>>>,
    beginning: Option<watch::Receiver<Option<bool>>>,
    release_pending: bool,
    beginning_active: bool,
    settled: Vec<Box<dyn FnOnce() + Send>>,
}

/// 单 Engine 的网络日志登记；进程观测由宿主独立管理。
#[derive(Clone, Default)]
pub(super) struct LogActivity {
    state: Arc<Mutex<State>>,
    transition: Arc<AsyncMutex<()>>,
}

impl LogActivity {
    pub(super) async fn begin(&self) -> Result<(), EngineError> {
        #[cfg(feature = "dev-tools")]
        if PROBE_BEGIN_FAILURE.swap(false, std::sync::atomic::Ordering::AcqRel) {
            return Err(EngineError::new(1108, EngineErrorCategory::Internal, true));
        }
        let _transition = self.transition.lock().await;
        let pending = self.state().release_pending;
        if pending {
            self.end_locked().await?;
        }
        let (beginning, registration) = {
            let mut state = self.state();
            if state.activity.is_some() {
                return Ok(());
            }
            if let Some(beginning) = &state.beginning {
                (beginning.clone(), None)
            } else {
                let (sender, receiver) = watch::channel(None);
                state.beginning = Some(receiver.clone());
                state.beginning_active = true;
                (
                    receiver,
                    Some(BeginningRegistration {
                        owner: self.clone(),
                        sender: Some(sender),
                        result: None,
                    }),
                )
            }
        };
        if let Some(registration) = registration {
            // 创建失败时守卫可能被同步丢弃；此时不能持有登记状态锁。
            let _ = tokio::task::spawn_blocking(move || {
                let mut registration = registration;
                registration.run();
            });
        }
        if wait_beginning(beginning).await? {
            Ok(())
        } else {
            self.state().beginning = None;
            Err(super::operation_cancelled_error())
        }
    }

    /// 已接收登记仍在执行时注册一次结束通知；回调不表示资源已释放。
    pub(super) fn on_beginning_settled(&self, callback: impl FnOnce() + Send + 'static) -> bool {
        let mut state = self.state();
        if !state.beginning_active {
            return false;
        }
        state.settled.push(Box::new(callback));
        true
    }

    pub(super) async fn end(&self) -> Result<(), EngineError> {
        let _transition = self.transition.lock().await;
        self.end_locked().await
    }

    async fn end_locked(&self) -> Result<(), EngineError> {
        let beginning = {
            let mut state = self.state();
            if state.beginning.is_none() && state.activity.is_none() {
                state.release_pending = false;
                return Ok(());
            }
            state.release_pending = true;
            state.beginning.clone()
        };
        if let Some(beginning) = beginning {
            if !wait_beginning(beginning).await? {
                let mut state = self.state();
                state.beginning = None;
                state.release_pending = false;
                return Ok(());
            }
        }
        let activity = {
            let state = self.state();
            let Some(activity) = state.activity.clone() else {
                return Ok(());
            };
            activity
        };
        // 停车或阻塞任务失败后，原负责人仍保留 closer。
        let result = tokio::task::spawn_blocking(move || {
            activity
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .end(Duration::MAX)
        })
        .await
        // discarded-source[contract-boundary]: the join owner records the stable failure classification
        .map_err(|_| join_failure())?;
        match result {
            None | Some(SignalResult::Completed) => {
                let mut state = self.state();
                state.activity = None;
                state.beginning = None;
                state.release_pending = false;
                Ok(())
            }
            Some(SignalResult::Failed | SignalResult::TimedOut | SignalResult::AlreadyShutdown) => {
                Err(EngineError::new(1108, EngineErrorCategory::Internal, true))
            }
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

struct BeginningRegistration {
    owner: LogActivity,
    sender: Option<watch::Sender<Option<bool>>>,
    result: Option<bool>,
}

impl BeginningRegistration {
    fn run(&mut self) {
        let activity = EngineLogActivity::begin(Duration::MAX).0;
        let admitted = activity.is_admitted();
        if admitted {
            self.owner.state().activity = Some(Arc::new(Mutex::new(activity)));
        }
        self.result = Some(admitted);
    }
}

impl Drop for BeginningRegistration {
    fn drop(&mut self) {
        let callbacks = {
            let mut state = self.owner.state();
            state.beginning_active = false;
            std::mem::take(&mut state.settled)
        };
        if let Some(sender) = self.sender.take() {
            if let Some(admitted) = self.result {
                sender.send_replace(Some(admitted));
            }
        }
        for callback in callbacks {
            callback();
        }
    }
}

async fn wait_beginning(mut beginning: watch::Receiver<Option<bool>>) -> Result<bool, EngineError> {
    loop {
        if let Some(admitted) = *beginning.borrow_and_update() {
            return Ok(admitted);
        }
        // discarded-source[channel]: the closed watch channel only indicates that its sender is gone
        beginning.changed().await.map_err(|_| join_failure())?;
    }
}

fn join_failure() -> EngineError {
    record_task_join_failure(DiagnosticTaskKind::EngineLifecycleTransition);
    EngineError::new(1108, EngineErrorCategory::Internal, true)
}
