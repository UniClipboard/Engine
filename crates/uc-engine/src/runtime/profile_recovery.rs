use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex as StdMutex, MutexGuard as StdMutexGuard};

use async_trait::async_trait;
use tokio::sync::{Mutex, Notify, RwLock};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use uc_application::deps::{LifecycleError, StopProfileRuntimePort};
use uc_application::facade::ProfileFactoryResetFacade;
use uc_core::app_dirs::HostConfigurationFileError;
use uc_core::crypto::domain::Passphrase;
use uc_core::ports::{SecureStorageError, SecureStoragePort};
use uc_infra_profile::security::{
    ProfileKeyRecoveryError, ProfileKeyRecoveryStore, ProfileRecoveryLosses,
    ProfileRecoveryOutcome, ProfileRecoveryPreparation,
};
use uc_observability_contract::{
    error_source::io_error_kind, log_fields::log_vocab_debug, uc_info, uc_warn,
};

use super::reusable_host::ReusableHost;
use super::session_supervisor::lifecycle_error;
use super::{profile_recovery_required_error, startup_error, ProductionRuntime};
use crate::assembly::host::{
    derive_app_paths, profile_key_recovery_store, wire_host_capabilities_with_emitter,
    EngineHostEventEmitter,
};
use crate::engine::event_stream::EventSender;
use crate::engine::startup::StartupProgressStore;
use crate::engine::startup_owner::StartupCleanupOwner;
use crate::engine::EngineRuntime;
use crate::error_codes::{
    FACTORY_RESET_RESTART_REQUIRED_CODE, HOST_CONFIGURATION_FILES_INVALID_CODE,
    HOST_LOG_PROFILE_INVALID_CODE, PROFILE_RECOVERY_PARTIAL_CODE,
    PROFILE_RECOVERY_PERSISTENCE_FAILED_CODE, PROFILE_RECOVERY_UNSUPPORTED_CODE,
    UNLOCK_SPACE_CORRUPTED_CODE, UNLOCK_SPACE_UNAUTHORIZED_CODE,
};
use crate::operations::space::factory_reset::execute_factory_reset_space;
use crate::{
    AdmissionRecoverySummary, EncryptionStateSummary, EngineConfig, EngineError,
    EngineErrorCategory, EngineEvent, HostCapabilities, HostCapabilityError,
    HostCapabilityErrorCategory, HostSecureStorage, Operation, OperationKind, OperationResult,
    ProfileRecoveryLoss, ProfileRecoveryState, ProfileRecoverySummary, StartupProgress,
};
#[cfg(feature = "dev-tools")]
use crate::{DevOperation, DevOperationResult};

#[derive(Clone)]
enum RuntimeMode {
    Stopped,
    Ready {
        runtime: Arc<ProductionRuntime>,
        recovered: bool,
    },
    Recovery(Arc<RecoveryBootstrap>),
    AdmissionRecovery {
        summary: AdmissionRecoverySummary,
        /// 启动后才发现时保留已启动的运行期，关闭时仍须完整释放。
        runtime: Option<Arc<ProductionRuntime>>,
    },
    /// 离开空间已清除资料，但新的运行期没能装配；只能查询恢复状态与关闭，宿主须重启 Engine。
    RestartRequired {
        /// 重置后尚未确认释放干净的旧运行期，关闭时仍须完整释放。
        runtime: Option<Arc<ProductionRuntime>>,
    },
}

struct RecoveryBootstrap {
    /// 口令恢复或恢复出厂只能各自消费一次启动输入；消费后只能重启。
    input_available: Mutex<bool>,
    gate: Mutex<()>,
    summary: StdMutex<ProfileRecoverySummary>,
    progress: Arc<StartupProgressStore>,
    recovered_runtime: StdMutex<Option<Arc<ProductionRuntime>>>,
    construction: StdMutex<Option<Arc<StartupCleanupOwner>>>,
    construction_inflight: AtomicBool,
    construction_interrupted: AtomicBool,
    construction_changed: Notify,
    construction_blocking_changed: Condvar,
    construction_blocking_gate: StdMutex<()>,
}

/// 构造尚未返回时也保留它已登记的资源和回滚负责人。
struct RecoveryConstructionGuard {
    bootstrap: Arc<RecoveryBootstrap>,
    owner: Arc<StartupCleanupOwner>,
    events: EventSender,
    completed: bool,
}

impl RecoveryConstructionGuard {
    fn new(
        bootstrap: Arc<RecoveryBootstrap>,
        host: Arc<ReusableHost>,
        events: EventSender,
    ) -> Self {
        let owner = Arc::new(StartupCleanupOwner::default());
        owner.accept_host(host);
        *bootstrap.lock_construction() = Some(Arc::clone(&owner));
        {
            let _gate = bootstrap
                .construction_blocking_gate
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            bootstrap
                .construction_inflight
                .store(true, Ordering::Release);
        }
        Self {
            bootstrap,
            owner,
            events,
            completed: false,
        }
    }

    fn handoff(mut self, runtime: Arc<ProductionRuntime>) -> RecoveryRuntimeGuard {
        let guard =
            RecoveryRuntimeGuard::new(Arc::clone(&self.bootstrap), runtime, self.events.clone());
        self.owner.deliver();
        *self.bootstrap.lock_construction() = None;
        self.completed = true;
        self.bootstrap.finish_construction();
        guard
    }

    fn handoff_mode(mut self) {
        self.owner.deliver();
        *self.bootstrap.lock_construction() = None;
        self.completed = true;
        self.bootstrap.finish_construction();
    }

    fn finish_without_runtime(mut self) -> bool {
        let settled = self.owner.settle_recovery_construction();
        if settled {
            *self.bootstrap.lock_construction() = None;
        }
        self.completed = true;
        self.bootstrap.finish_construction();
        settled
    }
}

impl Drop for RecoveryConstructionGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.bootstrap
                .construction_interrupted
                .store(true, Ordering::Release);
            let summary = {
                let mut summary = self.bootstrap.lock_summary();
                summary.state = ProfileRecoveryState::Failed;
                summary.can_submit_passphrase = false;
                summary.restart_required = true;
                summary.background_ready = false;
                summary.clone()
            };
            publish_profile_recovery(&self.events, summary);
            self.bootstrap.finish_construction();
        }
    }
}

