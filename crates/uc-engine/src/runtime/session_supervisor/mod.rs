use std::error::Error;
use std::future::Future;
use std::sync::atomic::AtomicBool;
#[cfg(feature = "dev-tools")]
use std::sync::atomic::AtomicUsize;
#[cfg(feature = "dev-tools")]
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use tokio::sync::{Mutex, Notify};
use tokio::time::{timeout_at, Instant};
use tokio_util::sync::CancellationToken;
use tracing::Instrument;
use uc_application::deps::LifecycleError;
use uc_application::facade::{
    AppFacade, ApplicationAssembly, ApplicationRuntime, ClipboardInboundEvent,
    ClipboardInboundEventAction, ClipboardInboundEventPort, CompletePendingSpaceTransitionError,
    RecoverSpaceSessionError, RuntimeLifecycle,
};
use uc_core::{FileTransferCancellationReason, TaskRegistry};
use uc_infra_local::fs::{FsAtomicPublisher, FsHiddenPathMarker, FsInboundFileTarget};
use uc_infra_p2p::network::iroh::{IrohNode, IrohSessionBuilder, PreparedIrohSession};
use uc_infra_profile::space::ProfileLocalResources;
use uc_observability_contract::diagnostics::connectivity::{
    observe_local_result, record_session_lock_wait, LocalWorkStep, SessionTransition,
    SessionTransitionResult,
};
use uc_observability_contract::diagnostics::{
    complete_operation, operation_span, DiagnosticDomain, DiagnosticErrorType, DiagnosticOperation,
    DiagnosticRole, DiagnosticSpanKind, OperationCompletion, OperationContext,
};
use uc_observability_contract::{
    error_source::io_error_kind, log_fields::log_vocab_debug, uc_error, uc_warn,
};

use crate::assembly::deps::WiredDependencies;
#[cfg(feature = "lan-compat")]
use crate::assembly::facade::build_mobile_sync_facade;
use crate::assembly::lifecycle::{
    build_network_runtime, prepare_daemon_session, reconcile_session_peers,
};
#[cfg(feature = "dev-tools")]
use crate::dev::JoinerFinalConfirmationGate;
use crate::engine::event_stream::{EventSender, RefreshHold};
use crate::operations::space::lifecycle::execute_reset_space;
use crate::subsystems::peer_keepalive::spawn_peer_reachability_event_task;
#[cfg(feature = "dev-tools")]
use crate::SessionHandoverFailurePoint;
use crate::{
    ActiveClipboardChanged, AdmissionRecoverySummary, EngineEvent, InboundNoticeActionSummary,
    InboundNoticeEvent, InboundRepresentationSummary, RefreshReason,
};

use super::{
    operation_error_with_code, operation_unavailable_error, profile_recovery_required_error,
    retryable_operation_error_with_code,
};
use crate::{EngineError, EngineErrorCategory, OperationResult};

const SESSION_OPERATION_GRACE: Duration = Duration::from_secs(2);
const SESSION_RUNTIME_FAILED_CODE: u32 = 1101;

#[cfg(feature = "dev-tools")]
#[derive(Default)]
struct SessionHandoverTestFailure {
    remaining: AtomicUsize,
    consumed: AtomicUsize,
}

#[cfg(feature = "dev-tools")]
impl SessionHandoverTestFailure {
    fn arm(&self) {
        self.remaining.fetch_add(1, Ordering::SeqCst);
    }

    fn consume(&self) -> bool {
        let consumed = self
            .remaining
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok();
        if consumed {
            self.consumed.fetch_add(1, Ordering::SeqCst);
        }
        consumed
    }

    fn consumed(&self) -> usize {
        self.consumed.load(Ordering::SeqCst)
    }
}

#[cfg(feature = "dev-tools")]
#[derive(Default)]
struct SessionHandoverTestControl {
    session_quiesce: SessionHandoverTestFailure,
    transition_completion: SessionHandoverTestFailure,
    session_preparation: SessionHandoverTestFailure,
    session_activation: SessionHandoverTestFailure,
    network_build_count: AtomicUsize,
}

#[cfg(feature = "dev-tools")]
impl SessionHandoverTestControl {
    fn failure(&self, point: SessionHandoverFailurePoint) -> &SessionHandoverTestFailure {
        match point {
            SessionHandoverFailurePoint::SessionQuiesce => &self.session_quiesce,
            SessionHandoverFailurePoint::TransitionCompletion => &self.transition_completion,
            SessionHandoverFailurePoint::SessionPreparation => &self.session_preparation,
            SessionHandoverFailurePoint::SessionActivation => &self.session_activation,
        }
    }
}

