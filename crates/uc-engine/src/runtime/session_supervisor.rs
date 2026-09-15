use std::future::Future;
#[cfg(feature = "dev-tools")]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use tokio::sync::{Mutex, Notify};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn, Instrument};
use uc_application::facade::{
    AppFacade, ApplicationRuntime, ClipboardInboundEvent, ClipboardInboundEventAction,
    ClipboardInboundEventPort, CompletePendingSpaceTransitionError,
};
use uc_core::TaskRegistry;
use uc_infra::fs::{FsAtomicPublisher, FsHiddenPathMarker, FsInboundFileTarget};
use uc_infra::network::iroh::{IrohNode, IrohSessionBuilder, PreparedIrohSession};
use uc_observability_contract::diagnostics::connectivity::{
    observe_local_result, record_session_lock_wait, LocalWorkObservation, LocalWorkOutcome,
    LocalWorkStep,
};
use uc_observability_contract::diagnostics::{
    complete_operation, operation_span, DiagnosticDomain, DiagnosticErrorType, DiagnosticOperation,
    DiagnosticRole, DiagnosticSpanKind, OperationCompletion, OperationContext,
};

use crate::assembly::deps::WiredDependencies;
#[cfg(feature = "lan-compat")]
use crate::assembly::facade::build_mobile_sync_facade;
use crate::assembly::lifecycle::{build_network_runtime, prepare_daemon_session};
use crate::assembly::sync_engine::SyncSessionAssembly;
use crate::engine::event_stream::EventSender;
use crate::subsystems::peer_keepalive::spawn_peer_reachability_event_task;
#[cfg(feature = "dev-tools")]
use crate::SessionHandoverFailurePoint;
use crate::{
    EngineEvent, InboundNoticeActionSummary, InboundNoticeEvent, InboundRepresentationSummary,
};

use super::{operation_error_with_code, operation_unavailable_error};
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

fn session_runtime_error(context: &'static str, error: impl std::fmt::Display) -> EngineError {
    error!(context, error = %error, "engine session lifecycle failed");
    EngineError::new(
        SESSION_RUNTIME_FAILED_CODE,
        EngineErrorCategory::Unavailable,
        true,
    )
}

fn retryable_space_transition_runtime_error(
    context: &'static str,
    error: impl std::fmt::Display,
) -> EngineError {
    error!(context, error = %error, "engine Space transition failed");
    EngineError::new(1103, EngineErrorCategory::Unavailable, true)
}

fn space_transition_error(
    context: &'static str,
    error: CompletePendingSpaceTransitionError,
) -> EngineError {
    error!(context, error = %error, "engine Space transition failed");
    match error {
        CompletePendingSpaceTransitionError::State { .. } => {
            EngineError::new(1103, EngineErrorCategory::Unavailable, true)
        }
        CompletePendingSpaceTransitionError::JoinNotActive => {
            EngineError::new(1103, EngineErrorCategory::Internal, false)
        }
    }
}

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
    network_partition_gate: uc_infra::network::iroh::IrohNetworkPartitionGate,
    #[cfg(feature = "dev-tools")]
    test_control: Arc<SessionHandoverTestControl>,
    network_recovery: Arc<uc_application::facade::NetworkRecoveryFacade>,
}

struct ProductionSession {
    facade: Arc<AppFacade>,
    application: Arc<ApplicationRuntime>,
    #[cfg(feature = "lan-compat")]
    mobile_sync: Arc<uc_mobile_lan::MobileSyncFacade>,
    sync_session: SyncSessionAssembly,
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
        crate::EngineErrorCategory::Unauthorized => DiagnosticErrorType::AuthenticationFailed,
        crate::EngineErrorCategory::Unavailable | crate::EngineErrorCategory::NotFound => {
            DiagnosticErrorType::Unavailable
        }
        crate::EngineErrorCategory::DeadlineExceeded => DiagnosticErrorType::Timeout,
        crate::EngineErrorCategory::InvalidInput => DiagnosticErrorType::DecodeFailed,
        crate::EngineErrorCategory::InvalidState | crate::EngineErrorCategory::Conflict => {
            DiagnosticErrorType::Corrupt
        }
        crate::EngineErrorCategory::Internal => DiagnosticErrorType::Internal,
    }
}

