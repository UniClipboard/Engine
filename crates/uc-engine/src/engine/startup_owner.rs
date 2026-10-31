#[cfg(feature = "dev-tools")]
use std::error::Error;
use std::io;
use std::panic::{catch_unwind, AssertUnwindSafe};
#[cfg(feature = "dev-tools")]
use std::sync::atomic::AtomicU8;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::runtime::{Handle, TryCurrentError};
use tokio::sync::oneshot;
use uc_application::deps::{LifecycleError, StopProfileRuntimePort};
use uc_application::facade::NetworkRecoveryFacade;
use uc_infra_profile::security::ProfileUpgradeBackupActivity;
use uc_observability_contract::diagnostics::{record_task_join_failure, DiagnosticTaskKind};

use super::lifecycle::TransitionQueue;
use super::log_activity::LogActivity;
use super::owned_executor::{ExecutorStop, OwnedExecutor, OwnedExecutorStartError};
use super::startup::StartupProgressStore;
use super::startup_outcome::{StartupOutcomeCell, StartupOutcomeSnapshot};
use super::{Engine, EngineRuntime, EventStream, StartupLifecycleInput, StartupProgressInput};
use crate::runtime::{classify_startup_error, RecoverableRuntime, ReusableHost};
#[cfg(feature = "dev-tools")]
use crate::HostCapabilityErrorCategory;
use crate::{
    EngineConfig, EngineError, EngineErrorCategory, HostCapabilityError, ResourceReleaseOutcome,
};

/// 仅供端到端程序控制未交付实例的清理执行器创建失败。
#[cfg(feature = "dev-tools")]
pub(crate) static PROBE_ENGINE_CLEANUP_FAILURE: AtomicU8 = AtomicU8::new(0);
#[cfg(feature = "dev-tools")]
pub(crate) static PROBE_HOST_RETRY_EXECUTOR_PANIC: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "dev-tools")]
pub(crate) static PROBE_HOST_RETRY_DROP_PANIC: AtomicBool = AtomicBool::new(false);

#[cfg(feature = "dev-tools")]
struct RetryExecutorDropProbe {
    _runtime: tokio::runtime::Runtime,
}

#[cfg(feature = "dev-tools")]
impl Drop for RetryExecutorDropProbe {
    fn drop(&mut self) {
        panic!("controlled retry executor destructor panic");
    }
}

#[cfg(test)]
mod tests;

/// 一次启动的失败清理负责人。未交出的实例仍可能持有文件；观察者消失不能解除该责任。
#[derive(Default)]
pub(crate) struct StartupCleanupOwner {
    failed: Mutex<Option<RetainedStartupCleanup>>,
    executor_error: Mutex<Option<TryCurrentError>>,
    owned_executor: Mutex<Option<Arc<OwnedExecutor>>>,
    owned_start_error: Mutex<Option<OwnedExecutorStartError>>,
    requests: Mutex<Option<Arc<TransitionQueue>>>,
    backup_activity: Mutex<Option<Arc<ProfileUpgradeBackupActivity>>>,
    progress: Mutex<Option<Arc<StartupProgressStore>>>,
    construction_finished: AtomicBool,
    unpolled_cleanup: AtomicBool,
    outcome: StartupOutcomeCell,
}

struct RetainedStartupCleanup {
    _resource: StartupResource,
    _error: Option<EngineError>,
    _startup_error: Option<EngineError>,
    _host_errors: Vec<HostCapabilityError>,
    _worker_error: Option<UnpolledCleanupError>,
    rollback: Option<StartupRollback>,
    host_close_confirmed: bool,
    host_close_pending: bool,
    executor_close_pending: bool,
    executor_close_confirmed: bool,
    construction_interrupted: bool,
    runtime_cleanup_available: bool,
    safe_runtime_interrupted: bool,
    pending_progress: Option<StartupFailureProgress>,
    // 失败后自持，直到负责人支持并完成显式恢复或进程结束。
    _owner: Arc<StartupCleanupOwner>,
}

