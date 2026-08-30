mod dispatch;
mod host_clipboard;
pub(crate) mod host_file;
mod host_operations;
#[cfg(feature = "lan-compat")]
mod lan_compatibility;
#[cfg(feature = "lan-compat")]
mod mobile_upload;
mod session_supervisor;

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tracing::{error, warn};
use uc_application::deps::{ProfileFactoryResetCapabilityError, StopProfileRuntimePort};
use uc_application::facade::clipboard_write::LocalActiveRegisterAdvancer;
use uc_application::facade::{
    build_space_session_activity, AppFacade, HistoryMaintenanceRuntime, NetworkRecoveryEvent,
    ProfileFactoryResetOutcome, ProfileFactoryResetRequest, ProfileFactoryResetUseCase,
    SpaceSessionActivityDeps, SpaceSessionActivityPort,
};
use uc_core::ports::ClockPort;
use uc_core::TaskRegistry;

use crate::assembly::blob_tasks::{spawn_blob_processing_tasks, BlobProcessingPorts};
use crate::assembly::clipboard_runtime::{build_clipboard_runtime, ClipboardRuntime};
use crate::assembly::deps::WiredDependencies;
#[cfg(feature = "lan-compat")]
use crate::assembly::facade::build_mobile_sync_facade;
use crate::assembly::facade::{
    build_app_facade_from_deps, ClipboardRestoreAssembly, RuntimeAppFacadeAssembly,
};
use crate::assembly::host::{
    wire_host_capabilities_with_emitter, EngineHostEventEmitter, HostWiring,
};
use crate::assembly::lifecycle::build_daemon_lifecycle;
#[cfg(feature = "lan-compat")]
use crate::assembly::mobile_lan::MobileLanEndpointUpdater;
use crate::assembly::search::build_search_runtime;
use crate::assembly::sync_engine::SyncEngineAssembly;
use crate::engine::event_stream::EventSender;
use crate::subsystems::peer_keepalive::spawn_peer_presence_event_task;
use crate::{EngineConfig, EngineError, EngineErrorCategory, HostCapabilities, HostFileAccess};
use host_clipboard::{spawn_host_clipboard_change_task, HostClipboardChangeRuntime};
use session_supervisor::SessionSupervisor;
const START_FAILED_CODE: u32 = 1101;
const OPERATION_UNAVAILABLE_CODE: u32 = 1103;

pub(crate) struct ProductionRuntime {
    app_version: String,
    session_supervisor: Arc<SessionSupervisor>,
    profile_reset: Arc<ProfileFactoryResetUseCase>,
    network_recovery: Arc<uc_application::facade::NetworkRecoveryFacade>,
    task_registry: Arc<TaskRegistry>,
    #[cfg(feature = "lan-compat")]
    mobile_lan_endpoint: MobileLanEndpointUpdater,
    clock: Arc<dyn ClockPort>,
    file_cache_dir: PathBuf,
    temporary_dir: std::path::PathBuf,
    clipboard_import_root: std::path::PathBuf,
    files: Arc<dyn HostFileAccess>,
    clipboard_change_runtime: HostClipboardChangeRuntime,
    events: EventSender,
}

struct SessionFactory {
    wired: WiredDependencies,
    paths: uc_application::facade::AppPaths,
    app_version: String,
    events: EventSender,
    rendezvous_base_url: Option<String>,
    relay_fallback_override: Option<bool>,
    iroh_bind_port_override: Option<u16>,
    network_recovery: Arc<uc_application::facade::NetworkRecoveryFacade>,
    recovery_generation: Arc<AtomicU64>,
}

struct ProductionSession {
    facade: Arc<AppFacade>,
    history_maintenance: HistoryMaintenanceRuntime,
    search_runtime: uc_application::facade::SearchRuntime,
    #[cfg(feature = "lan-compat")]
    mobile_sync: Arc<uc_mobile_lan::MobileSyncFacade>,
    clipboard: ClipboardRuntime,
    sync_engine: SyncEngineAssembly,
    tasks: Arc<TaskRegistry>,
}

