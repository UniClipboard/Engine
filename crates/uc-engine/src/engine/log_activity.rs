use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use crate::{EngineError, EngineErrorCategory};
use uc_observability_contract::diagnostics::{record_task_join_failure, DiagnosticTaskKind};
use uc_observability_runtime::{EngineLogActivity, SignalResult};

#[derive(Default)]
struct State {
    activity: Option<Arc<Mutex<EngineLogActivity>>>,
    release_pending: bool,
}

/// 单 Engine 的网络日志登记；进程观测由宿主独立管理。
#[derive(Clone, Default)]
pub(super) struct LogActivity(Arc<Mutex<State>>);

impl LogActivity {
    pub(super) async fn begin(&self) -> Result<(), EngineError> {
        let pending = self.state().release_pending;
        if pending {
            self.end().await?;
        }
        if self.state().activity.is_some() {
            return Ok(());
        }
        let activity = tokio::task::spawn_blocking(|| EngineLogActivity::begin(Duration::MAX).0)
            .await
            // discarded-source[contract-boundary]: the join owner records the stable failure classification
            // discarded-source[contract-boundary]: the join owner records the stable failure classification
            .map_err(|_| join_failure())?;
        self.state().activity = Some(Arc::new(Mutex::new(activity)));
        Ok(())
    }

    pub(super) async fn end(&self) -> Result<(), EngineError> {
        let activity = {
            let mut state = self.state();
            let Some(activity) = state.activity.clone() else {
                return Ok(());
            };
            state.release_pending = true;
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
                state.release_pending = false;
                Ok(())
            }
            Some(SignalResult::Failed | SignalResult::TimedOut | SignalResult::AlreadyShutdown) => {
                Err(EngineError::new(1108, EngineErrorCategory::Internal, true))
            }
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn join_failure() -> EngineError {
    record_task_join_failure(DiagnosticTaskKind::EngineLifecycleTransition);
    EngineError::new(1108, EngineErrorCategory::Internal, true)
}