struct StartupFailureProgress {
    input: StartupProgressInput,
    startup_error: EngineError,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum UnpolledCleanupError {
    #[error("startup cleanup thread creation failed")]
    Thread(#[source] io::Error),
    #[error("startup cleanup executor creation failed")]
    Executor(#[source] io::Error),
    #[error("startup cleanup executor construction panicked")]
    ExecutorPanicked,
    #[error("startup cleanup executor destruction panicked")]
    ExecutorDropPanicked,
    #[error("startup cleanup callback and executor destruction panicked")]
    CallbackAndExecutorDropPanicked,
    #[error("startup cleanup callback panicked")]
    CallbackPanicked,
}

struct StartupRollback {
    _profile: Arc<dyn StopProfileRuntimePort>,
    _network: Option<Arc<NetworkRecoveryFacade>>,
    _error: Option<LifecycleError>,
}

enum StartupResource {
    Host {
        _host: Arc<ReusableHost>,
    },
    Runtime {
        _runtime: Arc<RecoverableRuntime>,
        _log_activity: LogActivity,
    },
    Engine {
        _engine: Arc<Engine>,
        _events: EventStream,
    },
}

impl StartupCleanupOwner {
    pub(super) fn register_requests(&self, requests: Arc<TransitionQueue>) {
        *self.requests.lock().unwrap_or_else(PoisonError::into_inner) = Some(requests);
    }

    pub(super) fn register_progress(&self, progress: Arc<StartupProgressStore>) {
        *self.progress.lock().unwrap_or_else(PoisonError::into_inner) = Some(progress);
    }

    pub(crate) fn accept_host(self: &Arc<Self>, host: Arc<ReusableHost>) {
        *self.failed.lock().unwrap_or_else(PoisonError::into_inner) =
            Some(RetainedStartupCleanup {
                _resource: StartupResource::Host { _host: host },
                _error: None,
                _startup_error: None,
                _host_errors: Vec::new(),
                _worker_error: None,
                rollback: None,
                host_close_confirmed: false,
                host_close_pending: false,
                executor_close_pending: false,
                executor_close_confirmed: false,
                construction_interrupted: false,
                runtime_cleanup_available: false,
                safe_runtime_interrupted: false,
                pending_progress: None,
                _owner: Arc::clone(self),
            });
        self.outcome.update(|outcome| outcome.begin());
    }

    /// 等待构造期间已接收的备份工作；通知本身不是资源释放回执。
    pub(crate) async fn wait_recovery_backup(&self) -> bool {
        let activity = self
            .backup_activity
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let Some(activity) = activity else {
            return true;
        };
        let (settled, waiting) = oneshot::channel();
        if !activity.on_idle(move || {
            let _ = settled.send(());
        }) {
            return true;
        }
        waiting.await.is_ok()
    }

    /// 为进程释放线程等待同一组已接收的备份工作，不依赖异步执行器。
    pub(crate) fn wait_recovery_backup_blocking(&self) -> bool {
        let activity = self
            .backup_activity
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let Some(activity) = activity else {
            return true;
        };
        let (settled, waiting) = std::sync::mpsc::channel();
        if !activity.on_idle(move || {
            let _ = settled.send(());
        }) {
            return true;
        }
        waiting.recv().is_ok()
    }

    /// 未交出运行期时，只有回滚和备份工作均已结算才能交还宿主。
    pub(crate) fn settle_recovery_construction(&self) -> bool {
        if self.rollback_unconfirmed() {
            return false;
        }
        let activity = self
            .backup_activity
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(activity) = activity {
            if activity.on_idle(|| {}) || activity.first_abnormal().is_some() {
                return false;
            }
        }
        self.deliver();
        true
    }

    /// 仅重试构造与备份已结算且没有未确认回滚的空闲宿主变化流。
    pub(super) fn retry_host_internal(
        self: &Arc<Self>,
        deadline: Duration,
    ) -> StartupOutcomeSnapshot {
        let host = {
            let mut retained = self.failed.lock().unwrap_or_else(PoisonError::into_inner);
            retained.as_mut().and_then(|cleanup| {
                let host = self.retryable_host(cleanup)?;
                if self.outcome.lock().retry_cleanup() {
                    cleanup.host_close_pending = true;
                    Some(host)
                } else {
                    None
                }
            })
        };
        if let Some(host) = host {
            let owner = Arc::clone(self);
            let spawn_owner = Arc::clone(self);
            let worker = std::thread::Builder::new()
                .name("uc-startup-host-retry".to_owned())
                .spawn(move || {
                    let runtime = match catch_unwind(|| {
                        #[cfg(feature = "dev-tools")]
                        if PROBE_HOST_RETRY_EXECUTOR_PANIC.swap(false, Ordering::SeqCst) {
                            panic!("controlled retry executor construction panic");
                        }
                        tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                    }) {
                        Ok(Ok(runtime)) => runtime,
                        Ok(Err(source)) => {
                            owner.fail_unpolled_worker(UnpolledCleanupError::Executor(source));
                            return;
                        }
                        Err(_) => {
                            owner.fail_unpolled_worker(UnpolledCleanupError::ExecutorPanicked);
                            return;
                        }
                    };
                    let result = catch_unwind(AssertUnwindSafe(|| {
                        runtime.block_on(host.close_change_stream())
                    }));
                    let destroyed = catch_unwind(AssertUnwindSafe(|| {
                        #[cfg(feature = "dev-tools")]
                        if PROBE_HOST_RETRY_DROP_PANIC.swap(false, Ordering::SeqCst) {
                            drop(RetryExecutorDropProbe { _runtime: runtime });
                            return;
                        }
                        drop(runtime);
                    }));
                    drop(host);
                    if destroyed.is_err() {
                        let failure = match result {
                            Ok(Err(error)) => {
                                owner.record_host_error(error);
                                UnpolledCleanupError::ExecutorDropPanicked
                            }
                            Err(_) => UnpolledCleanupError::CallbackAndExecutorDropPanicked,
                            Ok(Ok(())) => UnpolledCleanupError::ExecutorDropPanicked,
                        };
                        owner.fail_unpolled_worker(failure);
                        return;
                    }
                    match result {
                        Ok(Ok(())) => owner.host_closed(),
                        Ok(Err(error)) => owner.fail_host(
                            owner.primary_error().unwrap_or_else(startup_task_failed),
                            error,
                        ),
                        Err(_) => {
                            owner.fail_unpolled_worker(UnpolledCleanupError::CallbackPanicked)
                        }
                    }
                });
            if let Err(source) = worker {
                spawn_owner.fail_unpolled_worker(UnpolledCleanupError::Thread(source));
            }
        }
        self.wait_internal(deadline)
    }

    /// 调用方持有失败资源锁时，判断是否仍有明确支持重试的宿主关闭器。
    fn retryable_host(&self, cleanup: &RetainedStartupCleanup) -> Option<Arc<ReusableHost>> {
        let executor_missing = self
            .executor_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some();
        let construction_finished = self.construction_finished.load(Ordering::Acquire);
        let unpolled_cleanup = self.unpolled_cleanup.load(Ordering::Acquire);
        let backup_unsettled = self
            .backup_activity
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|activity| !activity.is_idle_without_abnormal());
        if !(executor_missing || construction_finished || unpolled_cleanup)
            || backup_unsettled
            || cleanup.rollback.is_some()
            || cleanup._host_errors.is_empty()
            || cleanup._worker_error.is_some()
            || cleanup.construction_interrupted
        {
            return None;
        }
        match &cleanup._resource {
            StartupResource::Host { _host } if _host.has_retryable_change_stream() => {
                Some(Arc::clone(_host))
            }
            _ => None,
        }
    }

    pub(crate) fn fail_host(self: &Arc<Self>, startup: EngineError, error: HostCapabilityError) {
        let retained = {
            let mut retained = self.failed.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(cleanup) = retained.as_mut() {
                cleanup._startup_error.get_or_insert(startup.clone());
                cleanup._host_errors.push(error);
                cleanup.host_close_pending = false;
                self.outcome.lock().record_primary(startup);
                true
            } else {
                false
            }
        };
        if retained {
            self.settle_construction_after_work();
        }
    }

    fn record_host_error(&self, error: HostCapabilityError) {
        if let Some(cleanup) = self
            .failed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_mut()
        {
            cleanup._host_errors.push(error);
        }
    }

    /// 构造结束后等待所有已接收的关闭和备份工作。
    pub(crate) fn finish_construction_failure(
        self: &Arc<Self>,
        error: EngineError,
        progress: StartupProgressInput,
        startup_error: EngineError,
    ) {
        // 启动错误返回前先发布失败；清理未确认时不开放重试。
        progress.finish_cleanup_failed(&startup_error);
        let mut progress = Some(StartupFailureProgress {
            input: progress,
            startup_error,
        });
        let retained = {
            let mut retained = self.failed.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(cleanup) = retained.as_mut() {
                cleanup._error.get_or_insert(error);
                cleanup.pending_progress = progress.take();
                self.construction_finished.store(true, Ordering::Release);
                self.outcome.lock().begin_cleanup();
                true
            } else {
                false
            }
        };
        if retained {
            self.settle_construction_after_work();
        } else if let Some(progress) = progress {
            progress
                .input
                .finish(&Err::<(), _>(progress.startup_error.clone()));
        }
    }

    /// 每次完成通知都重新核对；回调不持有登记锁。
    fn settle_construction_after_work(self: &Arc<Self>) {
        let ready = self.construction_finished.load(Ordering::Acquire)
            || self.unpolled_cleanup.load(Ordering::Acquire)
            || self
                .executor_error
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_some();
        if !ready {
            return;
        }
        let log_activity = {
            let retained = self.failed.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(cleanup) = retained.as_ref() else {
                return;
            };
            if cleanup.host_close_pending || cleanup.executor_close_pending {
                return;
            }
            match &cleanup._resource {
                StartupResource::Runtime { _log_activity, .. } => Some(_log_activity.clone()),
                _ => None,
            }
        };
        if let Some(activity) = log_activity {
            let owner = Arc::clone(self);
            if activity.on_beginning_settled(move || owner.settle_construction_after_work()) {
                return;
            }
        }
        let activity = self
            .backup_activity
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(activity) = &activity {
            let owner = Arc::clone(self);
            if activity.on_idle(move || owner.settle_construction_after_work()) {
                return;
            }
        }
        self.capture_backup_failure();
        let abnormal = activity
            .as_ref()
            .is_some_and(|activity| activity.first_abnormal().is_some());
        let executor = {
            let mut retained = self.failed.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(cleanup) = retained.as_mut() else {
                return;
            };
            if cleanup.host_close_pending || cleanup.executor_close_pending {
                return;
            }
            if cleanup.host_close_confirmed
                && cleanup.rollback.is_none()
                && cleanup._worker_error.is_none()
                && !cleanup.construction_interrupted
                && !abnormal
                && !cleanup.executor_close_confirmed
                && self.outcome.lock().can_complete()
            {
                let executor = self
                    .owned_executor
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone();
                if executor.is_some() {
                    cleanup.executor_close_pending = true;
                }
                executor
            } else {
                None
            }
        };
        if let Some(executor) = executor {
            let owner = Arc::clone(self);
            let spawn_owner = Arc::clone(self);
            let worker = std::thread::Builder::new()
                .name("uc-startup-executor-stop".to_owned())
                .spawn(move || {
                    let result = executor.stop(Duration::MAX);
                    owner.finish_executor_stop(result);
                });
            if let Err(source) = worker {
                spawn_owner.fail_executor_stop(source);
            }
            return;
        }
        let (progress, released, interrupted) = {
            let mut retained = self.failed.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(cleanup) = retained.as_mut() else {
                return;
            };
            if cleanup.host_close_pending || cleanup.executor_close_pending {
                return;
            }
            let mut outcome = self.outcome.lock();
            if !outcome.can_complete() {
                return;
            }
            let progress = cleanup.pending_progress.take();
            let released = cleanup.host_close_confirmed
                && cleanup.rollback.is_none()
                && cleanup._worker_error.is_none()
                && !cleanup.construction_interrupted
                && !abnormal
                && (cleanup.executor_close_confirmed
                    || self
                        .owned_executor
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .is_none());
            let interrupted = cleanup.safe_runtime_interrupted
                || (self.unpolled_cleanup.load(Ordering::Acquire)
                    && cleanup._startup_error.is_none());
            if released {
                retained.take();
                outcome.released(None);
            } else {
                let retryable = self.retryable_host(cleanup).is_some();
                outcome.failed(
                    EngineError::new(1108, EngineErrorCategory::Internal, false),
                    retryable,
                );
            }
            (progress, released, interrupted)
        };
        if let Some(progress) = progress {
            if released {
                progress
                    .input
                    .finish(&Err::<(), _>(progress.startup_error.clone()));
            } else {
                progress
                    .input
                    .finish_cleanup_failed(&progress.startup_error);
            }
        }
        if released {
            let progress = self
                .progress
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            if let Some(progress) = progress {
                progress.confirm_cleanup_released(interrupted);
            }
        }
    }

    pub(crate) fn fail_unpolled_worker(self: &Arc<Self>, error: UnpolledCleanupError) {
        let retained = {
            let mut retained = self.failed.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(cleanup) = retained.as_mut() {
                cleanup._worker_error = Some(error);
                cleanup.host_close_pending = false;
                true
            } else {
                false
            }
        };
        if retained {
            self.settle_construction_after_work();
        }
    }

    fn finish_executor_stop(self: &Arc<Self>, result: ExecutorStop) {
        let mut retained = self.failed.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(cleanup) = retained.as_mut() {
            cleanup.executor_close_pending = false;
            if result == ExecutorStop::Stopped {
                cleanup.executor_close_confirmed = true;
            } else {
                cleanup._worker_error = Some(UnpolledCleanupError::CallbackPanicked);
            }
        }
        drop(retained);
        self.settle_construction_after_work();
    }

    fn fail_executor_stop(self: &Arc<Self>, source: io::Error) {
        let mut retained = self.failed.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(cleanup) = retained.as_mut() {
            cleanup.executor_close_pending = false;
            cleanup._worker_error = Some(UnpolledCleanupError::Thread(source));
        }
        drop(retained);
        self.settle_construction_after_work();
    }

    fn record_owned_start_error(&self, error: OwnedExecutorStartError) {
        *self
            .owned_start_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(error);
    }

    #[cfg(feature = "dev-tools")]
    #[allow(dead_code, reason = "Used by source-including probes")]
    pub(crate) fn probe_owned_start_failure(&self) -> Option<(&'static str, io::ErrorKind)> {
        let error = self
            .owned_start_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match error.as_ref()? {
            OwnedExecutorStartError::Thread(source) => Some(("thread", source.kind())),
            OwnedExecutorStartError::Runtime(source) => Some(("runtime", source.kind())),
            OwnedExecutorStartError::Initialization(_)
            | OwnedExecutorStartError::WorkerTerminated(_) => None,
        }
    }

    fn register_owned_executor(&self, executor: Arc<OwnedExecutor>) {
        *self
            .owned_executor
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(executor);
    }

    /// 启动前注册备份工作，保留执行器退出后仍在运行的任务。
    pub(crate) fn register_backup_activity(&self, activity: Arc<ProfileUpgradeBackupActivity>) {
        *self
            .backup_activity
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(activity);
    }

    pub(crate) fn register_profile(&self, profile: Arc<dyn StopProfileRuntimePort>) -> bool {
        let mut retained = self.failed.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(cleanup) = retained.as_mut() {
            cleanup.rollback = Some(StartupRollback {
                _profile: profile,
                _network: None,
                _error: None,
            });
            true
        } else {
            false
        }
    }

    pub(crate) fn register_network(&self, network: Arc<NetworkRecoveryFacade>) -> bool {
        let mut retained = self.failed.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(rollback) = retained
            .as_mut()
            .and_then(|cleanup| cleanup.rollback.as_mut())
        {
            rollback._network = Some(network);
            true
        } else {
            false
        }
    }

    pub(crate) fn fail_rollback(&self, primary: EngineError, error: LifecycleError) {
        let mut retained = self.failed.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(cleanup) = retained.as_mut() {
            cleanup._startup_error = Some(primary.clone());
            let mut outcome = self.outcome.lock();
            outcome.record_primary(primary);
            outcome.begin_cleanup();
            if let Some(rollback) = cleanup.rollback.as_mut() {
                rollback._error = Some(error);
            }
        }
    }

    pub(crate) fn confirm_rollback(&self) {
        let mut retained = self.failed.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(cleanup) = retained.as_mut() {
            cleanup.rollback = None;
        }
    }

    /// 仅供端到端程序验证回滚错误保留了真实的任务终止来源。
    #[cfg(feature = "dev-tools")]
    #[allow(dead_code, reason = "Used by source-including probes")]
    pub(crate) fn probe_rollback_task_panic(&self) -> bool {
        let retained = self.failed.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(error) = retained
            .as_ref()
            .and_then(|cleanup| cleanup.rollback.as_ref())
            .and_then(|rollback| rollback._error.as_ref())
        else {
            return false;
        };
        std::iter::once(&error.primary)
            .chain(error.additional.iter())
            .any(|error| {
                error
                    .chain()
                    .filter_map(|source| source.downcast_ref::<uc_core::TaskShutdownReport>())
                    .any(|report| {
                        report
                            .failures()
                            .iter()
                            .any(tokio::task::JoinError::is_panic)
                    })
            })
    }

    pub(crate) fn rollback_unconfirmed(&self) -> bool {
        self.failed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|cleanup| cleanup.rollback.is_some())
    }

    pub(crate) fn host_closed(self: &Arc<Self>) {
        let retained = {
            let mut retained = self.failed.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(cleanup) = retained.as_mut() {
                cleanup.host_close_confirmed = true;
                cleanup.host_close_pending = false;
                true
            } else {
                false
            }
        };
        if retained {
            self.settle_construction_after_work();
        }
    }

    pub(super) fn accept_runtime(
        self: &Arc<Self>,
        runtime: Arc<RecoverableRuntime>,
        log_activity: LogActivity,
    ) {
        *self.failed.lock().unwrap_or_else(PoisonError::into_inner) =
            Some(RetainedStartupCleanup {
                _resource: StartupResource::Runtime {
                    _runtime: runtime,
                    _log_activity: log_activity,
                },
                _error: None,
                _startup_error: None,
                _host_errors: Vec::new(),
                _worker_error: None,
                rollback: None,
                host_close_confirmed: false,
                host_close_pending: false,
                executor_close_pending: false,
                executor_close_confirmed: false,
                construction_interrupted: false,
                runtime_cleanup_available: true,
                safe_runtime_interrupted: false,
                pending_progress: None,
                _owner: Arc::clone(self),
            });
    }

    /// Engine 接管前关闭构造期自动清理，避免与后续 Engine 负责人重叠。
    pub(super) fn finish_runtime_construction(&self) {
        let mut retained = self.failed.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(cleanup) = retained.as_mut() {
            cleanup.runtime_cleanup_available = false;
        }
    }

    fn record_executor_error(&self, error: TryCurrentError) {
        *self
            .executor_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(error);
    }

    pub(super) fn input_dropped(&self, error: EngineError) {
        self.outcome.lock().input_dropped(error);
    }

    pub(crate) fn record_primary(&self, error: EngineError) {
        let primary = {
            let mut retained = self.failed.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(cleanup) = retained.as_mut() else {
                return;
            };
            let primary = cleanup._startup_error.get_or_insert(error).clone();
            self.outcome.lock().record_primary(primary.clone());
            primary
        };
        let requests = self
            .requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(requests) = requests {
            requests.fail_startup(primary);
        }
    }

    pub(super) fn primary_error(&self) -> Option<EngineError> {
        match self.outcome.lock().snapshot() {
            StartupOutcomeSnapshot::InputDropped { startup_error } => Some(startup_error),
            StartupOutcomeSnapshot::Constructing { startup_error }
            | StartupOutcomeSnapshot::Cleaning { startup_error }
            | StartupOutcomeSnapshot::Released { startup_error, .. }
            | StartupOutcomeSnapshot::Failed { startup_error, .. } => startup_error,
            StartupOutcomeSnapshot::NotStarted | StartupOutcomeSnapshot::Delivered => None,
        }
    }

    /// 完整启动范围验收前只供负责人内部使用。
    pub(crate) fn wait_internal(&self, deadline: Duration) -> StartupOutcomeSnapshot {
        self.outcome.wait(deadline)
    }

    pub(super) fn unconfirmed(&self) -> bool {
        self.failed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    fn accept(self: &Arc<Self>, engine: Engine, events: EventStream) -> Arc<Engine> {
        let engine = Arc::new(engine);
        let mut failed = self.failed.lock().unwrap_or_else(PoisonError::into_inner);
        *failed = Some(RetainedStartupCleanup {
            _resource: StartupResource::Engine {
                _engine: Arc::clone(&engine),
                _events: events,
            },
            _error: None,
            _startup_error: None,
            _host_errors: Vec::new(),
            _worker_error: None,
            rollback: None,
            host_close_confirmed: false,
            host_close_pending: false,
            executor_close_pending: false,
            executor_close_confirmed: false,
            construction_interrupted: false,
            runtime_cleanup_available: false,
            safe_runtime_interrupted: false,
            pending_progress: None,
            _owner: Arc::clone(self),
        });
        let mut outcome = self.outcome.lock();
        outcome.begin();
        outcome.begin_cleanup();
        engine
    }

    /// 已分离的备份任务完成后保留其独立操作结果；原始来源仍由备份负责人持有。
    fn capture_backup_failure(&self) {
        let activity = self
            .backup_activity
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let Some(activity) = activity else {
            return;
        };
        let error = if let Some(failure) = activity.first_failure() {
            Some(classify_startup_error(failure.as_ref()))
        } else if activity.first_abnormal().is_some() {
            Some(EngineError::new(1108, EngineErrorCategory::Internal, false))
        } else {
            None
        };
        if let Some(error) = error {
            self.outcome.lock().record_operation_error(error);
        }
    }

    #[cfg(feature = "dev-tools")]
    #[allow(dead_code, reason = "Used by source-including probes")]
    pub(crate) fn probe_cleanup_worker_failure(&self) -> Option<(&'static str, io::ErrorKind)> {
        let retained = self.failed.lock().unwrap_or_else(PoisonError::into_inner);
        match retained.as_ref()?._worker_error.as_ref()? {
            UnpolledCleanupError::Thread(error) => Some(("thread", error.kind())),
            UnpolledCleanupError::Executor(error) => Some(("executor", error.kind())),
            UnpolledCleanupError::CallbackPanicked
            | UnpolledCleanupError::ExecutorPanicked
            | UnpolledCleanupError::ExecutorDropPanicked
            | UnpolledCleanupError::CallbackAndExecutorDropPanicked => None,
        }
    }

    #[cfg(feature = "dev-tools")]
    #[allow(dead_code, reason = "Used by source-including probes")]
    pub(crate) fn probe_retry_executor_panic(&self) -> bool {
        self.failed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|cleanup| {
                matches!(
                    &cleanup._worker_error,
                    Some(UnpolledCleanupError::ExecutorPanicked)
                )
            })
    }

    #[cfg(feature = "dev-tools")]
    #[allow(dead_code, reason = "Used by source-including probes")]
    pub(crate) fn probe_retry_executor_drop_panic(&self) -> bool {
        self.failed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|cleanup| {
                matches!(
                    &cleanup._worker_error,
                    Some(UnpolledCleanupError::ExecutorDropPanicked)
                )
            })
    }