struct RecoveryRuntimeGuard {
    bootstrap: Arc<RecoveryBootstrap>,
    events: EventSender,
    completed: bool,
}

impl RecoveryRuntimeGuard {
    fn new(
        bootstrap: Arc<RecoveryBootstrap>,
        runtime: Arc<ProductionRuntime>,
        events: EventSender,
    ) -> Self {
        *bootstrap.lock_recovered_runtime() = Some(runtime);
        Self {
            bootstrap,
            events,
            completed: false,
        }
    }

    fn complete(mut self) {
        *self.bootstrap.lock_recovered_runtime() = None;
        self.completed = true;
    }
}

impl Drop for RecoveryRuntimeGuard {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        let summary = {
            let mut summary = self.bootstrap.lock_summary();
            summary.state = ProfileRecoveryState::Failed;
            summary.can_submit_passphrase = false;
            summary.restart_required = true;
            summary.background_ready = false;
            summary.clone()
        };
        publish_profile_recovery(&self.events, summary);
    }
}

pub(crate) struct RecoverableRuntime {
    mode: RwLock<RuntimeMode>,
    config: EngineConfig,
    host: Arc<ReusableHost>,
    recovery: Arc<ProfileKeyRecoveryStore>,
    ready_summary_override: StdMutex<Option<ProfileRecoverySummary>>,
    reconstruction: StdMutex<Option<Arc<RecoveryBootstrap>>>,
    events: EventSender,
}

impl RecoverableRuntime {
    pub(crate) async fn start(
        config: EngineConfig,
        host: Arc<ReusableHost>,
        events: EventSender,
        progress: Arc<StartupProgressStore>,
        cleanup: Arc<StartupCleanupOwner>,
    ) -> Result<Self, EngineError> {
        // 声明在任何资料读写之前校验：导入恢复和恢复出厂都只按校验后的名字处理宿主文件。
        if let Err(error) = config.validated_host_configuration_files() {
            let error_kind = match error {
                HostConfigurationFileError::InvalidName => "invalid_name",
                HostConfigurationFileError::Reserved => "reserved_name",
                HostConfigurationFileError::Duplicate => "duplicate_name",
            };
            uc_warn!(
                error_kind = error_kind,
                "host configuration file declaration rejected"
            );
            // discarded-source[core-pure-validation]: the Core rule is fully classified by the variant logged above
            let error = EngineError::new(
                HOST_CONFIGURATION_FILES_INVALID_CODE,
                EngineErrorCategory::InvalidInput,
                false,
            );
            return Err(finish_without_runtime_start(&host, &cleanup, error).await);
        }
        // 日志档位名称会进入诊断状态与导出清单，同样在任何资料读写之前校验。
        if config.validated_host_log_profile().is_err() {
            uc_warn!(
                error_kind = "invalid_label",
                "host log profile declaration rejected"
            );
            // discarded-source[core-pure-validation]: the Core rule has a single failure, fully classified above
            let error = EngineError::new(
                HOST_LOG_PROFILE_INVALID_CODE,
                EngineErrorCategory::InvalidInput,
                false,
            );
            return Err(finish_without_runtime_start(&host, &cleanup, error).await);
        }
        let paths = derive_app_paths(host.directories());
        let recovery = profile_key_recovery_store(&config, &paths, &host.capabilities());
        let mode = match start_profile_mode(
            &config,
            &host,
            &events,
            &progress,
            &recovery,
            Some(&cleanup),
        )
        .await
        {
            Ok(mode) => mode,
            Err(error) => return Err(finish_without_runtime_start(&host, &cleanup, error).await),
        };
        Ok(Self {
            mode: RwLock::new(mode),
            config,
            host,
            recovery,
            ready_summary_override: StdMutex::new(None),
            reconstruction: StdMutex::new(None),
            events,
        })
    }