#[cfg(feature = "dev-tools")]
pub(super) struct SessionHandoverDiagnostics {
    pub(super) network_build_count: usize,
    pub(super) session_quiesce_failure_count: usize,
    pub(super) transition_completion_failure_count: usize,
    pub(super) session_preparation_failure_count: usize,
    pub(super) session_activation_failure_count: usize,
}

/// 会话激活未完成时回收已准备好的会话；清理失败只记分类，不改变激活失败的结果。
async fn cleanup_prepared_session(session: ProductionSession, stage: &'static str) {
    if let Err(error) = session.shutdown(None).await {
        uc_warn!(
            error_kind = "prepared_session_cleanup",
            stage = stage,
            io_error_kind = io_error_kind(&error),
            "prepared session cleanup failed"
        );
    }
}

fn session_runtime_error(
    context: &'static str,
    error: impl Into<Box<dyn Error + Send + Sync>>,
) -> EngineError {
    let error = error.into();
    uc_error!(
        context = context,
        io_error_kind = io_error_kind(error.as_ref()),
        "engine session lifecycle failed"
    );
    EngineError::new(
        SESSION_RUNTIME_FAILED_CODE,
        EngineErrorCategory::Unavailable,
        true,
    )
}

#[cfg(any(test, feature = "dev-tools"))]
fn retryable_space_transition_runtime_error(
    context: &'static str,
    error: impl Into<Box<dyn Error + Send + Sync>>,
) -> EngineError {
    let error = error.into();
    uc_error!(
        context = context,
        io_error_kind = io_error_kind(error.as_ref()),
        "engine Space transition failed"
    );
    EngineError::new(1103, EngineErrorCategory::Unavailable, true)
}

fn space_transition_error(
    context: &'static str,
    error: CompletePendingSpaceTransitionError,
) -> EngineError {
    uc_error!(
        context = context,
        error_kind = "space_transition",
        io_error_kind = io_error_kind(&error),
        "engine Space transition failed"
    );
    match error {
        CompletePendingSpaceTransitionError::State { .. } => {
            EngineError::new(1103, EngineErrorCategory::Unavailable, true)
        }
        CompletePendingSpaceTransitionError::JoinNotActive => {
            EngineError::new(1103, EngineErrorCategory::Internal, false)
        }
    }
}

mod lifecycle;
mod recovery;
mod shutdown;
pub(super) use lifecycle::lifecycle_error;
use lifecycle::SessionWork;

struct ProductionSessionFactory {
    wired: WiredDependencies,
    #[cfg(feature = "lan-compat")]
    paths: uc_application::facade::AppPaths,
    app_version: String,
    events: EventSender,
    rendezvous_base_url: Option<String>,
    relay_fallback_override: Option<bool>,
    iroh_bind_port_override: Option<u16>,
    #[cfg(feature = "dev-tools")]
    network_partition_gate: uc_infra_p2p::network::iroh::IrohNetworkPartitionGate,
    #[cfg(feature = "dev-tools")]
    joiner_final_confirmation_gate: Arc<JoinerFinalConfirmationGate>,
    #[cfg(feature = "dev-tools")]
    test_control: Arc<SessionHandoverTestControl>,
    network_recovery: Arc<uc_application::facade::NetworkRecoveryFacade>,
}

struct ProductionSession {
    facade: Arc<AppFacade>,
    application: Arc<ApplicationRuntime>,
    #[cfg(feature = "lan-compat")]
    mobile_sync: Arc<uc_mobile_lan::MobileSyncFacade>,
    tasks: Arc<TaskRegistry>,
}

struct PreparedProductionSession {
    session: ProductionSession,
    network_session: PreparedIrohSession,
}

struct SessionRuntimeState {
    network: Option<IrohNode>,
    session: Option<ProductionSession>,
}

pub(super) struct SessionSupervisor {
    runtime: Mutex<SessionRuntimeState>,
    factory: StdMutex<Option<Arc<ProductionSessionFactory>>>,
    application: uc_application::facade::ApplicationAssembly,
    lifecycle: Mutex<()>,
    operations: SessionOperationGate,
    #[cfg(feature = "dev-tools")]
    test_control: Arc<SessionHandoverTestControl>,
    session_recovery_enabled: AtomicBool,
    coordinator: Arc<RuntimeLifecycle>,
    /// 会话装配发现准入资料无法读取后保持置位；同一进程不重试修复，由恢复运行期转入受限模式。
    admission_recovery: StdMutex<Option<AdmissionRecoverySummary>>,
    /// 空间切换拆除会话后暂存重新查询通知；只在替换会话完整可用后释放，安装或恢复失败则保留到下次成功。
    refresh_hold: StdMutex<Option<RefreshHold>>,
}