    #[cfg(feature = "dev-tools")]
    #[allow(dead_code, reason = "Used by source-including probes")]
    pub(crate) fn probe_retry_callback_and_drop_panic(&self) -> bool {
        self.failed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|cleanup| {
                matches!(
                    &cleanup._worker_error,
                    Some(UnpolledCleanupError::CallbackAndExecutorDropPanicked)
                )
            })
    }

    #[cfg(feature = "dev-tools")]
    #[allow(dead_code, reason = "Used by source-including probes")]
    pub(crate) fn probe_host_cleanup_errors(
        &self,
    ) -> Vec<(HostCapabilityErrorCategory, Option<io::ErrorKind>)> {
        self.failed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|cleanup| {
                cleanup
                    ._host_errors
                    .iter()
                    .map(|error| {
                        (
                            error.category(),
                            error
                                .source()
                                .and_then(|source| source.downcast_ref::<io::Error>())
                                .map(io::Error::kind),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn record_cleanup_worker_error(&self, error: UnpolledCleanupError) {
        let mut retained = self.failed.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(cleanup) = retained.as_mut() {
            cleanup._worker_error.get_or_insert(error);
        }
    }

    fn fail(&self, error: EngineError) {
        let mut failed = self.failed.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(cleanup) = failed.as_mut() {
            cleanup._error = Some(error.clone());
            self.outcome.lock().failed(error, false);
        }
    }

    fn release(&self, operation_error: Option<EngineError>) {
        let mut retained = self.failed.lock().unwrap_or_else(PoisonError::into_inner);
        let mut outcome = self.outcome.lock();
        if outcome.can_complete() {
            retained.take();
            outcome.released(operation_error);
        }
    }

    /// 登记失败时关闭已构造运行期，并等待日志登记与执行器的已接收工作。
    pub(super) fn start_runtime_cleanup(
        self: &Arc<Self>,
        runtime: Arc<RecoverableRuntime>,
        log_activity: LogActivity,
    ) {
        self.finish_runtime_construction();
        self.begin_cleanup();
        let owner = Arc::clone(self);
        let spawn_owner = Arc::clone(self);
        let worker = std::thread::Builder::new()
            .name("uc-startup-runtime-close".to_owned())
            .spawn(move || {
                let executor = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(executor) => executor,
                    Err(source) => {
                        owner.fail_unpolled_worker(UnpolledCleanupError::Executor(source));
                        return;
                    }
                };
                let result = catch_unwind(AssertUnwindSafe(|| {
                    executor.block_on(async {
                        match runtime.shutdown(None).await {
                            Ok(()) => log_activity.end().await,
                            Err(error) => {
                                runtime.wait_shutdown_obligations().await;
                                Err(error)
                            }
                        }
                    })
                }));
                drop(executor);
                drop(runtime);
                drop(log_activity);
                match result {
                    Ok(Ok(())) => owner.host_closed(),
                    Ok(Err(error)) => owner.fail_runtime_cleanup(error),
                    Err(_) => owner.fail_unpolled_worker(UnpolledCleanupError::CallbackPanicked),
                }
            });
        if let Err(source) = worker {
            spawn_owner.fail_unpolled_worker(UnpolledCleanupError::Thread(source));
        }
    }

    fn fail_runtime_cleanup(self: &Arc<Self>, error: EngineError) {
        let retained = {
            let mut retained = self.failed.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(cleanup) = retained.as_mut() {
                cleanup._error = Some(error);
                cleanup.host_close_pending = false;
                true
            } else {
                false
            }
        };
        if retained {
            self.settle_construction_after_work();
        }
    }

    /// 接受宿主关闭后由独立执行器持有，调用方消失不会取消该动作。
    pub(crate) fn start_host_close(
        self: &Arc<Self>,
        host: Arc<ReusableHost>,
        startup: EngineError,
    ) -> oneshot::Receiver<()> {
        self.begin_cleanup();
        let (completed, waiting) = oneshot::channel();
        let owner = Arc::clone(self);
        let spawn_owner = Arc::clone(self);
        let worker = std::thread::Builder::new()
            .name("uc-startup-host-close".to_owned())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(source) => {
                        drop(host);
                        owner.fail_unpolled_worker(UnpolledCleanupError::Executor(source));
                        let _ = completed.send(());
                        return;
                    }
                };
                let result = catch_unwind(AssertUnwindSafe(|| {
                    runtime.block_on(host.close_change_stream())
                }));
                // 回调和它交给该执行器的阻塞工作均结算后，才通知等待者。
                drop(runtime);
                drop(host);
                match result {
                    Ok(Ok(())) => owner.host_closed(),
                    Ok(Err(error)) => owner.fail_host(startup, error),
                    Err(_) => owner.fail_unpolled_worker(UnpolledCleanupError::CallbackPanicked),
                }
                let _ = completed.send(());
            });
        if let Err(source) = worker {
            spawn_owner.fail_unpolled_worker(UnpolledCleanupError::Thread(source));
        }
        waiting
    }

    pub(crate) fn begin_cleanup(&self) {
        let mut retained = self.failed.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(cleanup) = retained.as_mut() {
            cleanup.host_close_pending = true;
        }
        self.outcome.lock().begin_cleanup();
    }

    pub(crate) fn deliver(&self) {
        let mut retained = self.failed.lock().unwrap_or_else(PoisonError::into_inner);
        retained.take();
        self.outcome.lock().delivered();
        drop(retained);
        *self
            .owned_executor
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
    }
}