    fn reconstruction(&self) -> Option<Arc<RecoveryBootstrap>> {
        self.reconstruction
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    async fn settle_reconstruction(&self) -> bool {
        let Some(bootstrap) = self.reconstruction() else {
            return true;
        };
        bootstrap.wait_for_construction().await;
        let owner = bootstrap.lock_construction().clone();
        if let Some(owner) = owner {
            if !owner.wait_recovery_backup().await
                || bootstrap.construction_interrupted.load(Ordering::Acquire)
                || !owner.settle_recovery_construction()
            {
                return false;
            }
            *bootstrap.lock_construction() = None;
        }
        if bootstrap.construction_interrupted.load(Ordering::Acquire) {
            return false;
        }
        let mut current = self
            .reconstruction
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if current
            .as_ref()
            .is_some_and(|value| Arc::ptr_eq(value, &bootstrap))
        {
            *current = None;
        }
        true
    }

    async fn mode(&self) -> RuntimeMode {
        let mode = self.mode.read().await.clone();
        let RuntimeMode::Ready { runtime, .. } = &mode else {
            return mode;
        };
        match runtime.admission_recovery() {
            Some(summary) => {
                self.enter_admission_recovery(summary, Some(Arc::clone(runtime)))
                    .await
            }
            None => mode,
        }
    }

    /// 转入受限模式只发布一次；之后同一进程只允许查询与关闭，重建会话不被当作修复。
    async fn enter_admission_recovery(
        &self,
        summary: AdmissionRecoverySummary,
        runtime: Option<Arc<ProductionRuntime>>,
    ) -> RuntimeMode {
        let mut mode = self.mode.write().await;
        if !matches!(*mode, RuntimeMode::AdmissionRecovery { .. }) {
            *mode = RuntimeMode::AdmissionRecovery { summary, runtime };
            publish_profile_recovery(&self.events, admission_summary(summary));
        }
        mode.clone()
    }

    /// 启动后重建会话失败时，若已证实准入资料无法读取，改报需要恢复而不是可重试的会话错误。
    async fn restrict_after_failure<T>(
        &self,
        runtime: &Arc<ProductionRuntime>,
        result: Result<T, EngineError>,
    ) -> Result<T, EngineError> {
        let Err(error) = result else {
            return result;
        };
        match runtime.admission_recovery() {
            Some(summary) => {
                self.enter_admission_recovery(summary, Some(Arc::clone(runtime)))
                    .await;
                Err(profile_recovery_required_error())
            }
            None => Err(error),
        }
    }

    async fn execute_ready(
        &self,
        runtime: Arc<ProductionRuntime>,
        recovered: bool,
        operation: Operation,
        cancellation: CancellationToken,
    ) -> Result<OperationResult, EngineError> {
        if matches!(operation, Operation::QueryProfileRecovery) {
            let summary = self
                .lock_ready_summary_override()
                .clone()
                .unwrap_or_else(|| ready_summary(recovered, self.recovery.cleanup_pending()));
            return Ok(OperationResult::ProfileRecovery(summary));
        }
        let kind = operation.kind();
        let result = runtime.execute(operation, cancellation).await?;
        if matches!(
            kind,
            OperationKind::CreateSpace
                | OperationKind::JoinSpace
                | OperationKind::UnlockSpace
                | OperationKind::ResetSpace
        ) {
            match self.recovery.refresh_after_authentication().await {
                Ok(()) => {
                    if self.lock_ready_summary_override().take().is_some() {
                        publish_profile_recovery(
                            &self.events,
                            ready_summary(recovered, self.recovery.cleanup_pending()),
                        );
                    }
                }
                Err(error) => {
                    uc_warn!(
                        error_kind = "profile_recovery_refresh",
                        io_error_kind = io_error_kind(&error),
                        "profile recovery refresh failed after committed operation"
                    );
                    let summary = ProfileRecoverySummary {
                        state: ProfileRecoveryState::Failed,
                        can_submit_passphrase: false,
                        restart_required: false,
                        background_ready: true,
                        cleanup_pending: self.recovery.cleanup_pending(),
                        losses: Vec::new(),
                        admission: None,
                    };
                    *self.lock_ready_summary_override() = Some(summary.clone());
                    publish_profile_recovery(&self.events, summary);
                }
            }
        }
        Ok(result)
    }

    async fn execute_recovery(
        &self,
        bootstrap: Arc<RecoveryBootstrap>,
        operation: Operation,
        cancellation: CancellationToken,
    ) -> Result<OperationResult, EngineError> {
        match operation {
            Operation::QueryProfileRecovery => Ok(OperationResult::ProfileRecovery(
                bootstrap.lock_summary().clone(),
            )),
            Operation::QueryEncryptionState => {
                Ok(OperationResult::EncryptionState(EncryptionStateSummary {
                    initialized: true,
                    session_ready: false,
                }))
            }
            Operation::UnlockSpace(input) => {
                let _gate = bootstrap.gate.lock().await;
                match self.mode().await {
                    RuntimeMode::Ready { runtime, recovered } => {
                        return self
                            .execute_ready(
                                runtime,
                                recovered,
                                Operation::UnlockSpace(input),
                                cancellation,
                            )
                            .await;
                    }
                    RuntimeMode::Recovery(current) if Arc::ptr_eq(&current, &bootstrap) => {}
                    _ => return Err(profile_recovery_required_error()),
                }
                if !bootstrap.lock_summary().can_submit_passphrase {
                    return Err(profile_recovery_required_error());
                }
                self.publish_summary(&bootstrap, |summary| {
                    summary.state = ProfileRecoveryState::Recovering
                });
                let passphrase = Passphrase::new(input.passphrase.expose());
                match self.recovery.recover(&passphrase).await {
                    Ok(ProfileRecoveryOutcome::PartiallyRecoverable(losses)) => {
                        self.publish_summary(&bootstrap, |summary| {
                            summary.state = ProfileRecoveryState::PartiallyRecoverable;
                            summary.can_submit_passphrase = false;
                            summary.restart_required = false;
                            summary.losses = public_losses(losses);
                        });
                        Err(EngineError::new(
                            PROFILE_RECOVERY_PARTIAL_CODE,
                            EngineErrorCategory::Unavailable,
                            false,
                        ))
                    }
                    Ok(ProfileRecoveryOutcome::Ready) => {
                        if !take_startup_input(&bootstrap).await {
                            self.publish_restart_required(&bootstrap);
                            return Err(profile_recovery_required_error());
                        }
                        let paths = derive_app_paths(self.host.directories());
                        let construction = RecoveryConstructionGuard::new(
                            Arc::clone(&bootstrap),
                            Arc::clone(&self.host),
                            self.events.clone(),
                        );
                        let runtime = match ProductionRuntime::start(
                            self.config.clone(),
                            self.recovery_host_capabilities(),
                            paths,
                            self.events.clone(),
                            Arc::clone(&bootstrap.progress),
                            Arc::clone(&self.recovery),
                            Some(&construction.owner),
                        )
                        .await
                        {
                            Ok(runtime) => Arc::new(runtime),
                            Err(error) => {
                                let settled = construction.finish_without_runtime();
                                if !settled {
                                    self.publish_restart_required(&bootstrap);
                                    return Err(restart_required_error(error));
                                }
                                if let Some(summary) = error.admission_recovery() {
                                    bootstrap.progress.recovery_available();
                                    self.enter_admission_recovery(summary, None).await;
                                    return Err(profile_recovery_required_error());
                                }
                                self.publish_restart_required(&bootstrap);
                                return Err(restart_required_error(error));
                            }
                        };
                        let runtime_guard = construction.handoff(Arc::clone(&runtime));
                        let result = match self
                            .execute_ready(
                                Arc::clone(&runtime),
                                true,
                                Operation::UnlockSpace(input),
                                cancellation,
                            )
                            .await
                        {
                            Ok(result) => result,
                            Err(error) => {
                                let retained = if let Err(shutdown) = runtime.shutdown(None).await {
                                    uc_warn!(
                                        error_kind = "profile_recovery_shutdown",
                                        io_error_kind = io_error_kind(&shutdown),
                                        "profile recovery unlock failed and the runtime did not shut down cleanly"
                                    );
                                    Some(Arc::clone(&runtime))
                                } else {
                                    None
                                };
                                *self.mode.write().await =
                                    RuntimeMode::RestartRequired { runtime: retained };
                                runtime_guard.complete();
                                self.publish_restart_required(&bootstrap);
                                return Err(restart_required_error(error));
                            }
                        };
                        *self.mode.write().await = RuntimeMode::Ready {
                            runtime,
                            recovered: true,
                        };
                        runtime_guard.complete();
                        if self.lock_ready_summary_override().is_none() {
                            self.publish_summary(&bootstrap, |summary| {
                                summary.state = ProfileRecoveryState::Recovered;
                                summary.can_submit_passphrase = false;
                                summary.restart_required = false;
                                summary.background_ready = true;
                                summary.cleanup_pending = self.recovery.cleanup_pending();
                                summary.losses.clear();
                            });
                        }
                        Ok(result)
                    }
                    Err(ProfileKeyRecoveryError::WrongPassphrase) => {
                        self.publish_summary(&bootstrap, |summary| {
                            summary.state = ProfileRecoveryState::AwaitingPassphrase;
                            summary.restart_required = false;
                        });
                        Err(EngineError::new(
                            UNLOCK_SPACE_UNAUTHORIZED_CODE,
                            EngineErrorCategory::Unauthorized,
                            false,
                        ))
                    }
                    Err(error) => {
                        uc_warn!(
                            error_kind = "profile_recovery",
                            io_error_kind = io_error_kind(&error),
                            "profile recovery attempt failed"
                        );
                        let error = EngineError::from(error);
                        self.publish_summary(&bootstrap, |summary| {
                            summary.state = ProfileRecoveryState::Failed;
                            summary.can_submit_passphrase = error.is_retryable();
                            summary.restart_required = false;
                        });
                        Err(error)
                    }
                }
            }
            Operation::FactoryResetSpace => self.factory_reset_from_recovery(&bootstrap).await,
            _ => Err(profile_recovery_required_error()),
        }
    }

    /// 恢复模式不启动业务运行时；资料密钥完好时只装配恢复出厂依赖并执行重置，完成后须重启 Engine。
    async fn factory_reset_from_recovery(
        &self,
        bootstrap: &Arc<RecoveryBootstrap>,
    ) -> Result<OperationResult, EngineError> {
        let _gate = bootstrap.gate.lock().await;
        if !*bootstrap.input_available.lock().await {
            return Err(profile_recovery_required_error());
        }
        if !self.recovery.open_for_factory_reset().await? {
            return Err(profile_recovery_required_error());
        }
        if !take_startup_input(bootstrap).await {
            return Err(profile_recovery_required_error());
        }
        let paths = derive_app_paths(self.host.directories());
        let construction = RecoveryConstructionGuard::new(
            Arc::clone(bootstrap),
            Arc::clone(&self.host),
            self.events.clone(),
        );
        let wiring = wire_host_capabilities_with_emitter(
            &self.config,
            self.recovery_host_capabilities(),
            paths,
            Arc::new(EngineHostEventEmitter::new(self.events.clone())),
            Arc::clone(&bootstrap.progress),
            Arc::clone(&self.recovery),
            Some(&construction.owner),
        )
        .await;
        let wired = match wiring {
            Ok(wiring) => wiring.wired,
            Err(error) => {
                construction.finish_without_runtime();
                self.publish_restart_required(bootstrap);
                return Err(restart_required_error(startup_error(
                    "factory reset dependency wiring",
                    error,
                )));
            }
        };
        let reset = ProfileFactoryResetFacade::new(
            Arc::clone(&wired.profile_reset.lifecycle_repository),
            Arc::new(NoProfileRuntime),
            Arc::clone(&wired.profile_reset.keys),
            Arc::clone(&wired.profile_reset.backup_security),
            Arc::clone(&wired.profile_reset.state),
        );
        let result = execute_factory_reset_space(&reset).await;
        self.recovery.forget_after_factory_reset();
        drop(reset);
        drop(wired);
        if !construction.finish_without_runtime() {
            self.publish_restart_required(bootstrap);
            return Err(EngineError::new(1108, EngineErrorCategory::Internal, false));
        }
        match result {
            Ok(result) => {
                self.publish_summary(bootstrap, |summary| {
                    summary.state = ProfileRecoveryState::NotRequired;
                    summary.can_submit_passphrase = false;
                    summary.restart_required = true;
                    summary.background_ready = false;
                    summary.cleanup_pending = false;
                    summary.losses.clear();
                });
                Ok(result)
            }
            Err(error) => {
                self.publish_restart_required(bootstrap);
                Err(restart_required_error(error))
            }
        }
    }

    /// 离开空间：重置、释放旧运行期、按启动决策重建，由这里统一负责。
    ///
    /// 全程持有模式写锁，其他操作要么在旧运行期完成后排队，要么直接面对新运行期。
    /// 重置本身失败时保持原运行期（重置阶段已持久，重试同一操作会续做）；
    /// 清除完成但新运行期装配失败时转入需要重启，不报告成功。
    async fn factory_reset_ready(
        &self,
        cancellation: CancellationToken,
    ) -> Result<OperationResult, EngineError> {
        let mut mode = self.mode.write().await;
        let runtime = match &*mode {
            RuntimeMode::Ready { runtime, .. } => Arc::clone(runtime),
            RuntimeMode::Stopped | RuntimeMode::RestartRequired { .. } => {
                return Err(restart_required_error_for_reset())
            }
            RuntimeMode::Recovery(_) | RuntimeMode::AdmissionRecovery { .. } => {
                return Err(profile_recovery_required_error())
            }
        };
        let result = runtime
            .execute(Operation::FactoryResetSpace, cancellation)
            .await?;
        self.recovery.forget_after_factory_reset();
        *self.lock_ready_summary_override() = None;

        if let Err(error) = runtime.shutdown(None).await {
            uc_warn!(
                error_code = error.code(),
                "space left but the previous runtime did not release cleanly"
            );
            return Err(self.require_restart(&mut mode, Some(runtime)));
        }
        // 启动进度属于宿主观察过的那一次启动，这里重建使用不被任何人观察的独立进度。
        let (detached_progress, _) = StartupProgress::channel();
        let bootstrap = Arc::new(RecoveryBootstrap::new(
            Arc::clone(&detached_progress.store),
            restart_required_summary(),
            false,
        ));
        *self
            .reconstruction
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::clone(&bootstrap));
        let construction = RecoveryConstructionGuard::new(
            Arc::clone(&bootstrap),
            Arc::clone(&self.host),
            self.events.clone(),
        );
        match start_profile_mode(
            &self.config,
            &self.host,
            &self.events,
            &detached_progress.store,
            &self.recovery,
            Some(&construction.owner),
        )
        .await
        {
            Ok(next) => {
                uc_info!("space left and a fresh runtime is ready");
                *mode = next;
                construction.handoff_mode();
                *self
                    .reconstruction
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                Ok(result)
            }
            Err(error) => {
                uc_warn!(
                    error_code = error.code(),
                    "space left but the fresh runtime could not start"
                );
                if construction.finish_without_runtime() {
                    *self
                        .reconstruction
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                }
                Err(self.require_restart(&mut mode, None))
            }
        }
    }

