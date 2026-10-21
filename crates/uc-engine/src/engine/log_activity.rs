use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use uc_observability_contract::diagnostics::{record_task_join_failure, DiagnosticTaskKind};
use uc_observability_runtime::EngineLogActivity;

/// 本实例对进程网络日志的登记：运行与恢复期间持有，暂停或关闭成功、发布结果之前交还。
///
/// 最后一个工作中的 Engine 交还时网络日志在这里等待文件与租约实际交还，因此宿主读到 `Suspended`
/// 或 `Stopped` 时进程已不再持有网络日志文件。等待只涉及有界队列内已接受记录的本地落盘，不设期限；
/// 宿主期限照旧只结束宿主等待。
#[derive(Clone, Default)]
pub(super) struct LogActivity(Arc<Mutex<Option<EngineLogActivity>>>);

impl LogActivity {
    pub(super) async fn begin(&self) {
        if self.slot().is_some() {
            return;
        }
        // 重开会等待文件线程；移动绑定使用单线程运行期，不能在运行线程上等待。
        match tokio::task::spawn_blocking(|| EngineLogActivity::begin(Duration::MAX).0).await {
            Ok(activity) => *self.slot() = Some(activity),
            // discarded-source[contract-boundary]: `tokio::task::JoinError`: the task kind is the full classification; logging continues without this registration
            Err(_) => record_task_join_failure(DiagnosticTaskKind::EngineLifecycleTransition),
        }
    }

    pub(super) async fn end(&self) {
        let Some(activity) = self.slot().take() else {
            return;
        };
        if tokio::task::spawn_blocking(move || activity.end(Duration::MAX))
            .await
            .is_err()
        {
            record_task_join_failure(DiagnosticTaskKind::EngineLifecycleTransition);
        }
    }

    fn slot(&self) -> std::sync::MutexGuard<'_, Option<EngineLogActivity>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
