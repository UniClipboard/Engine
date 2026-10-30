use std::io;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use tokio::sync::oneshot;

use uc_engine::{Engine, EngineError, EngineErrorCategory, ResourceReleaseOutcome};

use crate::{BindingError, BindingResourceRelease};

#[cfg(feature = "test-binding-worker-panic")]
static PROBE_SETTLEMENT_RETRY_FAILURE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(true);

/// 绑定层交付前持有 Engine。失败时保留同一关闭负责人及其结果。
#[derive(Default)]
pub(super) struct StartupDeliveryOwner {
    retained: Mutex<Option<RetainedDelivery>>,
    outcome: Mutex<DeliveryOutcome>,
    outcome_changed: Condvar,
    worker_failures: Mutex<Vec<RecoveryWorkerFailure>>,
    #[cfg(feature = "test-binding-worker-panic")]
    recovery_close_started: std::sync::atomic::AtomicBool,
}

/// 仅描述绑定交付阶段；完整启动结果还须合并 Engine 启动负责人。
#[derive(Clone, Default)]
#[allow(dead_code, reason = "完整启动结果接口尚未公开")]
pub(super) enum DeliveryOutcome {
    #[default]
    NotAccepted,
    Pending,
    Delivered,
    Released {
        error: Option<BindingError>,
        worker_error: Option<BindingError>,
    },
    Failed {
        error: BindingError,
        worker_error: Option<BindingError>,
        operation_error: Option<BindingError>,
        explicit_retry_supported: bool,
    },
}

#[allow(dead_code, reason = "保留原始错误供完整启动结果读取")]
enum RecoveryWorkerFailure {
    Thread(io::Error),
    Runtime(io::Error),
    ExecutorPanicked,
    ObserverTerminated,
}

struct RetainedDelivery {
    _engine: Arc<Engine>,
    _error: Option<BindingError>,
    worker_error: Option<BindingError>,
    _owner: Arc<StartupDeliveryOwner>,
    cleanup_started: bool,
    worker_lost: bool,
    cleanup_result: Option<BindingResourceRelease>,
    pending_waiter: Option<oneshot::Sender<ResourceReleaseOutcome>>,
    settlement_retry_in_progress: bool,
    creation_failed: bool,
}

impl StartupDeliveryOwner {
    pub(super) fn accept(self: &Arc<Self>, engine: Arc<Engine>) {
        *self.retained.lock().unwrap_or_else(PoisonError::into_inner) = Some(RetainedDelivery {
            _engine: engine,
            _error: None,
            worker_error: None,
            _owner: Arc::clone(self),
            cleanup_started: false,
            worker_lost: false,
            cleanup_result: None,
            pending_waiter: None,
            settlement_retry_in_progress: false,
            creation_failed: false,
        });
        *self.outcome.lock().unwrap_or_else(PoisonError::into_inner) = DeliveryOutcome::Pending;
        self.outcome_changed.notify_all();
    }

    pub(super) fn delivered(&self) {
        let mut outcome = self.outcome.lock().unwrap_or_else(PoisonError::into_inner);
        if matches!(*outcome, DeliveryOutcome::Pending) {
            *outcome = DeliveryOutcome::Delivered;
            self.outcome_changed.notify_all();
        }
    }

    pub(super) fn delivery_lost(&self, error: BindingError) {
        if let Some(retained) = self
            .retained
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_mut()
        {
            retained.worker_error.get_or_insert(error);
        }
    }

    #[allow(dead_code, reason = "完整启动结果接口尚未公开")]
    pub(super) fn wait_internal(&self, budget: Duration) -> DeliveryOutcome {
        let started = Instant::now();
        let mut outcome = self.outcome.lock().unwrap_or_else(PoisonError::into_inner);
        while matches!(
            *outcome,
            DeliveryOutcome::NotAccepted | DeliveryOutcome::Pending
        ) {
            let remaining = budget.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                break;
            }
            outcome = self
                .outcome_changed
                .wait_timeout(outcome, remaining.min(Duration::from_secs(60)))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        outcome.clone()
    }

