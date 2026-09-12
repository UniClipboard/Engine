use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use tokio::sync::{Mutex, Notify};
use tokio::time::{timeout_at, Instant};
use tokio_util::sync::CancellationToken;
use tracing::Instrument;
use uc_application::facade::{
    AppFacade, ApplicationAssembly, ApplicationRuntime, ClipboardInboundEvent,
    ClipboardInboundEventAction, ClipboardInboundEventPort, LifecycleError, LifecycleTarget,
    RecoverSpaceSessionError, RuntimeLifecycleCoordinator, RuntimeLifecycleParticipants,
};
use uc_core::{FileTransferCancellationReason, TaskRegistry};
use uc_infra::fs::{FsAtomicPublisher, FsHiddenPathMarker, FsInboundFileTarget};
use uc_infra::space::RuntimeSpaceAccessAdapter;
use uc_observability_contract::diagnostics::connectivity::{
    SessionTransition, SessionTransitionResult,
};
use uc_observability_contract::diagnostics::{
    complete_operation, operation_span, DiagnosticDomain, DiagnosticErrorType, DiagnosticOperation,
    DiagnosticRole, DiagnosticSpanKind, OperationCompletion, OperationContext,
};

use crate::assembly::deps::WiredDependencies;
#[cfg(feature = "lan-compat")]
use crate::assembly::facade::build_mobile_sync_facade;
use crate::assembly::lifecycle::build_daemon_lifecycle;
use crate::assembly::sync_engine::SyncEngineAssembly;
use crate::engine::event_stream::EventSender;
use crate::subsystems::peer_keepalive::spawn_peer_reachability_event_task;
use crate::{
    EngineEvent, InboundNoticeActionSummary, InboundNoticeEvent, InboundRepresentationSummary,
};

use super::{operation_error_with_code, operation_unavailable_error, startup_error};
use crate::{EngineError, EngineErrorCategory, OperationResult};

const SESSION_OPERATION_GRACE: Duration = Duration::from_secs(2);

mod lifecycle;
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
    network_partition_gate: uc_infra::network::iroh::IrohNetworkPartitionGate,
    network_recovery: Arc<uc_application::facade::NetworkRecoveryFacade>,
}

struct ProductionSession {
    facade: Arc<AppFacade>,
    application: Arc<ApplicationRuntime>,
    #[cfg(feature = "lan-compat")]
    mobile_sync: Arc<uc_mobile_lan::MobileSyncFacade>,
    sync_engine: SyncEngineAssembly,
    tasks: Arc<TaskRegistry>,
}

