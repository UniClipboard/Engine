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
use crate::operations::space::lifecycle::execute_factory_reset_space;
use crate::{
    AdmissionRecoverySummary, EncryptionStateSummary, EngineConfig, EngineError,
    EngineErrorCategory, EngineEvent, HostCapabilities, HostCapabilityError,
    HostCapabilityErrorCategory, HostSecureStorage, Operation, OperationKind, OperationResult,
    ProfileRecoveryLoss, ProfileRecoveryState, ProfileRecoverySummary, StartupProgress,
};
#[cfg(feature = "dev-tools")]
use crate::{DevOperation, DevOperationResult};

#[derive(Clone)]
pub(crate) enum RuntimeMode {
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

pub(crate) struct RecoverableRuntime {
    mode: RwLock<RuntimeMode>,
    config: EngineConfig,
    host: Arc<ReusableHost>,
    recovery: Arc<ProfileKeyRecoveryStore>,
    ready_summary_override: StdMutex<Option<ProfileRecoverySummary>>,
    reconstruction: StdMutex<Option<Arc<RecoveryBootstrap>>>,
    events: EventSender,
}

mod bootstrap;
mod mode;
mod recovery_flow;
mod runtime_api;
mod startup_mode;
mod summary;

pub(super) use bootstrap::{RecoveryBootstrap, RecoveryConstructionGuard};
pub(super) use runtime_api::{NoProfileRuntime, RecoveryHostStorage};
pub(super) use startup_mode::{
    finish_without_runtime_start, publish_profile_recovery, restart_required_error_for_reset,
    restart_required_summary, start_profile_mode, take_startup_input,
};
pub(super) use summary::{admission_summary, public_losses, ready_summary, restart_required_error};

#[cfg(test)]
mod tests;