/// future 尚未开始时接管宿主清理；执行器不可用时仍保留未确认的负责人。
pub(super) struct UnpolledStartupCleanup {
    pending: Option<(Arc<StartupCleanupOwner>, Arc<ReusableHost>)>,
}

impl UnpolledStartupCleanup {
    pub(super) fn new(owner: Arc<StartupCleanupOwner>, host: Arc<ReusableHost>) -> Self {
        Self {
            pending: Some((owner, host)),
        }
    }

    pub(super) fn disarm(&mut self) {
        self.pending = None;
    }
}

impl Drop for UnpolledStartupCleanup {
    fn drop(&mut self) {
        let Some((owner, host)) = self.pending.take() else {
            return;
        };
        owner.unpolled_cleanup.store(true, Ordering::Release);
        let _waiting = owner.start_host_close(
            host,
            EngineError::new(1108, EngineErrorCategory::Internal, false),
        );
    }
}

struct StartupCleanupTask {
    owner: Arc<StartupCleanupOwner>,
    engine: Arc<Engine>,
    progress: Option<StartupProgressInput>,
    finished: bool,
}

impl StartupCleanupTask {
    fn run(mut self) {
        #[cfg(feature = "dev-tools")]
        let injected = PROBE_ENGINE_CLEANUP_FAILURE
            .compare_exchange(2, 0, Ordering::AcqRel, Ordering::Acquire)
            .is_ok();
        #[cfg(not(feature = "dev-tools"))]
        let injected = false;
        let runtime = match if injected {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "controlled executor creation failure",
            ))
        } else {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
        } {
            Ok(runtime) => runtime,
            Err(source) => {
                self.owner
                    .record_cleanup_worker_error(UnpolledCleanupError::Executor(source));
                return;
            }
        };
        let result = catch_unwind(AssertUnwindSafe(|| {
            runtime.block_on(self.engine.shutdown_until_released())
        }));
        // 保留执行器直到它接受的阻塞任务结算，再发布清理结果。
        drop(runtime);
        let result = match result {
            Ok(result) => result,
            Err(_) => {
                self.owner
                    .record_cleanup_worker_error(UnpolledCleanupError::CallbackPanicked);
                return;
            }
        };
        match result {
            ResourceReleaseOutcome::Released { error } => self.owner.release(error),
            ResourceReleaseOutcome::Pending => {
                let error = EngineError::new(1108, EngineErrorCategory::Internal, false);
                self.owner.fail(error.clone());
                if let Some(progress) = self.progress.as_ref() {
                    progress.finish_cleanup_failed(&error);
                }
            }
            ResourceReleaseOutcome::Failed { error } => {
                self.owner.fail(error.clone());
                if let Some(progress) = self.progress.as_ref() {
                    progress.finish_cleanup_failed(&error);
                }
            }
        }
        self.finished = true;
    }
}