struct ProductionProfileRuntimeStopper {
    session_supervisor: Arc<SessionSupervisor>,
    tasks: Arc<TaskRegistry>,
}

#[async_trait::async_trait]
impl StopProfileRuntimePort for ProductionProfileRuntimeStopper {
    async fn stop_profile_runtime(&self) -> Result<(), ProfileFactoryResetCapabilityError> {
        self.session_supervisor
            .suspend()
            .await
            .map_err(|_| ProfileFactoryResetCapabilityError)?;
        self.session_supervisor.clear_factory();
        self.tasks.shutdown(Duration::from_millis(500)).await;
        Ok(())
    }
}

impl ProductionSession {
    async fn shutdown(self, transfer_reason: uc_core::FileTransferCancellationReason) {
        #[cfg(feature = "lan-compat")]
        if self
            .mobile_sync
            .shutdown_mobile_file_uploads()
            .await
            .is_err()
        {
            warn!("mobile file upload shutdown finished with an error");
        }
        if let Err(error) = self.history_maintenance.shutdown().await {
            warn!(error = %error, "history maintenance stopped with an error");
        }
        self.tasks.shutdown(Duration::from_millis(500)).await;
        if let Err(error) = self.search_runtime.shutdown().await {
            error!(error = %error, "search runtime stopped with error");
        }
        self.clipboard.shutdown().await;
        self.sync_engine.shutdown(transfer_reason).await;
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

fn re_pairing_scope_for_setup_state(
    state: &uc_application::facade::SetupStateView,
) -> Option<crate::RePairingScope> {
    state
        .re_pairing_required
        .then_some(crate::RePairingScope::AllDevices)
}

fn network_recovery_summary(event: NetworkRecoveryEvent) -> crate::NetworkRecoveryStatusSummary {
    match event {
        NetworkRecoveryEvent::Started => crate::NetworkRecoveryStatusSummary {
            phase: crate::NetworkRecoveryPhaseSummary::Recovering,
            retryable: false,
            next_retry_in_ms: None,
        },
        NetworkRecoveryEvent::RetryScheduled { delay } => crate::NetworkRecoveryStatusSummary {
            phase: crate::NetworkRecoveryPhaseSummary::RetryScheduled,
            retryable: true,
            next_retry_in_ms: Some(delay.as_millis().min(u128::from(u64::MAX)) as u64),
        },
        NetworkRecoveryEvent::Succeeded => crate::NetworkRecoveryStatusSummary {
            phase: crate::NetworkRecoveryPhaseSummary::Idle,
            retryable: false,
            next_retry_in_ms: None,
        },
        NetworkRecoveryEvent::Failed { retryable } => crate::NetworkRecoveryStatusSummary {
            phase: crate::NetworkRecoveryPhaseSummary::Failed,
            retryable,
            next_retry_in_ms: None,
        },
    }
}

async fn spawn_network_recovery_events(
    mut changes: tokio::sync::broadcast::Receiver<NetworkRecoveryEvent>,
    tasks: &Arc<TaskRegistry>,
    events: EventSender,
) {
    tasks
        .spawn("network_recovery_events", move |cancel| async move {
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    change = changes.recv() => match change {
                        Ok(change) => events.send(crate::EngineEvent::NetworkRecoveryChanged(network_recovery_summary(change))),
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => events.send(crate::EngineEvent::RefreshRequired {
                            reason: crate::RefreshReason::ConsumerLagged,
                        }),
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                    }
                }
            }
        })
        .await;
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

#[cfg(feature = "lan-compat")]
fn engine_event_for_mobile_settings_update(
    settings: &crate::MobileSyncSettingsUpdateSummary,
) -> crate::EngineEvent {
    crate::EngineEvent::MobileLanSettingsChanged(crate::MobileLanSettingsChanged {
        enabled: settings.enabled,
        lan_listen_enabled: settings.lan_listen_enabled,
        lan_port: settings.lan_port,
    })
}

impl ProductionRuntime {
    pub(crate) async fn start(
        config: EngineConfig,
        host: HostCapabilities,
        events: EventSender,
    ) -> Result<Self, EngineError> {
        let app_version = config.app_version().to_string();
        let rendezvous_base_url = config.rendezvous_base_url_override();
        let relay_fallback_override = config.test_relay_fallback_override();
        let iroh_bind_port_override = config.test_iroh_bind_port_override();
        let emitter = Arc::new(EngineHostEventEmitter::new(events.clone()));
        let HostWiring {
            wired,
            background,
            paths,
            temporary_dir,
            clipboard_import_root,
            files,
            clipboard_changes,
        } = wire_host_capabilities_with_emitter(&config, host, emitter)
            .map_err(|error| startup_error("dependency wiring", error))?;

        let session = Arc::new(Mutex::new(None));
        let session_supervisor = Arc::new(SessionSupervisor::new(
            Arc::clone(&session),
            Arc::clone(&wired.shared.file_transfer_facade),
        ));
        let task_registry = Arc::new(TaskRegistry::new());
        let profile_runtime: Arc<dyn StopProfileRuntimePort> =
            Arc::new(ProductionProfileRuntimeStopper {
                session_supervisor: Arc::clone(&session_supervisor),
                tasks: Arc::clone(&task_registry),
            });
        let profile_reset = Arc::new(ProfileFactoryResetUseCase::new(
            Arc::clone(&wired.profile_reset.lifecycle_repository),
            profile_runtime,
            Arc::clone(&wired.profile_reset.keys),
            Arc::clone(&wired.profile_reset.state),
        ));
        if profile_reset
            .execute(ProfileFactoryResetRequest::ResumeIfNeeded)
            .await
            .map_err(crate::operations::space::factory_reset::map_profile_factory_reset_error)?
            == ProfileFactoryResetOutcome::Completed
        {
            return Err(EngineError::new(
                crate::error_codes::FACTORY_RESET_UNAVAILABLE_CODE,
                EngineErrorCategory::Unavailable,
                true,
            ));
        }
        let recovery_port: Arc<dyn uc_application::facade::RebuildNetworkSessionPort> =
            Arc::clone(&session_supervisor)
                as Arc<dyn uc_application::facade::RebuildNetworkSessionPort>;
        let network_recovery = Arc::new(uc_application::facade::NetworkRecoveryFacade::new(
            recovery_port,
        ));
        let session_factory = Arc::new(SessionFactory {
            wired: wired.clone(),
            paths: paths.clone(),
            app_version: app_version.clone(),
            events: events.clone(),
            rendezvous_base_url: rendezvous_base_url.clone(),
            relay_fallback_override,
            iroh_bind_port_override,
            network_recovery: Arc::clone(&network_recovery),
            recovery_generation: Arc::new(AtomicU64::new(0)),
        });
        session_supervisor.configure_factory(Arc::clone(&session_factory));
        session_supervisor.resume().await?;
        spawn_space_transition_watcher(
            Arc::clone(&session_supervisor),
            &task_registry,
            events.clone(),
        )
        .await;
        spawn_network_recovery_events(network_recovery.subscribe(), &task_registry, events.clone())
            .await;
        let blob_ports = BlobProcessingPorts::from_app_deps(&wired.deps);
        spawn_blob_processing_tasks(background, blob_ports, &task_registry).await;
        let clipboard_change_runtime = HostClipboardChangeRuntime {
            session_supervisor: Arc::clone(&session_supervisor),
            system_clipboard: Arc::clone(&wired.deps.clipboard.system_clipboard),
            change_origin: Arc::clone(&wired.deps.clipboard.clipboard_change_origin),
            active_register: LocalActiveRegisterAdvancer::new(
                Arc::clone(&wired.deps.clipboard.active_register),
                Arc::clone(&wired.deps.device.device_identity),
                Arc::clone(&wired.deps.system.clock),
                wired.deps.clipboard.mobile_consumability.clone(),
            ),
            host_events: Arc::clone(&wired.shared.host_event_bus),
        };
        if let Some(changes) = clipboard_changes {
            spawn_host_clipboard_change_task(
                changes,
                clipboard_change_runtime.clone(),
                Arc::clone(&task_registry),
            )
            .await;
        }

        #[cfg(feature = "lan-compat")]
        let mobile_lan_endpoint = MobileLanEndpointUpdater::new(Arc::clone(
            &wired.daemon_runtime.mobile_sync_endpoint_info,
        ));
        let clock = Arc::clone(&wired.deps.system.clock);
        let file_cache_dir = paths.file_cache_dir.clone();
        Ok(Self {
            app_version,
            session_supervisor,
            profile_reset,
            network_recovery,
            task_registry,
            #[cfg(feature = "lan-compat")]
            mobile_lan_endpoint,
            clock,
            file_cache_dir,
            temporary_dir,
            clipboard_import_root,
            files,
            clipboard_change_runtime,
            events,
        })
    }

