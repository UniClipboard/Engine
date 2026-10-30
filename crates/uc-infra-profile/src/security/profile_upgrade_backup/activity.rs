//! 持有已接收的备份阻塞任务，直到任务执行结束或未执行即被丢弃。
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::sync::{Arc, Mutex, PoisonError};

use uc_application::deps::ProfileUpgradeBackupError;

#[derive(Default)]
pub struct ProfileUpgradeBackupActivity {
    state: Mutex<ActivityState>,
}

#[derive(Default)]
struct ActivityState {
    active: usize,
    callbacks: Vec<Box<dyn FnOnce() + Send>>,
    first_failure: Option<Arc<ProfileUpgradeBackupError>>,
    first_abnormal: Option<BackupWorkerAbnormal>,
}

/// 已接收的阻塞任务没有返回结果时的固定分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupWorkerAbnormal {
    Abandoned,
    Panicked,
}

#[derive(Debug)]
struct SharedBackupError(Arc<ProfileUpgradeBackupError>);

impl Display for SharedBackupError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("shared profile backup failure")
    }
}

impl Error for SharedBackupError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}

impl ProfileUpgradeBackupActivity {
    pub(super) fn begin(self: &Arc<Self>) -> BackupActivityGuard {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .active += 1;
        BackupActivityGuard {
            owner: Arc::clone(self),
            finished: false,
        }
    }

    /// 只在已接收工作仍未结算时登记通知；通知不代表资源释放。
    pub fn on_idle(&self, callback: impl FnOnce() + Send + 'static) -> bool {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.active == 0 {
            return false;
        }
        state.callbacks.push(Box::new(callback));
        true
    }

    /// 仅当已接收工作全部正常结算时，允许依赖它的清理重试。
    pub fn is_idle_without_abnormal(&self) -> bool {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.active == 0 && state.first_abnormal.is_none()
    }

    /// 保留首个任务错误及其完整来源，不随等待者消失。
    pub fn first_failure(&self) -> Option<Arc<ProfileUpgradeBackupError>> {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .first_failure
            .clone()
    }

    /// 返回未取得任务结果的首个固定分类。
    pub fn first_abnormal(&self) -> Option<BackupWorkerAbnormal> {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .first_abnormal
    }
}

pub(super) struct BackupActivityGuard {
    owner: Arc<ProfileUpgradeBackupActivity>,
    finished: bool,
}

impl BackupActivityGuard {
    pub(super) fn finish<T>(
        mut self,
        result: Result<T, ProfileUpgradeBackupError>,
    ) -> Result<T, ProfileUpgradeBackupError> {
        let result = match result {
            Ok(value) => Ok(value),
            Err(error) => {
                let shared = Arc::new(error);
                self.owner
                    .state
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .first_failure
                    .get_or_insert_with(|| Arc::clone(&shared));
                Err(ProfileUpgradeBackupError {
                    source: SharedBackupError(shared).into(),
                })
            }
        };
        self.finished = true;
        result
    }
}

impl Drop for BackupActivityGuard {
    fn drop(&mut self) {
        let callbacks = {
            let mut state = self
                .owner
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if !self.finished {
                state
                    .first_abnormal
                    .get_or_insert(if std::thread::panicking() {
                        BackupWorkerAbnormal::Panicked
                    } else {
                        BackupWorkerAbnormal::Abandoned
                    });
            }
            state.active -= 1;
            if state.active == 0 {
                std::mem::take(&mut state.callbacks)
            } else {
                Vec::new()
            }
        };
        for callback in callbacks {
            callback();
        }
    }
}