    fn require_restart(
        &self,
        mode: &mut RuntimeMode,
        runtime: Option<Arc<ProductionRuntime>>,
    ) -> EngineError {
        *mode = RuntimeMode::RestartRequired { runtime };
        publish_profile_recovery(&self.events, restart_required_summary());
        restart_required_error_for_reset()
    }

    /// 重建与恢复装配共用：宿主能力加上资料密钥恢复存储。
    fn recovery_host_capabilities(&self) -> HostCapabilities {
        let mut capabilities = self.host.capabilities();
        capabilities.replace_secure_storage(Arc::new(RecoveryHostStorage {
            inner: Arc::clone(&self.recovery),
        }));
        capabilities
    }

    fn publish_summary(
        &self,
        bootstrap: &RecoveryBootstrap,
        update: impl FnOnce(&mut ProfileRecoverySummary),
    ) {
        let summary = {
            let mut summary = bootstrap.lock_summary();
            update(&mut summary);
            summary.clone()
        };
        publish_profile_recovery(&self.events, summary);
    }

    fn publish_restart_required(&self, bootstrap: &RecoveryBootstrap) {
        self.publish_summary(bootstrap, |summary| {
            summary.state = ProfileRecoveryState::Failed;
            summary.can_submit_passphrase = false;
            summary.restart_required = true;
            summary.background_ready = false;
        });
    }