    async fn build_session(factory: &SessionFactory) -> Result<ProductionSession, EngineError> {
        let wired = &factory.wired;
        let paths = &factory.paths;
        let events = factory.events.clone();
        let lifecycle = build_daemon_lifecycle(
            &wired.deps,
            &wired.sync_engine,
            &wired.shared,
            &factory.app_version,
            #[cfg(feature = "lan-compat")]
            wired.mobile_sync_ports.clone(),
            factory.rendezvous_base_url.clone(),
            factory.relay_fallback_override,
            factory.iroh_bind_port_override,
        )
        .await
        .map_err(|error| startup_error("p2p session", error))?;
        let sync_engine = lifecycle.sync_engine_assembly;
        let (restore_tx, restore_rx) = tokio::sync::mpsc::unbounded_channel();
        sync_engine.attach_restore_broadcast(restore_rx);
        let search_runtime = build_search_runtime(&wired.deps);
        let session_activity = build_space_session_activity(
            search_runtime.facade(),
            SpaceSessionActivityDeps {
                receive: Arc::clone(&wired.shared.file_transfer_facade)
                    as Arc<dyn uc_application::facade::EnsureReceiveReadyPort>,
            },
        );
        if !sync_engine
            .bind_space_session_activity(session_activity as Arc<dyn SpaceSessionActivityPort>)
        {
            return Err(startup_error(
                "Space session activity",
                "Space session activity was already bound",
            ));
        }
        let clipboard = build_clipboard_runtime(wired, &sync_engine, events.clone());
        #[cfg(feature = "lan-compat")]
        let mobile_sync = build_mobile_sync_facade(
            &wired.deps,
            paths,
            wired.mobile_sync_ports.clone(),
            Arc::clone(&clipboard.apply_inbound),
            Some(Arc::clone(&wired.shared.file_transfer_facade)),
            None,
            Some(Arc::clone(&clipboard.outbound)),
            Some(Arc::clone(&sync_engine.active_clipboard)),
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
        let facade = build_app_facade_from_deps(
            &wired.deps,
            paths,
            RuntimeAppFacadeAssembly {
                space: Arc::clone(&sync_engine.facade),
                clipboard_sync: Arc::clone(&sync_engine.clipboard_sync),
                blob_transfer: Arc::clone(&sync_engine.blob),
                blob_transfer_port: Arc::clone(&sync_engine.blob_transfer),
                file_transfer: Arc::clone(&wired.shared.file_transfer_facade),
                clipboard_restore: ClipboardRestoreAssembly {
                    write_coordinator: Arc::clone(&wired.shared.clipboard_write_coordinator),
                    integration_mode: uc_core::clipboard::ClipboardIntegrationMode::Full,
                    restore_broadcast: Some(
                        uc_application::facade::clipboard_write::RestoreBroadcastTrigger::new(
                            restore_tx,
                        ),
                    ),
                },
                search: search_runtime.facade(),
                clipboard_outbound: Arc::clone(&clipboard.outbound),
                network_recovery: Arc::clone(&factory.network_recovery),
            },
        );
        spawn_network_recovery_observation_task(
            sync_engine.subscribe_network_recovery_observations(),
            Arc::clone(&factory.network_recovery),
            Arc::clone(&factory.recovery_generation),
            &tasks,
        )
        .await;
        let history_maintenance = facade.start_history_maintenance().await;
        spawn_peer_presence_event_task(Arc::clone(&facade), &tasks, events.clone()).await;
        let blob_transfer = Arc::clone(&sync_engine.blob);
        let file_transfer_facade = Arc::clone(&wired.shared.file_transfer_facade);
        tasks
            .spawn("file_transfer_timeout_sweep", move |cancel| async move {
                let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
                let mut handle = file_transfer_facade.spawn_timeout_sweep(cancel_rx, blob_transfer);
                cancel.cancelled().await;
                let _ = cancel_tx.send(true);
                if tokio::time::timeout(Duration::from_secs(1), &mut handle)
                    .await
                    .is_err()
                {
                    handle.abort();
                }
            })
            .await;

        Ok(ProductionSession {
            facade,
            history_maintenance,
            search_runtime,
            #[cfg(feature = "lan-compat")]
            mobile_sync,
            clipboard,
            sync_engine,
            tasks,
        })
    }

    async fn current_session_field<T: ?Sized>(
        &self,
        project: impl FnOnce(&ProductionSession) -> Arc<T>,
    ) -> Result<Arc<T>, EngineError> {
        let session = self.session_supervisor.session();
        let result = session
            .lock()
            .await
            .as_ref()
            .map(project)
            .ok_or_else(operation_unavailable_error);
        result
    }

    async fn current_facade(&self) -> Result<Arc<AppFacade>, EngineError> {
        self.current_session_field(|session| Arc::clone(&session.facade))
            .await
    }

    async fn current_active_clipboard(
        &self,
    ) -> Result<Arc<uc_application::facade::ActiveClipboardFacade>, EngineError> {
        self.current_session_field(|session| Arc::clone(&session.sync_engine.active_clipboard))
            .await
    }

    async fn current_clipboard_sync_runtime(
        &self,
    ) -> Result<Arc<uc_application::facade::ClipboardSyncRuntime>, EngineError> {
        self.current_session_field(|session| Arc::clone(&session.clipboard.sync))
            .await
    }

    #[cfg(feature = "lan-compat")]
    async fn current_mobile_sync(
        &self,
    ) -> Result<Arc<uc_mobile_lan::MobileSyncFacade>, EngineError> {
        self.current_session_field(|session| Arc::clone(&session.mobile_sync))
            .await
    }
}

async fn spawn_space_transition_watcher(
    supervisor: Arc<SessionSupervisor>,
    tasks: &Arc<TaskRegistry>,
    events: EventSender,
) {
    tasks
        .spawn("space_transition_watcher", move |cancel| async move {
            let mut interval = tokio::time::interval(Duration::from_millis(100));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    _ = interval.tick() => match supervisor.transition_pending_session().await {
                        Ok(true) => {
                            events.send(crate::EngineEvent::RefreshRequired {
                                reason: crate::RefreshReason::StateInvalidated,
                            });
                        }
                        Ok(false) => {}
                        Err(error) => warn!(
                            error_code = error.code(),
                            retryable = error.is_retryable(),
                            "runtime Space transition attempt failed"
                        ),
                    }
                }
            }
        })
        .await;
}