impl SessionSupervisor {
    pub(super) fn new(application: uc_application::facade::ApplicationAssembly) -> Self {
        use uc_observability_contract::diagnostics::connectivity::{
            LocalDiagnosticSource, NetworkRecorder, SourceCapability, SourceCollection,
        };
        NetworkRecorder::current().register_source(
            LocalDiagnosticSource::Sessions,
            SourceCapability::Partial,
            SourceCollection::Enabled,
        );
        Self {
            runtime: Mutex::new(SessionRuntimeState {
                network: None,
                session: None,
            }),
            factory: StdMutex::new(None),
            application,
            lifecycle: Mutex::new(()),
            operations: SessionOperationGate::new_open(),
            #[cfg(feature = "dev-tools")]
            test_control: Arc::new(SessionHandoverTestControl::default()),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn configure_factory(
        &self,
        wired: WiredDependencies,
        #[cfg(feature = "lan-compat")] paths: uc_application::facade::AppPaths,
        app_version: String,
        events: EventSender,
        rendezvous_base_url: Option<String>,
        relay_fallback_override: Option<bool>,
        iroh_bind_port_override: Option<u16>,
        #[cfg(feature = "dev-tools")]
        network_partition_gate: uc_infra::network::iroh::IrohNetworkPartitionGate,
        network_recovery: Arc<uc_application::facade::NetworkRecoveryFacade>,
    ) {
        let factory = Arc::new(ProductionSessionFactory {
            wired,
            #[cfg(feature = "lan-compat")]
            paths,
            app_version,
            events,
            rendezvous_base_url,
            relay_fallback_override,
            iroh_bind_port_override,
            #[cfg(feature = "dev-tools")]
            network_partition_gate,
            #[cfg(feature = "dev-tools")]
            test_control: Arc::clone(&self.test_control),
            network_recovery,
        });
        let mut slot = self
            .factory
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *slot = Some(factory);
    }

    pub(super) async fn current_facade(&self) -> Result<Arc<AppFacade>, EngineError> {
        self.runtime
            .lock()
            .await
            .session
            .as_ref()
            .map(|session| Arc::clone(&session.facade))
            .ok_or_else(operation_unavailable_error)
    }

    pub(super) async fn current_application(&self) -> Result<Arc<ApplicationRuntime>, EngineError> {
        self.runtime
            .lock()
            .await
            .session
            .as_ref()
            .map(|session| Arc::clone(&session.application))
            .ok_or_else(operation_unavailable_error)
    }

    pub(super) async fn current_facade_and_application(
        &self,
    ) -> Result<(Arc<AppFacade>, Arc<ApplicationRuntime>), EngineError> {
        self.runtime
            .lock()
            .await
            .session
            .as_ref()
            .map(|session| {
                (
                    Arc::clone(&session.facade),
                    Arc::clone(&session.application),
                )
            })
            .ok_or_else(operation_unavailable_error)
    }

    #[cfg(feature = "lan-compat")]
    pub(super) async fn current_mobile_sync(
        &self,
    ) -> Result<Arc<uc_mobile_lan::MobileSyncFacade>, EngineError> {
        self.runtime
            .lock()
            .await
            .session
            .as_ref()
            .map(|session| Arc::clone(&session.mobile_sync))
            .ok_or_else(operation_unavailable_error)
    }

    pub(super) fn clear_factory(&self) {
        let mut slot = self
            .factory
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *slot = None;
    }

    pub(super) async fn acquire_operation(&self) -> Result<SessionOperationLease, EngineError> {
        self.operations.acquire()
    }

    #[cfg(feature = "dev-tools")]
    pub(super) fn fail_next_session_handover(&self, point: SessionHandoverFailurePoint) {
        self.test_control.failure(point).arm();
    }

    #[cfg(feature = "dev-tools")]
    pub(super) fn session_handover_diagnostics(&self) -> SessionHandoverDiagnostics {
        SessionHandoverDiagnostics {
            network_build_count: self.test_control.network_build_count.load(Ordering::SeqCst),
            session_quiesce_failure_count: self.test_control.session_quiesce.consumed(),
            transition_completion_failure_count: self.test_control.transition_completion.consumed(),
            session_preparation_failure_count: self.test_control.session_preparation.consumed(),
            session_activation_failure_count: self.test_control.session_activation.consumed(),
        }
    }

    pub(super) async fn rebuild_session(&self) -> Result<(), EngineError> {
        observe_runtime_operation(DiagnosticOperation::SessionRecovery, async {
            let _lifecycle = self.lifecycle.lock().await;
            self.operations.close_and_wait(None).await?;
            self.stop_current_session(
                uc_core::FileTransferCancellationReason::ConnectivityRecovery,
            )
            .await?;
            self.shutdown_network().await?;
            self.install_new_session(false).await
        })
        .await
    }

    pub(super) async fn transition_pending_session(&self) -> Result<Option<u64>, EngineError> {
        let waiting = Instant::now();
        let _lifecycle = self.lifecycle.lock().await;
        let waited = waiting.elapsed();
        let (facade, network_active) = {
            let runtime = self.runtime.lock().await;
            (
                runtime
                    .session
                    .as_ref()
                    .map(|session| Arc::clone(&session.facade)),
                runtime.network.is_some(),
            )
        };
        let facade = match facade {
            Some(facade) => facade,
            None if network_active => {
                record_session_lock_wait(waited);
                self.install_new_session(false).await?;
                let revision = self
                    .current_facade()
                    .await?
                    .query_device_group_choices()
                    .await
                    .map_err(|error| {
                        operation_error_with_code(
                            1103,
                            "query recovered device trust revision",
                            error,
                        )
                    })?
                    .revision;
                return Ok(Some(revision));
            }
            None => return Ok(None),
        };
        match facade.has_pending_space_transition().await {
            Ok(false) => return Ok(None),
            Ok(true) => {}
            Err(error) => {
                let error =
                    operation_error_with_code(1103, "inspect runtime space transition", error);
                return observe_session_lifecycle(async {
                    record_session_lock_wait(waited);
                    Err(error)
                })
                .await;
            }
        }
        observe_session_lifecycle(async {
            record_session_lock_wait(waited);
            self.operations.close_and_wait(None).await?;
            match facade.has_pending_space_transition().await {
                Ok(true) => {}
                Ok(false) => {
                    self.operations.reopen();
                    return Ok(None);
                }
                Err(error) => {
                    self.operations.reopen();
                    return Err(operation_error_with_code(
                        1103,
                        "confirm runtime space transition",
                        error,
                    ));
                }
            }

            self.quiesce_network_session().await?;
            let session = self
                .runtime
                .lock()
                .await
                .session
                .take()
                .ok_or_else(super::operation_unavailable_error)?;
            session
                .shutdown(uc_core::FileTransferCancellationReason::ConnectivityRecovery)
                .await;
            let completed = self
                .complete_pending_space_transition(&facade, "complete runtime space transition")
                .await;
            match completed {
                Ok(_) => {
                    self.install_new_session(true).await?;
                    let revision = self
                        .current_facade()
                        .await?
                        .query_device_group_choices()
                        .await
                        .map_err(|error| {
                            operation_error_with_code(
                                1103,
                                "query transitioned device trust revision",
                                error,
                            )
                        })?
                        .revision;
                    Ok(Some(revision))
                }
                Err(error) => match self.install_new_session(false).await {
                    Ok(()) => Err(error),
                    Err(restore_error) => Err(restore_error),
                },
            }
        })
        .await
    }

    pub(super) async fn reset_space(
        &self,
        current_operation: SessionOperationLease,
    ) -> Result<OperationResult, EngineError> {
        let _lifecycle = self.lifecycle.lock().await;
        self.operations
            .close_and_wait(Some(current_operation))
            .await?;
        self.quiesce_network_session().await?;
        let session = self
            .runtime
            .lock()
            .await
            .session
            .take()
            .ok_or_else(super::operation_unavailable_error)?;
        let facade = Arc::clone(&session.facade);
        session
            .shutdown(uc_core::FileTransferCancellationReason::ConnectivityRecovery)
            .await;
        let transfer_result = self
            .application
            .cancel_active_file_transfers(
                uc_core::FileTransferCancellationReason::ConnectivityRecovery,
            )
            .await
            .map_err(|error| {
                operation_error_with_code(1104, "cancel active file transfers", error)
            });
        if let Err(error) = transfer_result {
            return match self.install_new_session(false).await {
                Ok(()) => Err(error),
                Err(install_error) => Err(install_error),
            };
        }

        let result = crate::operations::space::reset_space::execute_reset_space(&facade).await;
        let install_result = self.install_new_session(result.is_ok()).await;
        match (result, install_result) {
            (Ok(result), Ok(())) => Ok(result),
            (Err(error), Ok(())) => {
                let facade = self
                    .runtime
                    .lock()
                    .await
                    .session
                    .as_ref()
                    .map(|session| Arc::clone(&session.facade))
                    .ok_or_else(super::operation_unavailable_error)?;
                match facade.has_committed_device_management_reset().await {
                    Ok(true) => {
                        crate::operations::space::reset_space::execute_reset_space(&facade).await
                    }
                    Ok(false) | Err(_) => Err(error),
                }
            }
            (_, Err(error)) => Err(error),
        }
    }

    pub(super) async fn suspend(&self) -> Result<(), EngineError> {
        observe_session_request(
            uc_observability_contract::diagnostics::connectivity::SessionTransition::Suspend,
            async {
                let _lifecycle = self.lifecycle.lock().await;
                self.operations.close_and_wait(None).await?;
                self.stop_current_session(uc_core::FileTransferCancellationReason::Unknown)
                    .await?;
                self.shutdown_network().await?;
                Ok(uc_observability_contract::diagnostics::connectivity::SessionTransitionResult::Completed)
            },
        )
        .await
    }

    pub(super) async fn resume(&self) -> Result<(), EngineError> {
        observe_session_request(
            uc_observability_contract::diagnostics::connectivity::SessionTransition::Resume,
            async {
                let _lifecycle = self.lifecycle.lock().await;
                if self.runtime.lock().await.session.is_some() {
                    return Ok(uc_observability_contract::diagnostics::connectivity::SessionTransitionResult::Skipped);
                }
                observe_runtime_operation(
                    DiagnosticOperation::SessionLifecycle,
                    self.install_new_session(false),
                )
                .await.map(|()| uc_observability_contract::diagnostics::connectivity::SessionTransitionResult::Completed)
            },
        )
        .await
    }

    pub(super) async fn close_file_transfers(&self) -> Result<(), EngineError> {
        self.application
            .close_file_transfers()
            .await
            .map_err(|error| operation_error_with_code(1104, "close file transfers", error))
    }

    async fn stop_current_session(
        &self,
        reason: uc_core::FileTransferCancellationReason,
    ) -> Result<(), EngineError> {
        self.quiesce_network_session().await?;
        let session = self.runtime.lock().await.session.take();
        if let Some(session) = session {
            session.shutdown(reason).await;
            self.application
                .cancel_active_file_transfers(reason)
                .await
                .map_err(|error| {
                    operation_error_with_code(1104, "cancel active file transfers", error)
                })?;
        }
        Ok(())
    }

    async fn quiesce_network_session(&self) -> Result<(), EngineError> {
        #[cfg(feature = "dev-tools")]
        if self
            .test_control
            .failure(SessionHandoverFailurePoint::SessionQuiesce)
            .consume()
        {
            return Err(session_runtime_error(
                "quiesce p2p session",
                "injected session quiesce failure",
            ));
        }
        let mut runtime = self.runtime.lock().await;
        let Some(network) = runtime.network.as_mut() else {
            return Ok(());
        };
        network
            .quiesce_session()
            .await
            .map_err(|error| session_runtime_error("quiesce p2p session", error))
    }

    async fn shutdown_network(&self) -> Result<(), EngineError> {
        let network = {
            let mut runtime = self.runtime.lock().await;
            if runtime.session.is_some() {
                return Err(operation_unavailable_error());
            }
            runtime.network.take()
        };
        if let Some(network) = network {
            network.shutdown().await;
        }
        Ok(())
    }

    async fn complete_pending_space_transition(
        &self,
        facade: &AppFacade,
        error_context: &'static str,
    ) -> Result<(), EngineError> {
        #[cfg(feature = "dev-tools")]
        if self
            .test_control
            .failure(SessionHandoverFailurePoint::TransitionCompletion)
            .consume()
        {
            return Err(retryable_space_transition_runtime_error(
                error_context,
                "injected transition completion failure",
            ));
        }
        observe_local_result(
            LocalWorkStep::SessionCompleteTransition,
            facade.complete_pending_space_transition(),
        )
        .await
        .map(|_| ())
        .map_err(|error| space_transition_error(error_context, error))
    }

    async fn install_active_session(
        &self,
        factory: &ProductionSessionFactory,
    ) -> Result<(), EngineError> {
        let session_builder = {
            let mut runtime = self.runtime.lock().await;
            if runtime.session.is_some() {
                return Err(operation_unavailable_error());
            }
            if runtime.network.is_none() {
                runtime.network = Some(factory.build_network().await?);
            }
            runtime
                .network
                .as_ref()
                .ok_or_else(operation_unavailable_error)?
                .prepare_session()
        };
        let prepared = factory.prepare(session_builder).await?;
        #[cfg(feature = "dev-tools")]
        if self
            .test_control
            .failure(SessionHandoverFailurePoint::SessionActivation)
            .consume()
        {
            prepared.network_session.shutdown().await;
            prepared
                .session
                .shutdown(uc_core::FileTransferCancellationReason::Unknown)
                .await;
            return Err(session_runtime_error(
                "activate p2p session",
                "injected session activation failure",
            ));
        }
        let mut runtime = self.runtime.lock().await;
        let Some(network) = runtime.network.as_mut() else {
            drop(runtime);
            prepared.network_session.shutdown().await;
            prepared
                .session
                .shutdown(uc_core::FileTransferCancellationReason::Unknown)
                .await;
            return Err(operation_unavailable_error());
        };
        let activation = network
            .activate_session(prepared.network_session)
            .await
            .map_err(|error| session_runtime_error("activate p2p session", error));
        if let Err(error) = activation {
            drop(runtime);
            prepared
                .session
                .shutdown(uc_core::FileTransferCancellationReason::Unknown)
                .await;
            return Err(error);
        }
        runtime.session = Some(prepared.session);
        Ok(())
    }

    async fn install_new_session(&self, resume_space_activities: bool) -> Result<(), EngineError> {
        let factory = self
            .factory
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .ok_or_else(super::operation_unavailable_error)?;
        observe_local_result(
            LocalWorkStep::SessionPrepare,
            self.install_active_session(&factory),
        )
        .await?;
        let mut resume_space_activities = resume_space_activities;
        let facade = self.current_facade().await?;
        if facade
            .has_pending_space_transition()
            .await
            .map_err(|error| {
                operation_error_with_code(1103, "inspect pending space transition", error)
            })?
        {
            self.quiesce_network_session().await?;
            let session = self
                .runtime
                .lock()
                .await
                .session
                .take()
                .ok_or_else(operation_unavailable_error)?;
            session
                .shutdown(uc_core::FileTransferCancellationReason::ConnectivityRecovery)
                .await;
            self.complete_pending_space_transition(&facade, "recover pending space transition")
                .await?;
            observe_local_result(
                LocalWorkStep::SessionPrepare,
                self.install_active_session(&factory),
            )
            .await?;
            resume_space_activities = true;
        }
        let facade = self.current_facade().await?;
        let recovered = observe_local_result(
            LocalWorkStep::SessionRecover,
            facade.recover_space_session(),
        )
        .await;
        if let Err(error) = &recovered {
            tracing::warn!(
                error_kind = recover_space_session_error_kind(error),
                "space session recovery failed; runtime remains locked"
            );
        }
        if resume_space_activities {
            let recovered = recovered.map_err(|error| {
                operation_error_with_code(1103, "activate transitioned space session", error)
            })?;
            if !recovered.unlocked {
                return Err(operation_error_with_code(
                    1103,
                    "activate transitioned space session",
                    "the transitioned space could not be unlocked",
                ));
            }
        }
        self.operations.reopen();
        Ok(())
    }
}

impl ProductionSessionFactory {
    async fn build_network(&self) -> Result<IrohNode, EngineError> {
        #[cfg(feature = "dev-tools")]
        self.test_control
            .network_build_count
            .fetch_add(1, Ordering::SeqCst);
        build_network_runtime(
            &self.wired.application,
            &self.wired.sync_engine,
            self.rendezvous_base_url.clone(),
            self.relay_fallback_override,
            self.iroh_bind_port_override,
            #[cfg(feature = "dev-tools")]
            Some(self.network_partition_gate.clone()),
            #[cfg(not(feature = "dev-tools"))]
            None,
        )
        .await
        .map_err(|error| session_runtime_error("p2p network", error))
    }

    async fn prepare(
        &self,
        session_builder: IrohSessionBuilder,
    ) -> Result<PreparedProductionSession, EngineError> {
        #[cfg(feature = "dev-tools")]
        if self
            .test_control
            .failure(SessionHandoverFailurePoint::SessionPreparation)
            .consume()
        {
            return Err(session_runtime_error(
                "p2p session",
                "injected session preparation failure",
            ));
        }
        let wired = &self.wired;
        #[cfg(feature = "lan-compat")]
        let paths = &self.paths;
        let events = self.events.clone();
        let application = wired.application.clone();
        let prepared = prepare_daemon_session(
            &application,
            &wired.sync_engine,
            &self.app_version,
            #[cfg(feature = "lan-compat")]
            wired.mobile_sync_ports.clone(),
            session_builder,
        )
        .await
        .map_err(|error| session_runtime_error("p2p session", error))?;
        let sync_session = prepared.session;
        let network_adapters = prepared.application;
        let prepared_session = prepared.prepared_session;
        let application_runtime = match observe_local_result(
            LocalWorkStep::SessionStart,
            ApplicationRuntime::start(
                &application,
                network_adapters.binding.complete(
                    network_adapters.active_pull_client,
                    Arc::clone(&self.network_recovery),
                    FsAtomicPublisher::new(),
                    FsInboundFileTarget::new(Arc::clone(&wired.sync_engine.settings)),
                    FsHiddenPathMarker::new(),
                    Arc::new(EngineClipboardInboundEvents {
                        events: events.clone(),
                    }),
                ),
            ),
        )
        .await
        {
            Ok(runtime) => Arc::new(runtime),
            Err(error) => {
                sync_session
                    .shutdown(uc_core::FileTransferCancellationReason::Unknown)
                    .await;
                return Err(session_runtime_error("application runtime", error));
            }
        };
        let facade = application_runtime.facade();
        #[cfg(feature = "lan-compat")]
        let mobile_sync = build_mobile_sync_facade(
            &wired.mobile_sync_application,
            paths,
            wired.mobile_sync_ports.clone(),
            application_runtime.inbound_clipboard(),
            Some(facade.file_transfer_for_lan_compatibility()),
            None,
            Some(facade.clipboard_outbound_for_lan_compatibility()),
            Some(facade.active_clipboard_for_lan_compatibility()),
        );
        let tasks = Arc::new(TaskRegistry::new());
        let mut active_clipboard_changes = wired.shared.active_clipboard_sse_source.subscribe();
        let active_clipboard_events = events.clone();
        let _ = tasks
            .spawn(move |cancel| async move {
                loop {
                    tokio::select! {
                        _ = cancel.cancelled() => return,
                        change = active_clipboard_changes.recv() => match change {
                            Ok(state) => active_clipboard_events
                                .send(engine_event_for_active_clipboard(&state)),
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                                active_clipboard_events.send(crate::EngineEvent::RefreshRequired {
                                    reason: crate::RefreshReason::ConsumerLagged,
                                });
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                        }
                    }
                }
            })
            .await;
        spawn_peer_reachability_event_task(Arc::clone(&facade), &tasks, events).await;
        Ok(PreparedProductionSession {
            session: ProductionSession {
                facade,
                application: application_runtime,
                #[cfg(feature = "lan-compat")]
                mobile_sync,
                sync_session,
                tasks,
            },
            network_session: prepared_session,
        })
    }
}

impl ProductionSession {
    async fn shutdown(self, transfer_reason: uc_core::FileTransferCancellationReason) {
        info!("Engine session 开始关闭");
        #[cfg(feature = "lan-compat")]
        if self
            .mobile_sync
            .shutdown_mobile_file_uploads()
            .await
            .is_err()
        {
            warn!("mobile file upload shutdown finished with an error");
        }
        let stopping = LocalWorkObservation::begin(LocalWorkStep::SessionStopTasks);
        let stopped =
            super::task_shutdown::shutdown_tasks(&self.tasks, Duration::from_millis(500)).await;
        stopping.finish(
            if stopped.timed_out_count > 0 || stopped.join_error_count > 0 {
                LocalWorkOutcome::Error
            } else {
                LocalWorkOutcome::Ok
            },
        );
        info!("Engine session 网络观测任务已停止");
        let stopping = LocalWorkObservation::begin(LocalWorkStep::SessionStopApplication);
        let application_shutdown = self.application.shutdown().await;
        stopping.finish(
            if application_shutdown.history.is_some() || application_shutdown.search.is_some() {
                LocalWorkOutcome::Error
            } else {
                LocalWorkOutcome::Ok
            },
        );
        info!("Engine session Application runtime 已停止");
        if application_shutdown.history.is_some() {
            warn!(
                error_kind = "history",
                "history maintenance stopped with an error"
            );
        }
        if application_shutdown.search.is_some() {
            error!(error_kind = "search", "search runtime stopped with error");
        }
        let stopping = LocalWorkObservation::begin(LocalWorkStep::SessionStopNetwork);
        self.sync_session.shutdown(transfer_reason).await;
        stopping.finish(LocalWorkOutcome::Ok);
        info!("Engine session 网络会话任务已停止");
    }
}

fn engine_event_for_active_clipboard(
    state: &uc_core::clipboard::ActiveClipboardState,
) -> crate::EngineEvent {
    crate::EngineEvent::ActiveClipboardChanged(crate::ActiveClipboardChanged {
        snapshot_hash: state.snapshot_hash.clone(),
        entry_id: state.entry_id.as_str().to_string(),
        activated_at_ms: state.activated_at_ms,
        activated_by: state.activated_by.as_str().to_string(),
    })
}

struct EngineClipboardInboundEvents {
    events: EventSender,
}

impl ClipboardInboundEventPort for EngineClipboardInboundEvents {
    fn emit(&self, event: ClipboardInboundEvent) {
        self.events
            .send(EngineEvent::InboundNotice(InboundNoticeEvent {
                from_device: event.from_device.as_str().to_owned(),
                snapshot_hash: event.snapshot_hash,
                text_preview: event.text_preview,
                representations: event
                    .representations
                    .into_iter()
                    .map(|representation| InboundRepresentationSummary {
                        mime_type: representation.mime_type,
                        size_bytes: representation.size_bytes,
                    })
                    .collect(),
                action: match event.action {
                    ClipboardInboundEventAction::NewEntry => InboundNoticeActionSummary::NewEntry,
                    ClipboardInboundEventAction::DuplicateIgnored => {
                        InboundNoticeActionSummary::DuplicateIgnored
                    }
                },
                at_ms: event.at_ms,
            }));
    }
}

fn recover_space_session_error_kind(
    error: &uc_application::facade::RecoverSpaceSessionError,
) -> &'static str {
    use uc_application::facade::RecoverSpaceSessionError;

    match error {
        RecoverSpaceSessionError::CurrentSpace(_) => "current_space",
        RecoverSpaceSessionError::KeyringMiss => "keyring_miss",
        RecoverSpaceSessionError::CorruptedKeyMaterial => "corrupted_key_material",
        RecoverSpaceSessionError::Activity(_) => "activity",
        RecoverSpaceSessionError::Internal(_) => "internal",
    }
}

#[async_trait::async_trait]
impl uc_application::facade::RebuildNetworkSessionPort for SessionSupervisor {
    async fn rebuild_network_session(
        &self,
    ) -> Result<(), uc_application::facade::RebuildNetworkSessionError> {
        self.rebuild_session().await.map_err(|error| {
            if error.is_retryable() {
                uc_application::facade::RebuildNetworkSessionError::Retryable
            } else {
                uc_application::facade::RebuildNetworkSessionError::Permanent
            }
        })
    }
}

impl SessionOperationGate {
    fn new_open() -> Self {
        Self {
            inner: Arc::new(SessionOperationGateInner {
                state: StdMutex::new(SessionOperationGateState {
                    open: true,
                    active: 0,
                    cancellation: CancellationToken::new(),
                }),
                changed: Notify::new(),
            }),
        }
    }