    fn recover_after_worker_panic(self: &Arc<Self>, error: BindingError) {
        let (engine, settled) = {
            let mut state = self.retained.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(retained) = state.as_mut() else {
                return;
            };
            retained.worker_lost = true;
            retained.worker_error.get_or_insert_with(|| error.clone());
            if retained.cleanup_started {
                (None, retained.cleanup_result.clone())
            } else {
                (Some(Arc::clone(&retained._engine)), None)
            }
        };
        if let Some(outcome) = settled {
            self.finish(&outcome);
        } else if let Some(engine) = engine {
            let _ = self.spawn_cleanup(engine, None, error);
        }
    }

    pub(super) async fn shutdown_owned(
        self: &Arc<Self>,
        engine: Arc<Engine>,
        grace: Option<Duration>,
        error: BindingError,
    ) -> ResourceReleaseOutcome {
        let receiver = match self.spawn_cleanup(engine, grace, error) {
            Some(receiver) => receiver,
            None => return failed_cleanup(),
        };
        receiver.await.unwrap_or_else(|_| failed_cleanup())
    }

    fn spawn_cleanup(
        self: &Arc<Self>,
        engine: Arc<Engine>,
        grace: Option<Duration>,
        error: BindingError,
    ) -> Option<oneshot::Receiver<ResourceReleaseOutcome>> {
        {
            let mut state = self.retained.lock().unwrap_or_else(PoisonError::into_inner);
            let retained = state.as_mut()?;
            retained.cleanup_started = true;
            retained.cleanup_result = None;
        }
        let (sender, receiver) = oneshot::channel();
        let owner = Arc::clone(self);
        let worker_error = error.clone();
        #[cfg(feature = "test-binding-worker-panic")]
        if inject_recovery_failure("thread") {
            self.record_worker_failure(
                RecoveryWorkerFailure::Thread(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "controlled thread failure",
                )),
                error,
            );
            return None;
        }
        let result = std::thread::Builder::new()
            .name("uc-binding-recovery".to_owned())
            .spawn(move || {
                let runtime = match std::panic::catch_unwind(recovery_runtime) {
                    Ok(Ok(runtime)) => runtime,
                    Ok(Err(source)) => {
                        owner.record_worker_failure(
                            RecoveryWorkerFailure::Runtime(source),
                            worker_error,
                        );
                        let _ = sender.send(failed_cleanup());
                        return;
                    }
                    Err(_) => {
                        owner.record_terminal_worker_failure(worker_error);
                        let _ = sender.send(failed_cleanup());
                        return;
                    }
                };
                #[cfg(feature = "test-binding-worker-panic")]
                owner
                    .recovery_close_started
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    runtime.block_on(async {
                        let shutdown = async {
                            match grace {
                                Some(grace) => {
                                    engine.shutdown_until_released_with_grace(grace).await
                                }
                                None => engine.shutdown_until_released().await,
                            }
                        };
                        #[cfg(feature = "test-binding-worker-panic")]
                        if let Some(marker) =
                            std::env::var_os("UC_TEST_BINDING_CLEANUP_OUTER_PANIC")
                        {
                            tokio::select! {
                                outcome = shutdown => return outcome,
                                _ = async {
                                    while !std::path::Path::new(&marker).is_file() {
                                        tokio::time::sleep(Duration::from_millis(10)).await;
                                    }
                                } => panic!("controlled cleanup worker panic"),
                            }
                        }
                        shutdown.await
                    })
                }));
                drop(runtime);
                let outcome = match outcome {
                    Ok(outcome) => outcome,
                    Err(_) => {
                        let observer = std::panic::catch_unwind(|| {
                            #[cfg(feature = "test-binding-worker-panic")]
                            if let Some(failure) = std::env::var_os("UC_TEST_BINDING_OBSERVER_FAIL")
                            {
                                if failure == std::ffi::OsStr::new("panic") {
                                    panic!("controlled observer construction panic");
                                }
                                if failure != std::ffi::OsStr::new("drop-panic") && failure != std::ffi::OsStr::new("run-panic") && failure != std::ffi::OsStr::new("polled-panic") {
                                    return Err(io::Error::new(
                                        io::ErrorKind::PermissionDenied,
                                        "controlled observer failure",
                                    ));
                                }
                            }
                            recovery_runtime()
                        });
                        let observer = match observer {
                            Ok(Ok(observer)) => observer,
                            Ok(Err(source)) => {
                                owner.retain_pending_worker_failure(
                                    RecoveryWorkerFailure::Runtime(source),
                                    sender,
                                );
                                return;
                            }
                            Err(_) => {
                                owner.retain_pending_worker_failure(
                                    RecoveryWorkerFailure::ObserverTerminated,
                                    sender,
                                );
                                return;
                            }
                        };
                        let observed =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                let outcome = observer.block_on(async {
                                    #[cfg(feature = "test-binding-worker-panic")]
                                    if std::env::var_os("UC_TEST_BINDING_OBSERVER_FAIL").as_deref()
                                        == Some(std::ffi::OsStr::new("run-panic"))
                                    {
                                        panic!("controlled observer execution panic");
                                    }
                                #[cfg(feature = "test-binding-worker-panic")]
                                if std::env::var_os("UC_TEST_BINDING_OBSERVER_FAIL").as_deref() == Some(std::ffi::OsStr::new("polled-panic")) {
                                    let marker = std::env::var_os("UC_TEST_BINDING_OBSERVER_POLL_DIR").map(std::path::PathBuf::from);
                                    if let Some(marker) = marker {
                                        let settle = engine.settle_terminated_shutdown();
                                        tokio::pin!(settle);
                                        tokio::select! {
                                            biased;
                                            outcome = &mut settle => return outcome,
                                            _ = async {
                                                let _ = std::fs::write(marker.join("observer-polled"), b"polled");
                                                while !marker.join("panic-observer").is_file() {
                                                    tokio::time::sleep(Duration::from_millis(10)).await;
                                                }
                                            } => panic!("controlled observer panic after settlement poll"),
                                        }
                                    }
                                }
                                    engine.settle_terminated_shutdown().await
                                });
                                #[cfg(feature = "test-binding-worker-panic")]
                                if std::env::var_os("UC_TEST_BINDING_OBSERVER_FAIL").as_deref() == Some(std::ffi::OsStr::new("drop-panic")) {
                                    drop(ObserverDropProbe { _runtime: observer });
                                    return outcome;
                                }
                                drop(observer);
                                outcome
                            }));
                        match observed {
                            Ok(outcome) => outcome,
                            Err(_) => {
                                owner.retain_pending_worker_failure(
                                    RecoveryWorkerFailure::ObserverTerminated,
                                    sender,
                                );
                                return;
                            }
                        }
                    }
                };
                owner.record_owned_outcome(BindingResourceRelease::from(outcome.clone()));
                let _ = sender.send(outcome);
            });
        if let Err(source) = result {
            self.record_worker_failure(RecoveryWorkerFailure::Thread(source), error);
            return None;
        }
        Some(receiver)
    }

    fn retain_pending_worker_failure(
        &self,
        source: RecoveryWorkerFailure,
        sender: oneshot::Sender<ResourceReleaseOutcome>,
    ) {
        self.worker_failures
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(source);
        if let Some(retained) = self
            .retained
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_mut()
        {
            retained.pending_waiter = Some(sender);
        }
    }

    /// 只重试已接受关闭的结算观察，不重新执行关闭器。
    #[allow(dead_code, reason = "完整启动结果接口尚未公开")]
    pub(super) fn retry_settlement(self: &Arc<Self>) -> bool {
        let engine = {
            let mut state = self.retained.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(retained) = state.as_mut() else {
                return false;
            };
            if retained.pending_waiter.is_none() || retained.settlement_retry_in_progress {
                return false;
            }
            retained.settlement_retry_in_progress = true;
            Arc::clone(&retained._engine)
        };
        let owner = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("uc-binding-settlement".to_owned())
            .spawn(move || {
                let observation = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let runtime = settlement_runtime().map_err(RecoveryWorkerFailure::Runtime)?;
                    let result = runtime.block_on(engine.settle_terminated_shutdown());
                    drop(runtime);
                    Ok::<_, RecoveryWorkerFailure>(result)
                }));
                let outcome = match observation {
                    Ok(Ok(outcome)) => outcome,
                    Ok(Err(source)) => {
                        owner.settlement_retry_failed(source);
                        return;
                    }
                    Err(_) => {
                        owner.settlement_retry_failed(RecoveryWorkerFailure::ObserverTerminated);
                        return;
                    }
                };
                let sender = {
                    let mut state = owner
                        .retained
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner);
                    let Some(retained) = state.as_mut() else {
                        return;
                    };
                    retained.settlement_retry_in_progress = false;
                    retained.pending_waiter.take()
                };
                owner.record_owned_outcome(BindingResourceRelease::from(outcome.clone()));
                if let Some(sender) = sender {
                    let _ = sender.send(outcome);
                }
            });
        if let Err(source) = spawned {
            self.settlement_retry_failed(RecoveryWorkerFailure::Thread(source));
            return false;
        }
        true
    }

    fn settlement_retry_failed(&self, source: RecoveryWorkerFailure) {
        self.worker_failures
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(source);
        if let Some(retained) = self
            .retained
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_mut()
        {
            retained.settlement_retry_in_progress = false;
        }
    }

    fn record_owned_outcome(&self, outcome: BindingResourceRelease) {
        let worker_lost = {
            let mut state = self.retained.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(retained) = state.as_mut() else {
                return;
            };
            retained.cleanup_result = Some(outcome.clone());
            retained.worker_lost
        };
        if worker_lost {
            self.finish(&outcome);
        }
    }

    fn record_worker_failure(&self, source: RecoveryWorkerFailure, error: BindingError) {
        self.worker_failures
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(source);
        if let Some(retained) = self
            .retained
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_mut()
        {
            retained.creation_failed = true;
        }
        self.record_owned_outcome(BindingResourceRelease::Failed { error });
    }

    fn record_terminal_worker_failure(&self, error: BindingError) {
        self.worker_failures
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(RecoveryWorkerFailure::ExecutorPanicked);
        self.record_owned_outcome(BindingResourceRelease::Failed { error });
    }

    /// 只在关闭尚未接受时重试创建工作线程。
    #[allow(dead_code, reason = "完整启动结果接口尚未公开")]
    pub(super) fn retry_creation(self: &Arc<Self>) -> bool {
        let (engine, error) = {
            let mut retained = self.retained.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(retained) = retained.as_mut() else {
                return false;
            };
            if !retained.creation_failed {
                return false;
            }
            let Some(error) = retained.worker_error.clone() else {
                return false;
            };
            let mut outcome = self.outcome.lock().unwrap_or_else(PoisonError::into_inner);
            if !matches!(
                *outcome,
                DeliveryOutcome::Failed {
                    explicit_retry_supported: true,
                    ..
                }
            ) {
                return false;
            }
            retained.creation_failed = false;
            retained.worker_lost = true;
            *outcome = DeliveryOutcome::Pending;
            self.outcome_changed.notify_all();
            (Arc::clone(&retained._engine), error)
        };
        self.spawn_cleanup(engine, None, error).is_some()
    }

    #[cfg(feature = "test-binding-worker-panic")]
    pub(super) fn probe_worker_failure(&self) -> Option<(String, String, bool)> {
        self.worker_failures
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .first()
            .map(|source| {
                let (stage, error) = match source {
                    RecoveryWorkerFailure::Thread(error) => ("thread", error),
                    RecoveryWorkerFailure::Runtime(error) => ("runtime", error),
                    RecoveryWorkerFailure::ExecutorPanicked => {
                        return (
                            "executor".to_owned(),
                            "panicked".to_owned(),
                            self.recovery_close_started
                                .load(std::sync::atomic::Ordering::SeqCst),
                        );
                    }
                    RecoveryWorkerFailure::ObserverTerminated => {
                        return (
                            "observer".to_owned(),
                            "terminated".to_owned(),
                            self.recovery_close_started
                                .load(std::sync::atomic::Ordering::SeqCst),
                        )
                    }
                };
                (
                    stage.to_owned(),
                    format!("{:?}", error.kind()),
                    self.recovery_close_started
                        .load(std::sync::atomic::Ordering::SeqCst),
                )
            })
    }

    #[cfg(feature = "test-binding-worker-panic")]
    pub(super) fn probe_retained(&self) -> bool {
        self.retained
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    pub(super) fn finish(&self, outcome: &BindingResourceRelease) {
        let mut retained = self.retained.lock().unwrap_or_else(PoisonError::into_inner);
        let worker_error = retained
            .as_ref()
            .and_then(|retained| retained.worker_error.clone());
        let prior_error = retained
            .as_ref()
            .and_then(|retained| retained._error.clone());
        let explicit_retry_supported = retained
            .as_ref()
            .is_some_and(|retained| retained.creation_failed && retained.worker_error.is_some());
        match outcome {
            BindingResourceRelease::Released { .. } => {
                retained.take();
            }
            BindingResourceRelease::Failed { error } => {
                if let Some(retained) = retained.as_mut() {
                    retained._error.get_or_insert_with(|| error.clone());
                }
            }
            BindingResourceRelease::Pending => {}
        }
        drop(retained);
        let mut current = self.outcome.lock().unwrap_or_else(PoisonError::into_inner);
        if !matches!(
            *current,
            DeliveryOutcome::NotAccepted
                | DeliveryOutcome::Delivered
                | DeliveryOutcome::Released { .. }
        ) {
            match outcome {
                BindingResourceRelease::Released { error } => {
                    *current = DeliveryOutcome::Released {
                        error: error.clone().or(prior_error),
                        worker_error,
                    }
                }
                BindingResourceRelease::Failed { error } => {
                    *current = DeliveryOutcome::Failed {
                        error: error.clone(),
                        worker_error,
                        operation_error: prior_error,
                        explicit_retry_supported,
                    }
                }
                BindingResourceRelease::Pending => {}
            }
            self.outcome_changed.notify_all();
        }
    }
}