fn startup_error(context: &'static str, error: impl std::fmt::Display) -> EngineError {
    let _ = writeln!(
        std::io::stderr().lock(),
        "uc-engine startup failed [{context}]: {error}"
    );
    error!(context, error = %error, "engine startup failed");
    EngineError::new(START_FAILED_CODE, EngineErrorCategory::Unavailable, true)
}

fn operation_unavailable_error() -> EngineError {
    EngineError::new(
        OPERATION_UNAVAILABLE_CODE,
        EngineErrorCategory::Unavailable,
        false,
    )
}

fn operation_error_with_code(
    code: u32,
    context: &'static str,
    error: impl std::fmt::Display,
) -> EngineError {
    error!(context, error = %error, "engine operation failed");
    EngineError::new(code, EngineErrorCategory::Internal, false)
}

#[cfg(test)]
mod tests {
    use uc_application::facade::{
        ClipboardOutboundOutcome, SearchFacadeError, SearchPageView, SearchResultView,
        StorageFacadeError, StorageStatsView,
    };
    use uc_core::ids::DeviceId;

    use super::*;
    use crate::error_codes::{CLEAR_STORAGE_CACHE_FAILED_CODE, QUERY_STORAGE_STATS_FAILED_CODE};
    use crate::operations::history::search::{
        history_page_result, history_search_input, map_query_history_error,
    };
    use crate::operations::settings::storage::{map_storage_error, storage_stats_result};
    use crate::runtime::host_operations::send_report_result;
    use crate::{EntrySummary, OperationResult, QueryHistoryInput, StorageStatsSummary};