    fn lock_ready_summary_override(&self) -> StdMutexGuard<'_, Option<ProfileRecoverySummary>> {
        self.ready_summary_override
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

async fn finish_without_runtime_start(
    host: &Arc<ReusableHost>,
    cleanup: &Arc<StartupCleanupOwner>,
    primary: EngineError,
) -> EngineError {
    cleanup.record_primary(primary.clone());
    if cleanup.rollback_unconfirmed() {
        return primary;
    }
    let waiting = cleanup.start_host_close(Arc::clone(host), primary.clone());
    // 关闭任务属于启动负责人；此处仅等待其完成。
    let _ = waiting.await;
    primary
}

/// 按资料当前状态决定运行模式：正常装配运行期，或进入口令恢复/准入恢复。
/// 进程启动与离开空间后的重建使用同一条路径。
async fn start_profile_mode(
    config: &EngineConfig,
    host: &ReusableHost,
    events: &EventSender,
    progress: &Arc<StartupProgressStore>,
    recovery: &Arc<ProfileKeyRecoveryStore>,
    cleanup: Option<&Arc<StartupCleanupOwner>>,
) -> Result<RuntimeMode, EngineError> {
    let paths = derive_app_paths(host.directories());
    match recovery.prepare_startup().await? {
        ProfileRecoveryPreparation::Ready => {
            let mut capabilities = host.capabilities();
            capabilities.replace_secure_storage(Arc::new(RecoveryHostStorage {
                inner: Arc::clone(recovery),
            }));
            match ProductionRuntime::start(
                config.clone(),
                capabilities,
                paths,
                events.clone(),
                Arc::clone(progress),
                Arc::clone(recovery),
                cleanup,
            )
            .await
            {
                Ok(runtime) => Ok(RuntimeMode::Ready {
                    runtime: Arc::new(runtime),
                    recovered: false,
                }),
                Err(error) if cleanup.is_some_and(|owner| owner.rollback_unconfirmed()) => {
                    Err(error)
                }
                Err(error) => match error.admission_recovery() {
                    Some(summary) => {
                        progress.recovery_available();
                        publish_profile_recovery(events, admission_summary(summary));
                        Ok(RuntimeMode::AdmissionRecovery {
                            summary,
                            runtime: None,
                        })
                    }
                    None => Err(error),
                },
            }
        }
        ProfileRecoveryPreparation::AwaitingPassphrase { losses } => {
            progress.recovery_available();
            let summary = ProfileRecoverySummary {
                state: if !losses.is_empty() {
                    ProfileRecoveryState::PartiallyRecoverable
                } else {
                    ProfileRecoveryState::AwaitingPassphrase
                },
                can_submit_passphrase: losses.is_empty(),
                restart_required: false,
                background_ready: false,
                cleanup_pending: false,
                losses: public_losses(losses),
                admission: None,
            };
            publish_profile_recovery(events, summary.clone());
            Ok(RuntimeMode::Recovery(Arc::new(RecoveryBootstrap::new(
                Arc::clone(progress),
                summary,
                true,
            ))))
        }
    }
}

/// 消费一次启动输入；已被消费时返回 `false`。
async fn take_startup_input(bootstrap: &RecoveryBootstrap) -> bool {
    std::mem::replace(&mut *bootstrap.input_available.lock().await, false)
}

fn restart_required_summary() -> ProfileRecoverySummary {
    ProfileRecoverySummary {
        state: ProfileRecoveryState::Failed,
        can_submit_passphrase: false,
        restart_required: true,
        background_ready: false,
        cleanup_pending: false,
        losses: Vec::new(),
        admission: None,
    }
}

/// 离开空间的资料已清除，但同一实例无法继续服务；重试同一请求不能解决，宿主须重启 Engine。
fn restart_required_error_for_reset() -> EngineError {
    EngineError::new(
        FACTORY_RESET_RESTART_REQUIRED_CODE,
        EngineErrorCategory::Unavailable,
        false,
    )
}

/// 资料恢复状态机的每次转换：同一处发布事件并留下时间线，只含状态枚举与两个布尔位。
fn publish_profile_recovery(events: &EventSender, summary: ProfileRecoverySummary) {
    uc_info!(
        recovery_state = log_vocab_debug(&summary.state),
        restart_required = summary.restart_required,
        can_submit_passphrase = summary.can_submit_passphrase,
        "profile recovery state changed"
    );
    events.send(EngineEvent::ProfileRecoveryChanged(summary));
}

impl RecoveryBootstrap {
    fn new(
        progress: Arc<StartupProgressStore>,
        summary: ProfileRecoverySummary,
        input_available: bool,
    ) -> Self {
        Self {
            input_available: Mutex::new(input_available),
            gate: Mutex::new(()),
            progress,
            summary: StdMutex::new(summary),
            recovered_runtime: StdMutex::new(None),
            construction: StdMutex::new(None),
            construction_inflight: AtomicBool::new(false),
            construction_interrupted: AtomicBool::new(false),
            construction_changed: Notify::new(),
            construction_blocking_changed: Condvar::new(),
            construction_blocking_gate: StdMutex::new(()),
        }
    }