    fn acquire(&self) -> Result<SessionOperationLease, EngineError> {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !state.open {
            return Err(super::operation_unavailable_error());
        }
        state.active = state.active.saturating_add(1);
        Ok(SessionOperationLease {
            gate: Arc::clone(&self.inner),
            cancellation: state.cancellation.clone(),
        })
    }

    fn reopen(&self) {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.open = true;
        state.cancellation = CancellationToken::new();
        self.inner.changed.notify_waiters();
    }

    async fn close_and_wait(
        &self,
        current_operation: Option<SessionOperationLease>,
    ) -> Result<(), EngineError> {
        observe_local_result(LocalWorkStep::SessionDrainOperations, async {
            if current_operation
                .as_ref()
                .is_some_and(|lease| !Arc::ptr_eq(&lease.gate, &self.inner))
            {
                return Err(super::operation_unavailable_error());
            }
            let cancellation = {
                let mut state = self
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state.open = false;
                state.cancellation.clone()
            };
            drop(current_operation);
            let drained = observe_local_result(
                LocalWorkStep::SessionDrainGrace,
                tokio::time::timeout(SESSION_OPERATION_GRACE, self.wait_for_drain()),
            )
            .await;
            if drained.is_err() {
                cancellation.cancel();
                if observe_local_result(
                    LocalWorkStep::SessionDrainCancellation,
                    tokio::time::timeout(SESSION_OPERATION_GRACE, self.wait_for_drain()),
                )
                .await
                .is_err()
                {
                    tracing::warn!(
                        error_kind = "session_operation_drain_timeout",
                        "session operation did not stop after cancellation"
                    );
                }
            }
            Ok(())
        })
        .await
    }