impl Drop for StartupCleanupTask {
    fn drop(&mut self) {
        if !self.finished {
            let error = EngineError::new(1108, EngineErrorCategory::Internal, false);
            self.owner.fail(error.clone());
            if let Some(progress) = self.progress.as_ref() {
                progress.finish_cleanup_failed(&error);
            }
        }
    }
}

struct StartupConstructionGuard {
    owner: Arc<StartupCleanupOwner>,
    finished: bool,
}

impl Drop for StartupConstructionGuard {
    fn drop(&mut self) {
        if !self.finished {
            self.owner.record_primary(startup_task_failed());
            let runtime_cleanup = {
                let mut retained = self
                    .owner
                    .failed
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                let action = retained.as_mut().and_then(|cleanup| {
                    if cleanup.runtime_cleanup_available {
                        if let StartupResource::Runtime {
                            _runtime,
                            _log_activity,
                        } = &cleanup._resource
                        {
                            let action = (Arc::clone(_runtime), _log_activity.clone());
                            cleanup.runtime_cleanup_available = false;
                            cleanup.safe_runtime_interrupted = true;
                            cleanup.host_close_pending = true;
                            return Some(action);
                        }
                    }
                    cleanup.construction_interrupted = true;
                    None
                });
                self.owner
                    .construction_finished
                    .store(true, Ordering::Release);
                self.owner.outcome.lock().begin_cleanup();
                action
            };
            if let Some((runtime, log_activity)) = runtime_cleanup {
                self.owner.start_runtime_cleanup(runtime, log_activity);
            } else {
                self.owner.settle_construction_after_work();
            }
        }
    }
}