    fn lock_summary(&self) -> StdMutexGuard<'_, ProfileRecoverySummary> {
        self.summary
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn finish_construction(&self) {
        {
            let _gate = self
                .construction_blocking_gate
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.construction_inflight.store(false, Ordering::Release);
            self.construction_blocking_changed.notify_all();
        }
        self.construction_changed.notify_waiters();
    }

    fn wait_for_construction_blocking(&self) {
        let mut gate = self
            .construction_blocking_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while self.construction_inflight.load(Ordering::Acquire) {
            gate = self
                .construction_blocking_changed
                .wait(gate)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    async fn wait_for_construction(&self) {
        loop {
            let notified = self.construction_changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if !self.construction_inflight.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }

    async fn wait_accepted_work(&self) {
        self.wait_for_construction().await;
        let owner = self.lock_construction().clone();
        if let Some(owner) = owner {
            let _ = owner.wait_recovery_backup().await;
        }
    }

    fn wait_accepted_work_blocking(&self) {
        self.wait_for_construction_blocking();
        let owner = self.lock_construction().clone();
        if let Some(owner) = owner {
            let _ = owner.wait_recovery_backup_blocking();
        }
    }

    fn lock_construction(&self) -> StdMutexGuard<'_, Option<Arc<StartupCleanupOwner>>> {
        self.construction
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn lock_recovered_runtime(&self) -> StdMutexGuard<'_, Option<Arc<ProductionRuntime>>> {
        self.recovered_runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[async_trait]
impl EngineRuntime for RecoverableRuntime {
    async fn execute(
        &self,
        operation: Operation,
        cancellation: CancellationToken,
    ) -> Result<OperationResult, EngineError> {
        match self.mode().await {
            RuntimeMode::Ready { runtime, recovered } => {
                if matches!(operation, Operation::FactoryResetSpace) {
                    return self.factory_reset_ready(cancellation).await;
                }
                let result = self
                    .execute_ready(Arc::clone(&runtime), recovered, operation, cancellation)
                    .await;
                self.restrict_after_failure(&runtime, result).await
            }
            RuntimeMode::Stopped | RuntimeMode::RestartRequired { .. } => match operation {
                Operation::QueryProfileRecovery => {
                    Ok(OperationResult::ProfileRecovery(restart_required_summary()))
                }
                _ => Err(restart_required_error_for_reset()),
            },
            RuntimeMode::Recovery(bootstrap) => {
                self.execute_recovery(bootstrap, operation, cancellation)
                    .await
            }
            RuntimeMode::AdmissionRecovery { summary, .. } => match operation {
                Operation::QueryProfileRecovery => {
                    Ok(OperationResult::ProfileRecovery(admission_summary(summary)))
                }
                Operation::QueryEncryptionState => {
                    Ok(OperationResult::EncryptionState(EncryptionStateSummary {
                        initialized: true,
                        session_ready: false,
                    }))
                }
                _ => Err(profile_recovery_required_error()),
            },
        }
    }

    #[cfg(feature = "dev-tools")]
    async fn execute_dev(
        &self,
        operation: DevOperation,
        cancellation: CancellationToken,
    ) -> Result<DevOperationResult, EngineError> {
        match self.mode().await {
            RuntimeMode::Ready { runtime, .. } => {
                runtime.execute_dev(operation, cancellation).await
            }
            RuntimeMode::Recovery(_) | RuntimeMode::AdmissionRecovery { .. } => {
                Err(profile_recovery_required_error())
            }
            RuntimeMode::Stopped | RuntimeMode::RestartRequired { .. } => {
                Err(restart_required_error_for_reset())
            }
        }
    }

    async fn suspend(&self, deadline: Option<Instant>) -> Result<(), EngineError> {
        match self.mode().await {
            RuntimeMode::Ready { runtime, .. }
            | RuntimeMode::AdmissionRecovery {
                runtime: Some(runtime),
                ..
            } => {
                // The lifecycle queue keeps an accepted suspend alive after its caller times out.
                // Once the original deadline has elapsed, cleanup must run without reusing that
                // stale deadline or it can leave the production runtime only partly suspended.
                let cleanup_deadline = deadline.filter(|deadline| *deadline > Instant::now());
                runtime.suspend(cleanup_deadline).await?;
                self.recovery.suspend();
                Ok(())
            }
            // 重置后的旧运行期已经停止，没有可挂起的内容。
            RuntimeMode::Recovery(bootstrap) => {
                if bootstrap.lock_construction().is_some()
                    || bootstrap.lock_recovered_runtime().is_some()
                {
                    return Err(EngineError::new(1108, EngineErrorCategory::Internal, false));
                }
                Ok(())
            }
            RuntimeMode::AdmissionRecovery { runtime: None, .. }
            | RuntimeMode::Stopped
            | RuntimeMode::RestartRequired { .. } => Ok(()),
        }
    }

    async fn resume(&self, cancellation: CancellationToken) -> Result<(), EngineError> {
        match self.mode().await {
            RuntimeMode::Ready { runtime, .. } => match runtime.resume(cancellation).await {
                Ok(()) => Ok(()),
                // 与启动期一致：恢复为可查询的受限实例，业务操作统一返回需要恢复。
                Err(error) => match runtime.admission_recovery() {
                    Some(summary) => {
                        self.enter_admission_recovery(summary, Some(Arc::clone(&runtime)))
                            .await;
                        Ok(())
                    }
                    None => Err(error),
                },
            },
            // 受限模式不重建业务会话；已启动的运行期保持挂起直到关闭。
            RuntimeMode::Recovery(bootstrap) => {
                if bootstrap.lock_construction().is_some()
                    || bootstrap.lock_recovered_runtime().is_some()
                {
                    return Err(EngineError::new(1108, EngineErrorCategory::Internal, false));
                }
                Ok(())
            }
            RuntimeMode::AdmissionRecovery { .. }
            | RuntimeMode::Stopped
            | RuntimeMode::RestartRequired { .. } => Ok(()),
        }
    }

    async fn shutdown(&self, deadline: Option<Instant>) -> Result<(), EngineError> {
        if !self.settle_reconstruction().await {
            return Err(EngineError::new(1108, EngineErrorCategory::Internal, false));
        }
        let stopped_in_recovery = loop {
            let current = self.mode().await;
            // 读取模式时可能等待正在登记的重建；拿到模式后再核对其责任。
            if !self.settle_reconstruction().await {
                return Err(EngineError::new(1108, EngineErrorCategory::Internal, false));
            }
            match current {
                RuntimeMode::Ready { runtime, .. }
                | RuntimeMode::AdmissionRecovery {
                    runtime: Some(runtime),
                    ..
                }
                | RuntimeMode::RestartRequired {
                    runtime: Some(runtime),
                } => {
                    runtime.shutdown(deadline).await?;
                    self.recovery.suspend();
                }
                RuntimeMode::Recovery(bootstrap) => {
                    // 恢复动作持有同一门直到模式交接完成。
                    let _recovery = bootstrap.gate.lock().await;
                    bootstrap.wait_for_construction().await;
                    if !matches!(self.mode().await, RuntimeMode::Recovery(current) if Arc::ptr_eq(&current, &bootstrap))
                    {
                        continue;
                    }
                    let construction = bootstrap.lock_construction().clone();
                    if let Some(owner) = construction {
                        if !owner.wait_recovery_backup().await {
                            return Err(EngineError::new(
                                1108,
                                EngineErrorCategory::Internal,
                                false,
                            ));
                        }
                        if bootstrap.construction_interrupted.load(Ordering::Acquire)
                            || !owner.settle_recovery_construction()
                        {
                            return Err(EngineError::new(
                                1108,
                                EngineErrorCategory::Internal,
                                false,
                            ));
                        }
                        *bootstrap.lock_construction() = None;
                    }
                    if bootstrap.lock_recovered_runtime().is_some() {
                        return Err(EngineError::new(1108, EngineErrorCategory::Internal, false));
                    }
                    let mut mode = self.mode.write().await;
                    if !matches!(&*mode, RuntimeMode::Recovery(current) if Arc::ptr_eq(current, &bootstrap))
                    {
                        continue;
                    }
                    *mode = RuntimeMode::Stopped;
                    break true;
                }
                RuntimeMode::AdmissionRecovery { runtime: None, .. }
                | RuntimeMode::Stopped
                | RuntimeMode::RestartRequired { runtime: None } => {}
            }
            break false;
        };
        // 最终关闭成功后释放本实例保留的生产运行期。
        // 不修改停止参与者为重置流程保留数据库的契约。
        if !stopped_in_recovery {
            let previous = std::mem::replace(&mut *self.mode.write().await, RuntimeMode::Stopped);
            drop(previous);
        }
        self.host.close_change_stream().await.map_err(|source| {
            uc_warn!(
                error_kind = "change_stream_shutdown",
                io_error_kind = io_error_kind(&source),
                "host clipboard change stream shutdown failed"
            );
            lifecycle_error(LifecycleError {
                primary: anyhow::Error::new(source).context("close host change stream"),
                additional: Vec::new(),
            })
        })?;
        Ok(())
    }

    async fn wait_shutdown_obligations(&self) {
        if let Some(bootstrap) = self.reconstruction() {
            bootstrap.wait_accepted_work().await;
        }
        let mode = self.mode().await;
        if let Some(bootstrap) = self.reconstruction() {
            bootstrap.wait_accepted_work().await;
        }
        if let RuntimeMode::Recovery(bootstrap) = mode {
            bootstrap.wait_accepted_work().await;
        }
    }

    fn wait_shutdown_obligations_blocking(&self) {
        if let Some(bootstrap) = self.reconstruction() {
            bootstrap.wait_accepted_work_blocking();
        }
        let mode = self.mode.blocking_read().clone();
        if let Some(bootstrap) = self.reconstruction() {
            bootstrap.wait_accepted_work_blocking();
        }
        if let RuntimeMode::Recovery(bootstrap) = mode {
            bootstrap.wait_accepted_work_blocking();
        }
    }

    async fn supports_shutdown_retry(&self) -> bool {
        matches!(self.mode().await, RuntimeMode::Stopped) && self.host.has_retryable_change_stream()
    }
}

/// 恢复模式没有启动任何资料运行时，恢复出厂前无需停止。
struct NoProfileRuntime;

#[async_trait]
impl StopProfileRuntimePort for NoProfileRuntime {
    async fn stop_profile_runtime(&self) -> Result<(), LifecycleError> {
        Ok(())
    }
}

struct RecoveryHostStorage {
    inner: Arc<ProfileKeyRecoveryStore>,
}

impl HostSecureStorage for RecoveryHostStorage {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, HostCapabilityError> {
        match self.inner.get(key) {
            Ok(value) => Ok(value),
            Err(error) => Err(error.into()),
        }
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<(), HostCapabilityError> {
        match self.inner.set(key, value) {
            Ok(()) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    fn delete(&self, key: &str) -> Result<(), HostCapabilityError> {
        match self.inner.delete(key) {
            Ok(()) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

impl From<SecureStorageError> for HostCapabilityError {
    fn from(error: SecureStorageError) -> Self {
        let category = match error {
            SecureStorageError::Unavailable(_) => HostCapabilityErrorCategory::Unavailable,
            SecureStorageError::PermissionDenied(_) => {
                HostCapabilityErrorCategory::PermissionDenied
            }
            _ => HostCapabilityErrorCategory::Io,
        };
        Self::new(category, "profile recovery storage failed")
    }
}

impl From<ProfileKeyRecoveryError> for EngineError {
    fn from(error: ProfileKeyRecoveryError) -> Self {
        match error {
            ProfileKeyRecoveryError::WrongPassphrase => Self::new(
                UNLOCK_SPACE_UNAUTHORIZED_CODE,
                EngineErrorCategory::Unauthorized,
                false,
            ),
            ProfileKeyRecoveryError::Corrupt => Self::new(
                UNLOCK_SPACE_CORRUPTED_CODE,
                EngineErrorCategory::Internal,
                false,
            ),
            ProfileKeyRecoveryError::Unsupported => Self::new(
                PROFILE_RECOVERY_UNSUPPORTED_CODE,
                EngineErrorCategory::Unavailable,
                false,
            ),
            ProfileKeyRecoveryError::Storage(_) => Self::new(
                PROFILE_RECOVERY_PERSISTENCE_FAILED_CODE,
                EngineErrorCategory::Unavailable,
                true,
            ),
        }
    }
}

fn restart_required_error(error: EngineError) -> EngineError {
    EngineError::new(error.code(), error.category(), false)
}

fn public_losses(losses: ProfileRecoveryLosses) -> Vec<ProfileRecoveryLoss> {
    let mut result = Vec::new();
    if losses.local_history {
        result.push(ProfileRecoveryLoss::LocalHistory);
    }
    if losses.local_control_state {
        result.push(ProfileRecoveryLoss::LocalControlState);
    }
    if losses.device_identity {
        result.push(ProfileRecoveryLoss::DeviceIdentity);
    }
    result
}

fn ready_summary(recovered: bool, cleanup_pending: bool) -> ProfileRecoverySummary {
    ProfileRecoverySummary {
        state: if recovered {
            ProfileRecoveryState::Recovered
        } else {
            ProfileRecoveryState::NotRequired
        },
        can_submit_passphrase: false,
        restart_required: false,
        background_ready: true,
        cleanup_pending,
        losses: Vec::new(),
        admission: None,
    }
}

fn admission_summary(admission: AdmissionRecoverySummary) -> ProfileRecoverySummary {
    ProfileRecoverySummary {
        state: ProfileRecoveryState::AdmissionRecoveryRequired,
        can_submit_passphrase: false,
        restart_required: false,
        background_ready: false,
        cleanup_pending: false,
        losses: Vec::new(),
        admission: Some(admission),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::event_stream::event_channel;

    #[test]
    fn a_state_change_is_recorded_with_its_state_and_flags_only() {
        let logs = uc_testkit::log_capture::CapturedLogs::default();
        let _guard = logs.install();
        let (events, _stream) = event_channel(4);

        publish_profile_recovery(
            &events,
            ProfileRecoverySummary {
                state: ProfileRecoveryState::Failed,
                can_submit_passphrase: false,
                restart_required: true,
                background_ready: false,
                cleanup_pending: false,
                losses: Vec::new(),
                admission: None,
            },
        );

        assert_eq!(logs.count("profile recovery state changed"), 1);
        assert!(logs.output().contains("recovery_state=Failed"));
        assert!(logs.output().contains("restart_required=true"));
    }

    #[test]
    fn recovery_storage_errors_keep_stable_public_categories() {
        for (source, expected) in [
            (
                SecureStorageError::Unavailable("unavailable".to_owned()),
                HostCapabilityErrorCategory::Unavailable,
            ),
            (
                SecureStorageError::PermissionDenied("denied".to_owned()),
                HostCapabilityErrorCategory::PermissionDenied,
            ),
            (
                SecureStorageError::Other("other".to_owned()),
                HostCapabilityErrorCategory::Io,
            ),
        ] {
            let error = HostCapabilityError::from(source);
            assert_eq!(error.category(), expected);
        }
    }

    #[test]
    fn recovery_failures_keep_stable_engine_categories_and_retryability() {
        let cases = [
            (
                ProfileKeyRecoveryError::WrongPassphrase,
                UNLOCK_SPACE_UNAUTHORIZED_CODE,
                EngineErrorCategory::Unauthorized,
                false,
            ),
            (
                ProfileKeyRecoveryError::Corrupt,
                UNLOCK_SPACE_CORRUPTED_CODE,
                EngineErrorCategory::Internal,
                false,
            ),
            (
                ProfileKeyRecoveryError::Unsupported,
                PROFILE_RECOVERY_UNSUPPORTED_CODE,
                EngineErrorCategory::Unavailable,
                false,
            ),
            (
                ProfileKeyRecoveryError::Storage(anyhow::anyhow!("storage")),
                PROFILE_RECOVERY_PERSISTENCE_FAILED_CODE,
                EngineErrorCategory::Unavailable,
                true,
            ),
        ];
        for (source, code, category, retryable) in cases {
            let error = EngineError::from(source);
            assert_eq!(error.code(), code);
            assert_eq!(error.category(), category);
            assert_eq!(error.is_retryable(), retryable);
        }
    }
}