    async fn wait_for_drain(&self) {
        loop {
            let notified = self.inner.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let active = self
                .inner
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .active;
            if active == 0 {
                return;
            }
            notified.await;
        }
    }
}

impl SessionOperationLease {
    pub(super) fn cancellation(&self) -> CancellationToken {
        self.cancellation.clone()
    }
}

impl Drop for SessionOperationLease {
    fn drop(&mut self) {
        let mut state = self
            .gate
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.active = state.active.saturating_sub(1);
        drop(state);
        self.gate.changed.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tracing::instrument::WithSubscriber;

    use super::*;

    #[derive(Clone, Default)]
    struct CapturedWriter(Arc<StdMutex<Vec<u8>>>);

    impl Write for CapturedWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'writer> tracing_subscriber::fmt::MakeWriter<'writer> for CapturedWriter {
        type Writer = Self;

        fn make_writer(&'writer self) -> Self::Writer {
            self.clone()
        }
    }

    #[tokio::test]
    async fn session_lifecycle_records_an_early_failure_as_the_total_result() {
        let writer = CapturedWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(writer.clone())
            .finish();

        let result =
            observe_session_lifecycle(async { Err::<(), _>(operation_unavailable_error()) })
                .with_subscriber(subscriber)
                .await;

        assert!(result.is_err());
        let output = String::from_utf8(
            writer
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
        )
        .expect("UTF-8 logs");
        assert!(output.contains("uc.operation=\"session_lifecycle\""));
        assert!(output.contains("uc.outcome=\"error\""));
        assert!(output.contains("error.type=\"unavailable\""));
    }

    #[tokio::test]
    async fn requested_session_transition_records_start_and_failure_without_hiding_the_result() {
        let writer = CapturedWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(writer.clone())
            .finish();
        let result = observe_session_request(
            uc_observability_contract::diagnostics::connectivity::SessionTransition::Suspend,
            async {
                Err::<
                    uc_observability_contract::diagnostics::connectivity::SessionTransitionResult,
                    _,
                >(operation_unavailable_error())
            },
        )
        .with_subscriber(subscriber)
        .await;
        assert!(result.is_err());
        let output = String::from_utf8(writer.0.lock().expect("capture").clone()).expect("logs");
        assert!(output.contains("suspend"));
        assert!(output.contains("session.transition.started"));
        assert!(output.contains("failed"));
        assert!(
            output.contains("unavailable"),
            "the known failure category must remain visible"
        );
        assert_eq!(output.lines().count(), 2);
    }

    #[tokio::test]
    async fn cancelled_session_transition_records_interruption_instead_of_success() {
        let writer = CapturedWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(writer.clone())
            .finish();
        let _subscriber = tracing::subscriber::set_default(subscriber);
        let result = tokio::time::timeout(std::time::Duration::from_millis(5), observe_session_request(
            uc_observability_contract::diagnostics::connectivity::SessionTransition::Suspend,
            std::future::pending::<Result<uc_observability_contract::diagnostics::connectivity::SessionTransitionResult, EngineError>>(),
        )).await;
        assert!(result.is_err());
        let output = String::from_utf8(writer.0.lock().expect("capture").clone()).expect("logs");
        assert!(
            output.contains("interrupted"),
            "dropping the operation must not leave a false active transition"
        );
        assert!(!output.contains("completed"));
    }

    #[test]
    fn session_runtime_failure_remains_retryable_without_becoming_an_operation_error() {
        let error = session_runtime_error("prepare p2p session", "test failure");

        assert_eq!(error.code(), SESSION_RUNTIME_FAILED_CODE);
        assert_eq!(error.category(), EngineErrorCategory::Unavailable);
        assert!(error.is_retryable());
    }

    #[test]
    fn space_transition_runtime_failure_remains_retryable_for_background_recovery() {
        let error =
            retryable_space_transition_runtime_error("complete Space transition", "test failure");

        assert_eq!(error.code(), 1103);
        assert_eq!(error.category(), EngineErrorCategory::Unavailable);
        assert!(error.is_retryable());
    }

    #[test]
    fn missing_active_join_stops_background_transition_recovery() {
        let error = space_transition_error(
            "complete Space transition",
            CompletePendingSpaceTransitionError::JoinNotActive,
        );

        assert_eq!(error.code(), 1103);
        assert_eq!(error.category(), EngineErrorCategory::Internal);
        assert!(!error.is_retryable());
    }

    #[test]
    fn active_clipboard_event_preserves_mobile_sse_identity() {
        let state = uc_core::clipboard::ActiveClipboardState::new(
            "hash-1",
            uc_core::ids::EntryId::from("entry-1"),
            42,
            uc_core::ids::DeviceId::new("device-1"),
        );

        assert_eq!(
            engine_event_for_active_clipboard(&state),
            crate::EngineEvent::ActiveClipboardChanged(crate::ActiveClipboardChanged {
                snapshot_hash: "hash-1".into(),
                entry_id: "entry-1".into(),
                activated_at_ms: 42,
                activated_by: "device-1".into(),
            })
        );
    }

    #[tokio::test]
    async fn closed_gate_rejects_new_operations_and_waits_for_the_existing_one() {
        let gate = Arc::new(SessionOperationGate::new_open());
        let lease = gate.acquire().expect("new gate accepts an operation");
        let closing = tokio::spawn({
            let gate = Arc::clone(&gate);
            async move { gate.close_and_wait(None).await }
        });
        tokio::task::yield_now().await;
        assert!(gate.acquire().is_err());
        assert!(!closing.is_finished());

        drop(lease);
        match closing.await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => panic!("operation gate close failed: {error}"),
            Err(error) => panic!("operation gate close task did not complete: {error}"),
        }
    }

