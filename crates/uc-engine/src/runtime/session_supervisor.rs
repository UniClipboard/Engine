use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use tokio::sync::{Mutex, Notify};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use uc_application::facade::{
    AppFacade, ApplicationRuntime, ClipboardInboundEvent, ClipboardInboundEventAction,
    ClipboardInboundEventPort,
};
use uc_core::TaskRegistry;
use uc_infra::fs::{FsAtomicPublisher, FsHiddenPathMarker, FsInboundFileTarget};

use crate::assembly::deps::WiredDependencies;
#[cfg(feature = "lan-compat")]
use crate::assembly::facade::build_mobile_sync_facade;
use crate::assembly::lifecycle::build_daemon_lifecycle;
use crate::assembly::sync_engine::SyncEngineAssembly;
use crate::engine::event_stream::EventSender;
use crate::subsystems::peer_keepalive::spawn_peer_presence_event_task;
use crate::{
    EngineEvent, InboundNoticeActionSummary, InboundNoticeEvent, InboundRepresentationSummary,
};

use super::{operation_error_with_code, operation_unavailable_error, startup_error};
use crate::{EngineError, OperationResult};

const SESSION_OPERATION_GRACE: Duration = Duration::from_secs(2);

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
    recovery_generation: Arc<AtomicU64>,
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

impl SessionSupervisor {
    pub(super) fn new(application: uc_application::facade::ApplicationAssembly) -> Self {
        Self {
            session: Arc::new(Mutex::new(None)),
            factory: StdMutex::new(None),
            application,
            lifecycle: Mutex::new(()),
            operations: SessionOperationGate::new_open(),
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
            network_recovery,
            recovery_generation: Arc::new(AtomicU64::new(0)),
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
        let _lifecycle = self.lifecycle.lock().await;
        self.operations.close_and_wait(None).await?;
        self.stop_current_session(uc_core::FileTransferCancellationReason::ConnectivityRecovery)
            .await?;
        self.install_new_session(false).await
    }

    pub(super) async fn transition_pending_session(&self) -> Result<bool, EngineError> {
        let _lifecycle = self.lifecycle.lock().await;
        let facade = match self.session.lock().await.as_ref() {
            Some(session) => Arc::clone(&session.facade),
            None => return Ok(false),
        };
        tracing::debug!("运行时 Space transition 检查开始");
        if !facade
            .has_pending_space_transition()
            .await
            .map_err(|error| {
                operation_error_with_code(1103, "inspect runtime space transition", error)
            })?
        {
            return Ok(false);
        }
        tracing::info!("运行时发现待完成 Space transition");
        self.operations.close_and_wait(None).await?;
        tracing::info!("运行时 Space transition 已关闭新操作并等待在途操作");
        match facade.has_pending_space_transition().await {
            Ok(true) => {}
            Ok(false) => {
                tracing::info!("运行时 Space transition 二次确认已无待处理状态");
                self.operations.reopen();
                return Ok(false);
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
            .shutdown(uc_core::FileTransferCancellationReason::ConnectivityRecovery)
            .await;
        tracing::info!("运行时 Space transition 已关闭旧 session");
        let completed = facade.complete_pending_space_transition().await;
        match completed {
            Ok(_) => {
                tracing::info!("运行时 Space transition 持久步骤已完成");
                self.install_new_session(true).await?;
                tracing::info!("运行时 Space transition 新 session 已安装");
                Ok(true)
            }
            Err(error) => {
                tracing::warn!("运行时 Space transition 持久步骤失败，开始恢复 session");
                let original =
                    operation_error_with_code(1103, "complete runtime space transition", error);
                match self.install_new_session(false).await {
                    Ok(()) => Err(original),
                    Err(restore_error) => Err(restore_error),
                }
            }
        }
    }

    pub(super) async fn reset_space(
        &self,
        current_operation: SessionOperationLease,
    ) -> Result<OperationResult, EngineError> {
        let _lifecycle = self.lifecycle.lock().await;
        self.operations
            .close_and_wait(Some(current_operation))
            .await?;
        let session = self
            .session
            .lock()
            .await
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

    pub(super) async fn suspend(&self) -> Result<(), EngineError> {
        let _lifecycle = self.lifecycle.lock().await;
        self.operations.close_and_wait(None).await?;
        self.stop_current_session(uc_core::FileTransferCancellationReason::Unknown)
            .await
    }

    pub(super) async fn resume(&self) -> Result<(), EngineError> {
        let _lifecycle = self.lifecycle.lock().await;
        if self.session.lock().await.is_some() {
            return Ok(());
        }
        self.install_new_session(false).await
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
        let session = self.session.lock().await.take();
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

    async fn install_new_session(&self, resume_space_activities: bool) -> Result<(), EngineError> {
        let factory = self
            .factory
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .ok_or_else(super::operation_unavailable_error)?;
        let mut session = factory.build().await?;
        let mut resume_space_activities = resume_space_activities;
        if session
            .facade
            .has_pending_space_transition()
            .await
            .map_err(|error| {
                operation_error_with_code(1103, "inspect pending space transition", error)
            })?
        {
            let facade = Arc::clone(&session.facade);
            session
                .shutdown(uc_core::FileTransferCancellationReason::ConnectivityRecovery)
                .await;
            facade
                .complete_pending_space_transition()
                .await
                .map_err(|error| {
                    operation_error_with_code(1103, "recover pending space transition", error)
                })?;
            session = factory.build().await?;
            resume_space_activities = true;
        }
        let recovered = session.facade.recover_space_session().await;
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
                sync_engine
                    .shutdown(uc_core::FileTransferCancellationReason::Unknown)
                    .await;
                return Err(startup_error("application runtime", error));
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
        tasks
            .spawn("active_clipboard_events", move |cancel| async move {
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
        spawn_network_recovery_observation_task(
            sync_engine.subscribe_network_recovery_observations(),
            Arc::clone(&self.network_recovery),
            Arc::clone(&self.recovery_generation),
            &tasks,
        )
        .await;
        spawn_peer_presence_event_task(Arc::clone(&facade), &tasks, events).await;
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
        self.tasks.shutdown(Duration::from_millis(500)).await;
        info!("Engine session 网络观测任务已停止");
        let application_shutdown = self.application.shutdown().await;
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
        self.sync_engine.shutdown(transfer_reason).await;
        info!("Engine session Iroh 网络已停止");
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

async fn spawn_network_recovery_observation_task(
    mut observations: tokio::sync::broadcast::Receiver<
        uc_infra::network::iroh::NetworkRecoveryObservation,
    >,
    recovery: Arc<uc_application::facade::NetworkRecoveryFacade>,
    generation: Arc<AtomicU64>,
    tasks: &Arc<TaskRegistry>,
) {
    tasks
        .spawn("network_recovery_observations", move |cancel| async move {
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
        let drained = tokio::time::timeout(SESSION_OPERATION_GRACE, self.wait_for_drain()).await;
        if drained.is_err() {
            cancellation.cancel();
            if tokio::time::timeout(SESSION_OPERATION_GRACE, self.wait_for_drain())
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
    use super::*;

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