pub(super) struct SessionOperationLease {
    gate: Arc<SessionOperationGateInner>,
    cancellation: CancellationToken,
}

struct SessionOperationGate {
    inner: Arc<SessionOperationGateInner>,
}

struct SessionOperationGateInner {
    state: StdMutex<SessionOperationGateState>,
    changed: Notify,
}

struct SessionOperationGateState {
    open: bool,
    active: usize,
    cancellation: CancellationToken,
}

fn session_lifecycle_span(operation: DiagnosticOperation) -> tracing::Span {
    operation_span(OperationContext {
        domain: DiagnosticDomain::Runtime,
        operation,
        role: DiagnosticRole::Local,
        kind: DiagnosticSpanKind::Internal,
    })
}

async fn observe_session_lifecycle<T>(
    future: impl Future<Output = Result<T, EngineError>>,
) -> Result<T, EngineError> {
    observe_runtime_operation(DiagnosticOperation::SessionLifecycle, future).await
}

async fn observe_runtime_operation<T>(
    operation: DiagnosticOperation,
    future: impl Future<Output = Result<T, EngineError>>,
) -> Result<T, EngineError> {
    let started = std::time::Instant::now();
    let span = session_lifecycle_span(operation);
    let result = future.instrument(span.clone()).await;
    span.in_scope(|| {
        let completion = match &result {
            Ok(_) => OperationCompletion::succeeded(
                DiagnosticDomain::Runtime,
                operation,
                DiagnosticRole::Local,
                started.elapsed(),
            ),
            Err(error) => OperationCompletion::failed(
                DiagnosticDomain::Runtime,
                operation,
                DiagnosticRole::Local,
                session_lifecycle_error_type(error),
                started.elapsed(),
            ),
        };
        complete_operation(completion);
    });
    result
}

fn observe_session_request(
    transition: uc_observability_contract::diagnostics::connectivity::SessionTransition,
    future: impl Future<
        Output = Result<
            uc_observability_contract::diagnostics::connectivity::SessionTransitionResult,
            EngineError,
        >,
    >,
) -> impl Future<Output = Result<(), EngineError>> {
    // 在进入观测 future 前固定存放业务 future，避免新增包裹重复放大调用方状态。
    let future = Box::pin(future);
    async move {
        use uc_observability_contract::diagnostics::connectivity::{
            SessionFailure, SessionTransitionObservation, SessionTransitionResult,
        };
        let observation = SessionTransitionObservation::begin(transition);
        let result = future.await;
        let completion = match &result {
            Ok(completion) => *completion,
            Err(error) => SessionTransitionResult::Failed(match error.category() {
                EngineErrorCategory::InvalidInput => SessionFailure::InvalidInput,
                EngineErrorCategory::InvalidState => SessionFailure::InvalidState,
                EngineErrorCategory::Unauthorized => SessionFailure::Unauthorized,
                EngineErrorCategory::NotFound => SessionFailure::NotFound,
                EngineErrorCategory::Conflict => SessionFailure::Conflict,
                EngineErrorCategory::Unavailable => SessionFailure::Unavailable,
                EngineErrorCategory::DeadlineExceeded => SessionFailure::DeadlineExceeded,
                EngineErrorCategory::Internal => SessionFailure::Internal,
            }),
        };
        observation.finish(completion);
        result.map(|_| ())
    }
}

fn session_lifecycle_error_type(error: &EngineError) -> DiagnosticErrorType {
    match error.category() {
        EngineErrorCategory::Unauthorized => DiagnosticErrorType::AuthenticationFailed,
        EngineErrorCategory::Unavailable | EngineErrorCategory::NotFound => {
            DiagnosticErrorType::Unavailable
        }
        EngineErrorCategory::DeadlineExceeded => DiagnosticErrorType::Timeout,
        EngineErrorCategory::InvalidInput => DiagnosticErrorType::DecodeFailed,
        EngineErrorCategory::InvalidState | EngineErrorCategory::Conflict => {
            DiagnosticErrorType::Corrupt
        }
        EngineErrorCategory::Internal => DiagnosticErrorType::Internal,
    }
}

mod factory;
mod gate;
mod handover;
mod ops;

#[cfg(test)]
mod tests;