    // 流程：会话关闭后仍有操作忽略取消信号；等待两段固定期限后关闭必须继续完成。
    #[tokio::test(start_paused = true)]
    async fn closed_gate_stops_waiting_when_an_operation_ignores_cancellation() {
        let gate = Arc::new(SessionOperationGate::new_open());
        let _lease = gate.acquire().expect("new gate accepts an operation");

        tokio::time::timeout(
            SESSION_OPERATION_GRACE.saturating_mul(2) + Duration::from_millis(1),
            gate.close_and_wait(None),
        )
        .await
        .expect("gate close must remain bounded after cancellation")
        .expect("gate close must accept no current operation");
    }

    #[tokio::test]
    async fn transition_operation_excludes_itself_but_waits_for_other_operations() {
        let gate = Arc::new(SessionOperationGate::new_open());
        let transition = gate.acquire().expect("transition acquires ordinary lease");
        let other = gate.acquire().expect("concurrent operation acquires lease");
        let closing = tokio::spawn({
            let gate = Arc::clone(&gate);
            async move { gate.close_and_wait(Some(transition)).await }
        });

        tokio::task::yield_now().await;
        assert!(gate.acquire().is_err());
        assert!(!closing.is_finished());

        drop(other);
        closing.await.unwrap().unwrap();
    }
}