impl Engine {
    pub(super) async fn start_owned(
        config: EngineConfig,
        host: Arc<ReusableHost>,
        progress: StartupProgressInput,
        lifecycle: StartupLifecycleInput,
        mut unpolled: UnpolledStartupCleanup,
    ) -> Result<(Self, EventStream), EngineError> {
        match Handle::try_current() {
            Ok(_) => {}
            Err(source) => {
                let error = startup_task_failed();
                lifecycle.cleanup.record_executor_error(source);
                lifecycle.cleanup.record_primary(error.clone());
                lifecycle.requests.fail_startup(error.clone());
                progress.finish_cleanup_failed(&error);
                return Err(error);
            }
        }
        let executor = match OwnedExecutor::start() {
            Ok(executor) => Arc::new(executor),
            Err(source) => {
                let error = startup_task_failed();
                lifecycle.cleanup.record_owned_start_error(source);
                lifecycle.cleanup.record_primary(error.clone());
                lifecycle.requests.fail_startup(error.clone());
                progress.finish_cleanup_failed(&error);
                return Err(error);
            }
        };
        lifecycle
            .cleanup
            .register_owned_executor(Arc::clone(&executor));
        let engine_executor = Arc::clone(&executor);
        let (sender, receiver) = oneshot::channel();
        let task = async move {
            // 只有启动任务开始运行后，才接管宿主清理责任。
            unpolled.disarm();
            let mut construction = StartupConstructionGuard {
                owner: Arc::clone(&lifecycle.cleanup),
                finished: false,
            };
            let result = Self::start_runtime(
                config,
                host,
                &progress,
                lifecycle.requests.clone(),
                Arc::clone(&lifecycle.cleanup),
                engine_executor,
            )
            .await;
            construction.finished = true;
            let result = match result {
                Ok(parts) => Ok(StartupHandoff::new(
                    parts,
                    progress,
                    Arc::clone(&lifecycle.cleanup),
                )),
                Err(error) => {
                    lifecycle.requests.fail_startup(error.clone());
                    if lifecycle.cleanup.unconfirmed() {
                        lifecycle.cleanup.record_primary(error.clone());
                        lifecycle.cleanup.finish_construction_failure(
                            EngineError::new(1108, EngineErrorCategory::Internal, false),
                            progress,
                            error.clone(),
                        );
                    } else {
                        progress.finish(&Err::<(), _>(error.clone()));
                    }
                    Err(error)
                }
            };
            // 发送成功也可能尚未被接收；交接守卫持续持有关闭责任直到调用方实际取走。
            let _ = sender.send(result);
        };
        if let Err(task) = executor.spawn(task) {
            drop(task);
            return Err(startup_task_failed());
        }
        let handoff = receiver
            .await
            // discarded-source[channel]: `tokio::sync::oneshot::error::RecvError`: the error only means the peer is gone or carries the unsent payload, which must not outlive it
            .map_err(|_| startup_task_failed_recorded())??;
        handoff.claim()
    }
}