pub(super) struct SessionSupervisor {
    session: Arc<Mutex<Option<ProductionSession>>>,
    factory: StdMutex<Option<Arc<ProductionSessionFactory>>>,
    application: uc_application::facade::ApplicationAssembly,
    lifecycle: Mutex<()>,
    operations: SessionOperationGate,
    suspended: AtomicBool,
    coordinator: Arc<RuntimeLifecycleCoordinator>,
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
    pub(super) fn new(
        application: ApplicationAssembly,
        security: Arc<RuntimeSpaceAccessAdapter>,
    ) -> Arc<Self> {
        use uc_observability_contract::diagnostics::connectivity::{
            LocalDiagnosticSource, NetworkRecorder, SourceCapability, SourceCollection,
        };
        NetworkRecorder::current().register_source(
            LocalDiagnosticSource::Sessions,
            SourceCapability::Partial,
            SourceCollection::Enabled,
        );
        Arc::new_cyclic(|owner| Self {
            session: Arc::new(Mutex::new(None)),
            factory: StdMutex::new(None),
            application: application.clone(),
            lifecycle: Mutex::new(()),
            operations: SessionOperationGate::new_open(),
            suspended: AtomicBool::new(true),
            coordinator: Arc::new(RuntimeLifecycleCoordinator::new(
                RuntimeLifecycleParticipants {
                    session_work: Arc::new(SessionWork(owner.clone())),
                    local_work: Arc::new(application),
                    local_resources: security,
                },
            )),
        })
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
            network_recovery,
        });
        let mut slot = self
            .factory
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *slot = Some(factory);
    }

    pub(super) async fn current_facade(&self) -> Result<Arc<AppFacade>, EngineError> {
        self.session
            .lock()
            .await
            .as_ref()
            .map(|session| Arc::clone(&session.facade))
            .ok_or_else(operation_unavailable_error)
    }

    pub(super) async fn current_application(&self) -> Result<Arc<ApplicationRuntime>, EngineError> {
        self.session
            .lock()
            .await
            .as_ref()
            .map(|session| Arc::clone(&session.application))
            .ok_or_else(operation_unavailable_error)
    }

    pub(super) async fn current_facade_and_application(
        &self,
    ) -> Result<(Arc<AppFacade>, Arc<ApplicationRuntime>), EngineError> {
        self.session
            .lock()
            .await
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
        self.session
            .lock()
            .await
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

    pub(super) async fn rebuild_session(&self) -> Result<(), EngineError> {
        observe_runtime_operation(DiagnosticOperation::SessionRecovery, async {
            let _lifecycle = self.lifecycle.lock().await;
            if self.suspended.load(Ordering::Acquire) {
                return Err(operation_unavailable_error());
            }
            self.operations.close_and_wait(None, None).await?;
            self.stop_current_session(
                uc_core::FileTransferCancellationReason::ConnectivityRecovery,
                None,
            )
            .await
            .map_err(lifecycle_error)?;
            self.install_new_session(false).await
        })
        .await
    }

    pub(super) async fn transition_pending_session(&self) -> Result<Option<u64>, EngineError> {
        let _lifecycle = self.lifecycle.lock().await;
        let facade = match self.session.lock().await.as_ref() {
            Some(session) => Arc::clone(&session.facade),
            None => return Ok(None),
        };
        match facade.has_pending_space_transition().await {
            Ok(false) => return Ok(None),
            Ok(true) => {}
            Err(error) => {
                let error =
                    operation_error_with_code(1103, "inspect runtime space transition", error);
                return observe_session_lifecycle(async { Err(error) }).await;
            }
        }
        observe_session_lifecycle(async {
            self.operations.close_and_wait(None, None).await?;
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

            let session = self
                .session
                .lock()
                .await
                .take()
                .ok_or_else(super::operation_unavailable_error)?;
            session
                .shutdown(FileTransferCancellationReason::ConnectivityRecovery, None)
                .await
                .map_err(lifecycle_error)?;
            let completed = facade.complete_pending_space_transition().await;
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
                Err(error) => {
                    let original =
                        operation_error_with_code(1103, "complete runtime space transition", error);
                    match self.install_new_session(false).await {
                        Ok(()) => Err(original),
                        Err(restore_error) => Err(restore_error),
                    }
                }
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
            .close_and_wait(Some(current_operation), None)
            .await?;
        let session = self
            .session
            .lock()
            .await
            .take()
            .ok_or_else(super::operation_unavailable_error)?;
        let facade = Arc::clone(&session.facade);
        session
            .shutdown(FileTransferCancellationReason::ConnectivityRecovery, None)
            .await
            .map_err(lifecycle_error)?;
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
                    .session
                    .lock()
                    .await
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

    pub(super) async fn suspend(&self, deadline: Option<Instant>) -> Result<(), EngineError> {
        observe_session_request(SessionTransition::Suspend, async {
            self.coordinator
                .transition(LifecycleTarget::Suspended, deadline)
                .await
                .map_err(lifecycle_error)?;
            Ok(SessionTransitionResult::Completed)
        })
        .await
    }

    pub(super) async fn resume(&self) -> Result<(), EngineError> {
        observe_session_request(SessionTransition::Resume, async {
            self.coordinator
                .transition(LifecycleTarget::Active, None)
                .await
                .map_err(lifecycle_error)?;
            Ok(SessionTransitionResult::Completed)
        })
        .await
    }

    pub(super) async fn close_file_transfers(&self) -> Result<(), EngineError> {
        self.application
            .close_file_transfers()
            .await
            .map_err(|error| operation_error_with_code(1104, "close file transfers", error))
    }

    pub(super) async fn stop(&self, deadline: Option<Instant>) -> Result<(), LifecycleError> {
        self.coordinator.stop(deadline).await
    }

    fn configured_factory(&self) -> Option<Arc<ProductionSessionFactory>> {
        self.factory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    async fn stop_current_session(
        &self,
        reason: FileTransferCancellationReason,
        deadline: Option<Instant>,
    ) -> Result<(), LifecycleError> {
        let session = self.session.lock().await.take();
        let mut errors = Vec::new();
        if let Some(session) = session {
            if let Err(error) = session.shutdown(reason, deadline).await {
                errors.push(error.into());
            }
        }
        if let Err(error) = self.application.cancel_active_file_transfers(reason).await {
            errors.push(anyhow::Error::new(error).context("cancel active file transfers"));
        }
        LifecycleError::from_errors(errors)
    }

    async fn install_new_session(&self, resume_space_activities: bool) -> Result<(), EngineError> {
        let factory = self
            .configured_factory()
            .ok_or_else(super::operation_unavailable_error)?;
        let mut session = factory.build().await?;
        let mut resume_space_activities = resume_space_activities;
        let pending_transition = match session.facade.has_pending_space_transition().await {
            Ok(pending) => pending,
            Err(error) => {
                let primary =
                    operation_error_with_code(1103, "inspect pending space transition", error);
                return Err(session.shutdown_after_failure(primary).await);
            }
        };
        if pending_transition {
            let facade = Arc::clone(&session.facade);
            session
                .shutdown(FileTransferCancellationReason::ConnectivityRecovery, None)
                .await
                .map_err(lifecycle_error)?;
            facade
                .complete_pending_space_transition()
                .await
                .map_err(|error| {
                    operation_error_with_code(1103, "recover pending space transition", error)
                })?;
            session = factory.build().await?;
            resume_space_activities = true;
        }
        let unlocked = match session.facade.recover_space_session().await {
            Ok(recovered) => recovered.unlocked,
            // 缺少缓存口令是正常锁定；存储故障和损坏不能降级为恢复成功。
            Err(RecoverSpaceSessionError::KeyringMiss) if !resume_space_activities => false,
            Err(error) => {
                let primary = operation_error_with_code(1103, "recover local session", error);
                return Err(session.shutdown_after_failure(primary).await);
            }
        };
        if resume_space_activities && !unlocked {
            let primary = operation_error_with_code(
                1103,
                "activate transitioned space session",
                "the transitioned space could not be unlocked",
            );
            return Err(session.shutdown_after_failure(primary).await);
        }
        *self.session.lock().await = Some(session);
        self.operations.reopen();
        Ok(())
    }
}

impl ProductionSessionFactory {
    async fn build(&self) -> Result<ProductionSession, EngineError> {
        let wired = &self.wired;
        #[cfg(feature = "lan-compat")]
        let paths = &self.paths;
        let events = self.events.clone();
        let application = wired.application.clone();
        let lifecycle = build_daemon_lifecycle(
            &application,
            &wired.sync_engine,
            &self.app_version,
            #[cfg(feature = "lan-compat")]
            wired.mobile_sync_ports.clone(),
            self.rendezvous_base_url.clone(),
            self.relay_fallback_override,
            self.iroh_bind_port_override,
            #[cfg(feature = "dev-tools")]
            Some(self.network_partition_gate.clone()),
            #[cfg(not(feature = "dev-tools"))]
            None,
        )
        .await
        .map_err(|error| startup_error("p2p session", error))?;
        let sync_engine = lifecycle.sync_engine_assembly;
        let network_adapters = lifecycle.application_adapters;
        let application_runtime = match ApplicationRuntime::start(
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
        )
        .await
        {
            Ok(runtime) => Arc::new(runtime),
            Err(error) => {
                let primary = startup_error("application runtime", error);
                let additional = sync_engine
                    .shutdown(FileTransferCancellationReason::Unknown)
                    .await
                    .err()
                    .map(anyhow::Error::new)
                    .into_iter()
                    .collect();
                return Err(lifecycle_error(LifecycleError {
                    primary: primary.into(),
                    additional,
                }));
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
        Ok(ProductionSession {
            facade,
            application: application_runtime,
            #[cfg(feature = "lan-compat")]
            mobile_sync,
            sync_engine,
            tasks,
        })
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

async fn spawn_network_recovery_observation_task(
    mut observations: tokio::sync::broadcast::Receiver<
        uc_infra::network::iroh::NetworkRecoveryObservation,
    >,
    recovery: Arc<uc_application::facade::NetworkRecoveryFacade>,
    generation: Arc<AtomicU64>,
    tasks: &Arc<TaskRegistry>,
) {
    let _ = tasks
        .spawn(move |cancel| async move {
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    observation = observations.recv() => match observation {
                        Ok(uc_infra::network::iroh::NetworkRecoveryObservation::LocalRelayRecovered) => {
                            let current_generation = generation.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
                            recovery.observe_local_network_recovered(current_generation).await;
                        }
                        Ok(uc_infra::network::iroh::NetworkRecoveryObservation::PreviouslyOnlinePeerPathExhausted) => {
                            recovery.observe_previously_online_peer_path_exhausted(generation.load(Ordering::Relaxed)).await;
                        }
                        Ok(uc_infra::network::iroh::NetworkRecoveryObservation::FreshPeerDialSucceeded) => {
                            recovery.observe_fresh_peer_dial_succeeded(generation.load(Ordering::Relaxed)).await;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                    }
                }
            }
        })
        .await;
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
        deadline: Option<Instant>,
    ) -> Result<(), EngineError> {
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
        let grace = Instant::now() + SESSION_OPERATION_GRACE;
        let drained = timeout_at(
            deadline.map_or(grace, |end| end.min(grace)),
            self.wait_for_drain(),
        )
        .await;
        if drained.is_err() {
            cancellation.cancel();
            if timeout_at(
                deadline.unwrap_or_else(|| Instant::now() + SESSION_OPERATION_GRACE),
                self.wait_for_drain(),
            )
            .await
            .is_err()
            {
                tracing::warn!(
                    error_kind = "session_operation_drain_timeout",
                    "session operation did not stop after cancellation"
                );
                return Err(EngineError::new(
                    1106,
                    EngineErrorCategory::DeadlineExceeded,
                    true,
                ));
            }
        }
        Ok(())
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
            async move { gate.close_and_wait(None, None).await }
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

    // 忽略取消的工作仍持有资源时，关闭不能报告成功，且继续拒绝新工作。
    #[tokio::test(start_paused = true)]
    async fn closed_gate_stops_waiting_when_an_operation_ignores_cancellation() {
        let gate = Arc::new(SessionOperationGate::new_open());
        let _lease = gate.acquire().expect("new gate accepts an operation");

        let error = tokio::time::timeout(
            SESSION_OPERATION_GRACE.saturating_mul(2) + Duration::from_millis(1),
            gate.close_and_wait(None, None),
        )
        .await
        .expect("gate close must remain bounded after cancellation")
        .expect_err("active operations prevent successful suspension");
        assert_eq!(error.category(), EngineErrorCategory::DeadlineExceeded);
        assert!(gate.acquire().is_err());
        drop(_lease);
        gate.close_and_wait(None, None).await.unwrap();
    }

    #[tokio::test]
    async fn transition_operation_excludes_itself_but_waits_for_other_operations() {
        let gate = Arc::new(SessionOperationGate::new_open());
        let transition = gate.acquire().expect("transition acquires ordinary lease");
        let other = gate.acquire().expect("concurrent operation acquires lease");
        let closing = tokio::spawn({
            let gate = Arc::clone(&gate);
            async move { gate.close_and_wait(Some(transition), None).await }
        });

        tokio::task::yield_now().await;
        assert!(gate.acquire().is_err());
        assert!(!closing.is_finished());

        drop(other);
        closing.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn shared_deadline_limits_both_drain_and_cancellation_waits() {
        let gate = SessionOperationGate::new_open();
        let lease = gate.acquire().unwrap();
        let started = Instant::now();
        let deadline = Some(started + Duration::from_millis(10));
        let error = gate.close_and_wait(None, deadline).await.unwrap_err();
        assert_eq!(error.category(), EngineErrorCategory::DeadlineExceeded);
        assert!(started.elapsed() < Duration::from_millis(100));
        assert!(gate.acquire().is_err());
        drop(lease);
        gate.close_and_wait(None, deadline).await.unwrap();
    }
}