    #[test]
    fn active_clipboard_event_preserves_mobile_sse_identity() {
        let state = uc_core::clipboard::ActiveClipboardState::new(
            "hash-1",
            uc_core::ids::EntryId::from("entry-1"),
            42,
            DeviceId::new("device-1"),
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

    #[test]
    fn network_recovery_events_expose_only_stable_status() {
        assert_eq!(
            network_recovery_summary(NetworkRecoveryEvent::RetryScheduled {
                delay: Duration::from_millis(500)
            }),
            crate::NetworkRecoveryStatusSummary {
                phase: crate::NetworkRecoveryPhaseSummary::RetryScheduled,
                retryable: true,
                next_retry_in_ms: Some(500),
            }
        );
        assert_eq!(
            network_recovery_summary(NetworkRecoveryEvent::Failed { retryable: false }),
            crate::NetworkRecoveryStatusSummary {
                phase: crate::NetworkRecoveryPhaseSummary::Failed,
                retryable: false,
                next_retry_in_ms: None,
            }
        );
    }

    #[test]
    fn re_pairing_setup_state_requests_an_all_devices_product_event() {
        let state = uc_application::facade::SetupStateView {
            has_completed: true,
            space_id: None,
            current_invitation: None,
            device_name: None,
            re_pairing_required: true,
        };

        assert_eq!(
            re_pairing_scope_for_setup_state(&state),
            Some(crate::RePairingScope::AllDevices)
        );
    }

    #[cfg(feature = "lan-compat")]
    #[test]
    fn mobile_settings_event_preserves_listener_target() {
        let settings = crate::MobileSyncSettingsUpdateSummary {
            enabled: true,
            lan_listen_enabled: true,
            lan_advertise_ip: None,
            lan_advertise_base_url: None,
            lan_port: Some(51234),
            changed: true,
        };

        assert_eq!(
            engine_event_for_mobile_settings_update(&settings),
            crate::EngineEvent::MobileLanSettingsChanged(crate::MobileLanSettingsChanged {
                enabled: true,
                lan_listen_enabled: true,
                lan_port: Some(51234),
            })
        );
    }

    #[test]
    fn history_search_input_parses_only_versioned_bounded_cursors() {
        let parsed = history_search_input(QueryHistoryInput {
            cursor: Some("uc-history-v1:40".into()),
            limit: 20,
            query: Some("needle".into()),
        })
        .unwrap();
        assert_eq!(parsed.offset, 40);
        assert_eq!(parsed.limit, 20);
        assert_eq!(parsed.query, "needle");

        for input in [
            QueryHistoryInput {
                cursor: Some("40".into()),
                limit: 20,
                query: None,
            },
            QueryHistoryInput {
                cursor: Some("uc-history-v2:40".into()),
                limit: 20,
                query: None,
            },
            QueryHistoryInput {
                cursor: None,
                limit: 0,
                query: None,
            },
            QueryHistoryInput {
                cursor: None,
                limit: 201,
                query: None,
            },
        ] {
            let error = history_search_input(input).unwrap_err();
            assert_eq!(error.category(), EngineErrorCategory::InvalidInput);
        }
    }

    #[test]
    fn history_page_result_projects_entries_and_advances_cursor() {
        let result = history_page_result(
            SearchPageView {
                total: 61,
                has_more: true,
                items: vec![SearchResultView {
                    entry_id: "entry-1".into(),
                    content_type: "text".into(),
                    active_time_ms: 123,
                    tags: Vec::new(),
                    text_preview: Some("private preview".into()),
                    char_count: Some(15),
                    mime_type: "text/plain".into(),
                    file_extensions: Vec::new(),
                    file_names: Vec::new(),
                    file_paths: Vec::new(),
                    link_urls: Vec::new(),
                    source_device: None,
                    payload_state: None,
                }],
                state: "ready".into(),
            },
            40,
            20,
        )
        .unwrap();

        assert_eq!(
            result,
            OperationResult::HistoryPage {
                entries: vec![EntrySummary {
                    entry_id: "entry-1".into(),
                    content_type: "text".into(),
                    preview: Some("private preview".into()),
                    created_at_ms: 123,
                }],
                next_cursor: Some("uc-history-v1:60".into()),
            }
        );
    }

    #[test]
    fn history_error_mapping_preserves_retry_semantics() {
        let locked = map_query_history_error(SearchFacadeError::SessionLocked);
        assert_eq!(locked.category(), EngineErrorCategory::Unauthorized);
        assert!(!locked.is_retryable());

        let rebuilding = map_query_history_error(SearchFacadeError::IndexRebuilding);
        assert_eq!(rebuilding.category(), EngineErrorCategory::Unavailable);
        assert!(rebuilding.is_retryable());
    }

    #[test]
    fn send_result_preserves_every_dispatch_field() {
        let result = send_report_result(
            "entry-1".into(),
            ClipboardOutboundOutcome::Dispatched {
                snapshot_hash: "hash-1".into(),
                per_target: vec![uc_application::facade::DispatchEntryPerTarget {
                    device_id: DeviceId::new("device-1"),
                    outcome: Err("private failure detail".into()),
                }],
                accepted: 1,
                duplicate: 2,
                offline: 3,
                errored: 4,
                pending: 5,
                pending_targets: Vec::new(),
                at_ms: 123,
                blob_ref_count: 6,
            },
        )
        .unwrap();

        let OperationResult::EntrySent(report) = result else {
            panic!("expected entry-sent result");
        };
        assert_eq!(report.entry_id, "entry-1");
        assert_eq!(report.snapshot_hash, "hash-1");
        assert_eq!(report.at_ms, 123);
        assert_eq!(report.total_accepted, 1);
        assert_eq!(report.total_duplicate, 2);
        assert_eq!(report.total_offline, 3);
        assert_eq!(report.total_errored, 4);
        assert_eq!(report.total_pending, 5);
        assert_eq!(report.per_target.len(), 1);
        assert!(!format!("{report:?}").contains("private failure detail"));
    }

    #[test]
    fn storage_stats_projection_does_not_expose_the_host_data_path() {
        let result = storage_stats_result(StorageStatsView {
            total_bytes: 50,
            database_bytes: 10,
            vault_bytes: 20,
            cache_bytes: 15,
            logs_bytes: 5,
            data_dir: "/private/user/path".into(),
        });

        assert_eq!(
            result,
            OperationResult::StorageStats(StorageStatsSummary {
                total_bytes: 50,
                database_bytes: 10,
                vault_bytes: 20,
                cache_bytes: 15,
                logs_bytes: 5,
            })
        );
        assert!(!format!("{result:?}").contains("/private/user/path"));
    }

    #[test]
    fn storage_failures_use_distinct_stable_codes() {
        let stats = map_storage_error(StorageFacadeError::Stats("private detail".into()));
        let clear = map_storage_error(StorageFacadeError::ClearCache("private detail".into()));

        assert_eq!(stats.code(), QUERY_STORAGE_STATS_FAILED_CODE);
        assert_eq!(clear.code(), CLEAR_STORAGE_CACHE_FAILED_CODE);
        assert_eq!(stats.category(), EngineErrorCategory::Internal);
        assert_eq!(clear.category(), EngineErrorCategory::Internal);
    }
}