struct StartupHandoff {
    parts: Option<(Engine, EventStream)>,
    progress: Option<StartupProgressInput>,
    cleanup: Arc<StartupCleanupOwner>,
}

impl StartupHandoff {
    fn new(
        parts: (Engine, EventStream),
        progress: StartupProgressInput,
        cleanup: Arc<StartupCleanupOwner>,
    ) -> Self {
        Self {
            parts: Some(parts),
            progress: Some(progress),
            cleanup,
        }
    }

    fn claim(mut self) -> Result<(Engine, EventStream), EngineError> {
        let parts = self.parts.take().ok_or_else(startup_task_failed)?;
        if let Some(progress) = self.progress.take() {
            progress.finish(&Ok::<(), EngineError>(()));
        }
        self.cleanup.deliver();
        Ok(parts)
    }
}

impl Drop for StartupHandoff {
    fn drop(&mut self) {
        let Some((engine, events)) = self.parts.take() else {
            return;
        };
        let progress = self.progress.take();
        let engine = self.cleanup.accept(engine, events);
        let task = StartupCleanupTask {
            owner: Arc::clone(&self.cleanup),
            engine,
            progress,
            finished: false,
        };
        // 未交出的实例由独立执行器清理，不依赖构造执行器的寿命。
        let pending = Arc::new(Mutex::new(Some(task)));
        let worker_task = Arc::clone(&pending);
        #[cfg(feature = "dev-tools")]
        let injected = PROBE_ENGINE_CLEANUP_FAILURE
            .compare_exchange(1, 0, Ordering::AcqRel, Ordering::Acquire)
            .is_ok();
        #[cfg(not(feature = "dev-tools"))]
        let injected = false;
        let worker = if injected {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "controlled thread creation failure",
            ))
        } else {
            std::thread::Builder::new()
                .name("uc-startup-engine-close".to_owned())
                .spawn(move || {
                    let task = worker_task
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .take();
                    if let Some(task) = task {
                        task.run();
                    }
                })
        };
        if let Err(source) = worker {
            self.cleanup
                .record_cleanup_worker_error(UnpolledCleanupError::Thread(source));
            // 在线程创建失败后，先记录来源，再发布未确认结果。
            drop(
                pending
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take(),
            );
        }
    }
}

fn startup_task_failed_recorded() -> EngineError {
    record_task_join_failure(DiagnosticTaskKind::EngineStartup);
    startup_task_failed()
}

fn startup_task_failed() -> EngineError {
    EngineError::new(1108, EngineErrorCategory::Internal, true)
}