#[cfg(feature = "test-binding-worker-panic")]
struct ObserverDropProbe {
    _runtime: tokio::runtime::Runtime,
}

#[cfg(feature = "test-binding-worker-panic")]
impl Drop for ObserverDropProbe {
    fn drop(&mut self) {
        panic!("controlled observer destructor panic");
    }
}

fn failed_cleanup() -> ResourceReleaseOutcome {
    ResourceReleaseOutcome::Failed {
        error: EngineError::new(1108, EngineErrorCategory::Internal, false),
    }
}

fn settlement_runtime() -> io::Result<tokio::runtime::Runtime> {
    #[cfg(feature = "test-binding-worker-panic")]
    if std::env::var_os("UC_TEST_BINDING_SETTLEMENT_RETRY_FAIL").is_some()
        && PROBE_SETTLEMENT_RETRY_FAILURE.swap(false, std::sync::atomic::Ordering::SeqCst)
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "controlled settlement retry failure",
        ));
    }
    recovery_runtime()
}

fn recovery_runtime() -> io::Result<tokio::runtime::Runtime> {
    #[cfg(feature = "test-binding-worker-panic")]
    if inject_recovery_failure("executor-panic") {
        panic!("controlled recovery executor construction panic");
    }
    #[cfg(feature = "test-binding-worker-panic")]
    if inject_recovery_failure("runtime") {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "controlled runtime failure",
        ));
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
}

#[cfg(feature = "test-binding-worker-panic")]
fn inject_recovery_failure(stage: &str) -> bool {
    std::env::var_os("UC_TEST_BINDING_RECOVERY_FAIL").as_deref()
        == Some(std::ffi::OsStr::new(stage))
        && std::env::var_os("UC_TEST_BINDING_RECOVERY_ALLOW")
            .is_none_or(|path| !std::path::Path::new(&path).exists())
}

/// 工作线程异常终止时，仅为尚未开始关闭的 Engine 接受一次清理。
pub(super) struct StartupWorkerGuard {
    owner: Arc<StartupDeliveryOwner>,
    error: BindingError,
}

impl StartupWorkerGuard {
    pub(super) fn new(owner: Arc<StartupDeliveryOwner>, error: BindingError) -> Self {
        Self { owner, error }
    }
}

impl Drop for StartupWorkerGuard {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.owner.recover_after_worker_panic(self.error.clone());
        }
    }
}
