//! Application 顶层对象图与运行期所有权。
//!
//! Engine 只选择具体 adapter；本模块统一构造稳定 facade，并持有 Search、
//! Clipboard 与历史维护的启动、关闭顺序。

use std::sync::Arc;
use std::time::Duration;

use uc_core::clipboard::ClipboardIntegrationMode;
use uc_core::file_transfer::OutboundProgressReporterPort;
use uc_core::ports::blob::{BlobReferenceRepositoryPort, BlobTransferPort};
use uc_core::ports::{
    ActiveClipboardDispatchPort, ActiveClipboardPullClientPort, ActiveClipboardPullServePort,
    ActiveClipboardReceiverPort, CleanupDirectoryStagingPort, ClipboardDispatchPort,
    ClipboardReceiverPort, ConnectionChannelPort, LocalIdentityPort,
    PairingInvitationAddressQueryPort, PairingInvitationByAddressPort, PairingInvitationPort,
    PeerAddressRepositoryPort, PeerReachabilityPort,
};

use crate::clipboard::active::ActiveClipboardLifecycleError;
use crate::clipboard::assembly::{
    ActiveClipboardSession, ActiveClipboardSessionDeps, ActiveClipboardStartError,
    ClipboardAssembly, ClipboardInboundAdapters, ClipboardSession, ClipboardSessionDeps,
};
use crate::clipboard::inbound::{ClipboardInboundEventPort, InboundClipboardApplyPort};
use crate::clipboard::local::{
    LocalClipboardOutcome, LocalClipboardProcessError, LocalClipboardRequest,
};
use crate::deps::{ApplicationDeps, CurrentSpaceMemberScopePort};
use crate::device::query_local_device::QueryLocalDeviceUseCase;
use crate::facade::app_facade::{AppFacade, AppFacadeParts};
use crate::facade::blob_transfer::BlobTransferFacade;
use crate::facade::clipboard::facade::ClipboardSyncDeps;
use crate::facade::clipboard::ClipboardSyncFacade;
use crate::facade::clipboard_history::{HistoryMaintenanceRuntime, HistoryMaintenanceRuntimeError};
use crate::facade::clipboard_write::RestoreBroadcastTrigger;
use crate::search::{SearchAssembly, SearchShutdownError};
use crate::settings::SettingsAssembly;
use crate::space::SpaceFacade;
use crate::space::{
    SpaceAdmissionDeps, SpaceFacadeDeps, SpaceRuntimeAdapters, SpaceSessionDeps,
    SpaceTransitionDeps,
};
use crate::transfer::blob::facade::BlobTransferDeps;
use crate::transfer::file::assembly::FileTransferAssembly;
use crate::transfer::file::assembly::{FileTransferAssemblyDeps, ReceiveCancellationDeps};

/// Engine 在 Iroh builder 上选择完成的 Space adapter。
pub struct ApplicationSpaceAdapters {
    pub current_engine_version: String,
    pub admission_credentials: Arc<dyn crate::deps::PrepareSpaceAdmissionCredentialsPort>,
    pub local_identity: Arc<dyn LocalIdentityPort>,
    pub pairing_invitation: Arc<dyn PairingInvitationPort>,
    pub pairing_invitation_addresses: Arc<dyn PairingInvitationAddressQueryPort>,
    pub pairing_invitation_by_address: Arc<dyn PairingInvitationByAddressPort>,
    pub presence: Arc<dyn PeerReachabilityPort>,
    pub analytics: Arc<dyn uc_observability_contract::analytics::AnalyticsFacade>,
    pub connection_channel: Option<Arc<dyn ConnectionChannelPort>>,
    pub device_management_reset_data: Arc<dyn crate::deps::DeviceManagementResetDataPort>,
    pub relationship_reset: Arc<dyn uc_core::membership::RelationshipStateResetPort>,
    pub space_security_reset: Arc<dyn uc_core::membership::SpaceSecurityStateResetPort>,
    pub runtime: SpaceRuntimeAdapters,
    pub peer_reachability_changed_events:
        tokio::sync::broadcast::Receiver<uc_core::ports::PeerReachabilityChanged>,
}

/// Engine 在共享 Iroh node 上选择完成的 Clipboard adapter。
pub struct ApplicationClipboardAdapters {
    pub peer_addresses: Arc<dyn PeerAddressRepositoryPort>,
    pub peer_reachability: Arc<dyn PeerReachabilityPort>,
    pub clipboard_dispatch: Arc<dyn ClipboardDispatchPort>,
    pub clipboard_receiver: Arc<dyn ClipboardReceiverPort>,
    pub local_identity: Arc<dyn LocalIdentityPort>,
    pub mobile_device_repo: Arc<dyn uc_core::ports::FindMobileDeviceByIdPort>,
    pub active_receiver: Arc<dyn ActiveClipboardReceiverPort>,
    pub active_dispatch: Arc<dyn ActiveClipboardDispatchPort>,
    pub active_pull_publisher: Arc<dyn uc_core::ports::atomic_publish::AtomicPublishPort>,
    pub active_pull_target_reserver:
        Arc<dyn uc_core::ports::inbound_file_target::ReserveInboundFileTargetPort>,
    pub active_pull_hidden_marker: Arc<dyn uc_core::ports::hidden_path::MarkHiddenPort>,
    pub staging_cleanup: Arc<dyn CleanupDirectoryStagingPort>,
}

/// Engine 一次提交给 Application 的网络 adapter 集合。
pub struct ApplicationNetworkAdapters {
    pub blob_transfer: Arc<dyn BlobTransferPort>,
    pub blob_reference: Arc<dyn BlobReferenceRepositoryPort>,
    pub outbound_progress_reporter: Arc<dyn OutboundProgressReporterPort>,
    pub space: ApplicationSpaceAdapters,
    pub clipboard: ApplicationClipboardAdapters,
}

/// Engine 宿主事件桥所需的被动端口投影。
pub struct ApplicationHostAdapters {
    pub system_clipboard: Arc<dyn uc_core::ports::clipboard::SystemClipboardPort>,
    pub change_origin: Arc<dyn uc_core::ports::clipboard::SelfWriteLedgerPort>,
    pub clock: Arc<dyn uc_core::ports::ClockPort>,
}

/// Application 构造完成、等待 Iroh 注册领域 endpoint 的一次性绑定。
///
/// Engine 只能读取必须注册到 Router 的窄端口，不能取得领域 assembly、
/// use case 或生命周期 owner。注册完成后必须消费 `complete` 并交还运行期。
pub struct ApplicationNetworkBinding {
    space: Arc<SpaceFacade>,
    blob_transfer: Arc<BlobTransferFacade>,
    blob_transfer_port: Arc<dyn BlobTransferPort>,
    clipboard_sync: Arc<ClipboardSyncFacade>,
    clipboard_receiver: Arc<dyn ClipboardReceiverPort>,
    member_scope: Arc<dyn CurrentSpaceMemberScopePort>,
    peer_reachability: Arc<dyn PeerReachabilityPort>,
    peer_addresses: Arc<dyn PeerAddressRepositoryPort>,
    outbound_progress_reporter: Arc<dyn OutboundProgressReporterPort>,
    active_receiver: Arc<dyn ActiveClipboardReceiverPort>,
    active_dispatch: Arc<dyn ActiveClipboardDispatchPort>,
    active_pull_serve: Arc<dyn ActiveClipboardPullServePort>,
    active_pull_adapters: ClipboardInboundAdapters,
    is_unlocked: Arc<dyn crate::deps::IsSpaceUnlockedPort>,
}

/// Engine 注册完 Application endpoint 后提交的最终 adapters。
pub struct ApplicationAdapters {
    network: ApplicationNetworkBinding,
    active_pull_client: Arc<dyn ActiveClipboardPullClientPort>,
    network_recovery: Arc<crate::space::NetworkRecoveryFacade>,
    inbound_adapters: ClipboardInboundAdapters,
    inbound_events: Arc<dyn ClipboardInboundEventPort>,
}

impl ApplicationNetworkBinding {
    pub fn space_admission_endpoint(
        &self,
    ) -> Arc<dyn crate::deps::HandleAuthenticatedSpaceAdmissionMessagePort> {
        self.space.space_admission_endpoint()
    }

    pub fn membership_history_endpoint(
        &self,
    ) -> Arc<dyn uc_core::membership::MembershipHistoryExchangeEndpointPort> {
        self.space.membership_history_endpoint()
    }

    pub fn membership_branch_recovery_endpoint(
        &self,
    ) -> Arc<dyn crate::deps::IssueMembershipBranchRecoveryPort> {
        self.space.membership_branch_recovery_endpoint()
    }

    pub fn current_member_scope(&self) -> Arc<dyn CurrentSpaceMemberScopePort> {
        Arc::clone(&self.member_scope)
    }

    pub fn active_clipboard_pull_serve(&self) -> Arc<dyn ActiveClipboardPullServePort> {
        Arc::clone(&self.active_pull_serve)
    }

    pub fn complete(
        self,
        active_pull_client: Arc<dyn ActiveClipboardPullClientPort>,
        network_recovery: Arc<crate::space::NetworkRecoveryFacade>,
        inbound_publisher: Arc<dyn uc_core::ports::atomic_publish::AtomicPublishPort>,
        inbound_target_reserver: Arc<
            dyn uc_core::ports::inbound_file_target::ReserveInboundFileTargetPort,
        >,
        inbound_hidden_marker: Arc<dyn uc_core::ports::hidden_path::MarkHiddenPort>,
        inbound_events: Arc<dyn ClipboardInboundEventPort>,
    ) -> ApplicationAdapters {
        let inbound_adapters = ClipboardInboundAdapters {
            fetcher: Arc::clone(&self.blob_transfer) as Arc<_>,
            publisher: inbound_publisher,
            target_reserver: inbound_target_reserver,
            hidden_marker: inbound_hidden_marker,
        };
        ApplicationAdapters {
            network: self,
            active_pull_client,
            network_recovery,
            inbound_adapters,
            inbound_events,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ApplicationStartError {
    #[error("Space application runtime was unavailable")]
    SpaceRuntimeUnavailable,
    #[error("active clipboard startup failed")]
    ActiveClipboard {
        #[source]
        source: ActiveClipboardStartError,
        search_rollback: Option<SearchShutdownError>,
    },
    #[error("active clipboard restore source attachment failed")]
    ActiveClipboardRestore {
        #[source]
        source: ActiveClipboardLifecycleError,
        search_rollback: Option<SearchShutdownError>,
    },
    #[error("Space session activity was already bound")]
    SpaceActivityAlreadyBound {
        search_rollback: Option<SearchShutdownError>,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ApplicationUpgradeError {
    #[error("application upgrade detection failed")]
    Detect {
        #[source]
        source: crate::facade::DetectUpgradeError,
    },
    #[error("application upgrade acknowledgement failed")]
    Acknowledge {
        #[source]
        source: crate::facade::AcknowledgeUpgradeError,
    },
}

/// 所有 Application 领域的唯一顶层 factory。
#[derive(Clone)]
pub struct ApplicationAssembly {
    deps: ApplicationDeps,
    settings: SettingsAssembly,
    file_transfer: Arc<FileTransferAssembly>,
    clipboard: Arc<ClipboardAssembly>,
}

impl ApplicationAssembly {
    pub fn build(deps: ApplicationDeps) -> Self {
        let settings = SettingsAssembly::build(&deps, &deps.paths, deps.relay_diagnostic.clone());
        let file_transfer = Arc::new(FileTransferAssembly::build(FileTransferAssemblyDeps {
            event_store: Arc::clone(&deps.file_transfer_event_store),
            host_event_bus: Arc::clone(&deps.host_event_bus),
            file_transfer: deps.storage.file_transfer.clone(),
            directory_receive: deps.storage.directory_receive.clone(),
            clock: Arc::clone(&deps.system.clock),
            artifact_cleanup: Arc::clone(&deps.receive_artifact_cleanup),
            save_dir_resolver: Arc::clone(&deps.receive_save_dir),
            file_cache_dir: deps.paths.file_cache_dir.clone(),
        }));
        let clipboard = Arc::new(ClipboardAssembly::build(
            crate::clipboard::assembly::ClipboardAssemblyDeps {
                application: deps.clone(),
                file_cache_dir: deps.paths.file_cache_dir.clone(),
                file_transfer: Arc::clone(&file_transfer),
                host_event_bus: Arc::clone(&deps.host_event_bus),
                background: Arc::clone(&deps.clipboard_background),
            },
        ));
        Self {
            deps,
            settings,
            file_transfer,
            clipboard,
        }
    }

    pub fn host_event_bus(&self) -> Arc<crate::facade::HostEventBus> {
        Arc::clone(&self.deps.host_event_bus)
    }

    pub fn host_adapters(&self) -> ApplicationHostAdapters {
        ApplicationHostAdapters {
            system_clipboard: Arc::clone(&self.deps.clipboard.system_clipboard),
            change_origin: Arc::clone(&self.deps.clipboard.clipboard_change_origin),
            clock: Arc::clone(&self.deps.system.clock),
        }
    }

    pub async fn prepare_network(
        &self,
    ) -> Result<crate::settings::PreparedNetworkSettings, crate::facade::SettingsFacadeError> {
        self.settings.prepare_network().await
    }

    pub async fn ensure_current_version(
        &self,
        current_version: &str,
    ) -> Result<(), ApplicationUpgradeError> {
        let upgrade = self.settings.upgrade();
        let status = upgrade
            .detect_on_startup(current_version)
            .await
            .map_err(|source| ApplicationUpgradeError::Detect { source })?;
        if matches!(status, crate::facade::UpgradeStatus::FreshInstall) {
            upgrade
                .acknowledge(current_version)
                .await
                .map_err(|source| ApplicationUpgradeError::Acknowledge { source })?;
        }
        Ok(())
    }

    pub async fn start_process_runtime(
        &self,
        task_registry: Arc<uc_core::TaskRegistry>,
    ) -> Result<(), crate::deps::ClipboardBackgroundStartError> {
        self.clipboard.start_background(task_registry).await
    }

    pub async fn cancel_active_file_transfers(
        &self,
        reason: uc_core::FileTransferCancellationReason,
    ) -> Result<(), crate::facade::FileTransferApplicationError> {
        self.file_transfer
            .facade()
            .cancel_active_sessions(reason)
            .await
    }

    pub async fn close_file_transfers(
        &self,
    ) -> Result<(), crate::facade::FileTransferApplicationError> {
        self.file_transfer.facade().close().await
    }

    /// 从 Engine 已选择的网络 adapters 一次构造 Space、Blob 与 Clipboard 对象图。
    pub fn assemble_network(
        &self,
        adapters: ApplicationNetworkAdapters,
    ) -> ApplicationNetworkBinding {
        let ApplicationNetworkAdapters {
            blob_transfer,
            blob_reference,
            outbound_progress_reporter,
            space,
            clipboard,
        } = adapters;
        let blob = Arc::new(BlobTransferFacade::new(BlobTransferDeps {
            hash: Arc::clone(&self.deps.system.hash),
            blob_transfer: Arc::clone(&blob_transfer),
            blob_reference,
            transfer_cipher: Arc::clone(&self.deps.security.transfer_cipher),
            host_event_emitter: Some(Arc::clone(&self.deps.host_event_bus)),
            outbound_progress_reporter: Some(Arc::clone(&outbound_progress_reporter)),
            file_transfer: Some(self.file_transfer.facade()),
        }));
        let ApplicationSpaceAdapters {
            current_engine_version,
            admission_credentials,
            local_identity,
            pairing_invitation,
            pairing_invitation_addresses,
            pairing_invitation_by_address,
            presence,
            analytics,
            connection_channel,
            device_management_reset_data,
            relationship_reset,
            space_security_reset,
            runtime,
            peer_reachability_changed_events,
        } = space;
        let space = Arc::new(SpaceFacade::new_dormant(SpaceFacadeDeps {
            application: self.deps.clone(),
            session: SpaceSessionDeps {
                space_access: self.deps.security.space_access_ports.clone(),
                mobile_consumable_backfill: Arc::clone(
                    &self.deps.clipboard.mobile_consumable_backfill,
                ),
                engine_version_state: Arc::clone(&self.deps.engine_version_state),
                current_engine_version,
                current_space_identity: Arc::clone(&self.deps.current_space_identity),
                initial_space_activation: Arc::clone(&self.deps.initial_space_activation),
                admission_credentials,
            },
            admission: SpaceAdmissionDeps {
                local_identity: Arc::clone(&local_identity),
                device_identity: Arc::clone(&self.deps.device.device_identity),
                member_repo: Arc::clone(&self.deps.device.member_repo),
                settings: Arc::clone(&self.deps.settings),
                clock: Arc::clone(&self.deps.system.clock),
                pairing_invitation,
                pairing_invitation_addresses,
                pairing_invitation_by_address,
                presence: Arc::clone(&presence),
                analytics,
                connection_channel,
            },
            transition: SpaceTransitionDeps {
                device_management_reset_data,
                relationship_reset,
                space_security_reset,
                space_rebuild_progress: Arc::clone(&self.deps.space_rebuild_progress),
                re_pairing_state_store: Arc::clone(&self.deps.re_pairing_state_store),
            },
            runtime_adapters: runtime,
            peer_reachability_changed_events,
        }));
        let member_scope = space.current_member_scope();
        let ApplicationClipboardAdapters {
            peer_addresses,
            peer_reachability,
            clipboard_dispatch,
            clipboard_receiver,
            local_identity,
            mobile_device_repo,
            active_receiver,
            active_dispatch,
            active_pull_publisher,
            active_pull_target_reserver,
            active_pull_hidden_marker,
            staging_cleanup,
        } = clipboard;
        let clipboard_sync = Arc::new(self.file_transfer.with_receive_cancellation(
            ClipboardSyncFacade::new(ClipboardSyncDeps {
                peer_addr_repo: Arc::clone(&peer_addresses),
                member_repo: Arc::clone(&self.deps.device.member_repo),
                peer_scope: Arc::clone(&member_scope),
                peer_reachability: Arc::clone(&peer_reachability),
                transfer_cipher: Arc::clone(&self.deps.security.transfer_cipher),
                clipboard_dispatch,
                device_identity: Arc::clone(&self.deps.device.device_identity),
                local_identity,
                settings: Arc::clone(&self.deps.settings),
                clock: Arc::clone(&self.deps.system.clock),
                analytics: Arc::clone(&self.deps.analytics),
                first_sync_state: Arc::clone(&self.deps.first_sync_state),
                entry_delivery_repo: Arc::clone(&self.deps.entry_delivery_repo),
                entry_repo: Arc::clone(&self.deps.clipboard.entry_ports.get),
                event_repo: Arc::clone(&self.deps.clipboard.clipboard_event_reader_repo),
                trusted_peer_repo: Arc::clone(&self.deps.trusted_peer_repo),
                mobile_device_repo,
                host_event_bus: Arc::clone(&self.deps.host_event_bus),
            }),
            ReceiveCancellationDeps {
                staging_cleanup,
                blob_transfer: Arc::clone(&blob),
            },
        ));
        let active_pull_serve = self.clipboard.active_pull_serve(Arc::clone(&blob));

        ApplicationNetworkBinding {
            space,
            blob_transfer: Arc::clone(&blob),
            blob_transfer_port: blob_transfer,
            clipboard_sync,
            clipboard_receiver,
            member_scope,
            peer_reachability,
            peer_addresses,
            outbound_progress_reporter,
            active_receiver,
            active_dispatch,
            active_pull_serve,
            active_pull_adapters: ClipboardInboundAdapters {
                fetcher: Arc::clone(&blob) as Arc<_>,
                publisher: active_pull_publisher,
                target_reserver: active_pull_target_reserver,
                hidden_marker: active_pull_hidden_marker,
            },
            is_unlocked: Arc::clone(&self.deps.security.space_access_ports.is_unlocked),
        }
    }

    async fn start_runtime(
        &self,
        adapters: ApplicationAdapters,
    ) -> Result<ApplicationRuntime, ApplicationStartError> {
        let ApplicationAdapters {
            network,
            active_pull_client,
            network_recovery,
            inbound_adapters,
            inbound_events,
        } = adapters;
        let ApplicationNetworkBinding {
            space,
            blob_transfer,
            blob_transfer_port,
            clipboard_sync,
            clipboard_receiver,
            member_scope,
            peer_reachability,
            peer_addresses,
            outbound_progress_reporter,
            active_receiver,
            active_dispatch,
            active_pull_serve: _,
            active_pull_adapters,
            is_unlocked,
        } = network;
        if !space.start_application_runtime().await {
            return Err(ApplicationStartError::SpaceRuntimeUnavailable);
        }
        let search = SearchAssembly::start(&self.deps);
        let active_clipboard = match self
            .clipboard
            .start_active(ActiveClipboardSessionDeps {
                receiver: active_receiver,
                dispatch: active_dispatch,
                is_unlocked,
                peer_addresses: Arc::clone(&peer_addresses),
                member_scope: Arc::clone(&member_scope),
                peer_reachability: Arc::clone(&peer_reachability),
                pull_client: active_pull_client,
                pull_adapters: active_pull_adapters,
            })
            .await
        {
            Ok(runtime) => runtime,
            Err(source) => {
                let search_rollback = search.shutdown().await.err();
                space.on_shutdown().await;
                return Err(ApplicationStartError::ActiveClipboard {
                    source,
                    search_rollback,
                });
            }
        };
        let (restore_tx, restore_rx) = tokio::sync::mpsc::unbounded_channel();
        if let Err(source) = active_clipboard.attach_restore_broadcast(restore_rx) {
            active_clipboard.shutdown().await;
            let search_rollback = search.shutdown().await.err();
            space.on_shutdown().await;
            return Err(ApplicationStartError::ActiveClipboardRestore {
                source,
                search_rollback,
            });
        }
        if !space.bind_session_activity(
            search.facade(),
            self.file_transfer.facade()
                as Arc<dyn crate::transfer::receive::reconciliation::EnsureReceiveReadyPort>,
        ) {
            active_clipboard.shutdown().await;
            let search_rollback = search.shutdown().await.err();
            space.on_shutdown().await;
            return Err(ApplicationStartError::SpaceActivityAlreadyBound { search_rollback });
        }

        let clipboard = self.clipboard.start_session(ClipboardSessionDeps {
            clipboard_sync: Arc::clone(&clipboard_sync),
            blob_transfer: Arc::clone(&blob_transfer),
            receiver: clipboard_receiver,
            member_scope,
            presence: peer_reachability,
            known_peers: peer_addresses,
            deliveries: Arc::clone(&self.deps.entry_delivery_repo),
            trusted_peers: Arc::clone(&self.deps.trusted_peer_repo),
            outbound_progress_reporter,
            inbound_adapters,
            inbound_events,
        });
        let settings = self.settings.clone().into_parts();
        let clipboard_restore = self.clipboard.restore(
            ClipboardIntegrationMode::Full,
            Some(RestoreBroadcastTrigger::new(restore_tx)),
        );
        let facade = Arc::new(AppFacade::new(AppFacadeParts {
            space: Arc::clone(&space),
            probe_profile_key_access: Arc::new(
                crate::profile::probe_profile_key_access::ProbeProfileKeyAccessUseCase::new(
                    Arc::clone(&self.deps.security.profile_key_access_probe),
                ),
            ),
            resource: self.clipboard.resource(),
            clipboard_history: self.clipboard.history(Some(blob_transfer_port)),
            clipboard_capture: self.clipboard.capture(),
            clipboard_sync,
            blob_transfer: Arc::clone(&blob_transfer),
            file_transfer: self.file_transfer.facade(),
            clipboard_outbound: clipboard.outbound(),
            clipboard_restore,
            active_clipboard: active_clipboard.facade(),
            search: search.facade(),
            settings: settings.settings,
            diagnostics: settings.diagnostics,
            query_local_device: Arc::new(QueryLocalDeviceUseCase::new(
                Arc::clone(&self.deps.device.device_identity),
                Arc::clone(&self.deps.settings),
            )),
            storage: settings.storage,
            config_migration: settings.config_migration,
            upgrade: settings.upgrade,
            network_recovery,
        }));
        let history_maintenance = facade.start_history_maintenance().await;
        let inbound_clipboard = clipboard.apply_inbound();
        let file_transfer_timeout =
            FileTransferTimeoutRuntime::start(self.file_transfer.facade(), blob_transfer);

        Ok(ApplicationRuntime {
            facade,
            inbound_clipboard,
            owners: tokio::sync::Mutex::new(Some(ApplicationRuntimeOwners {
                history_maintenance,
                file_transfer_timeout,
                search,
                space,
                clipboard,
                active_clipboard,
            })),
        })
    }
}

/// Application 领域运行期的唯一关闭 owner。
pub struct ApplicationRuntime {
    facade: Arc<AppFacade>,
    inbound_clipboard: Arc<dyn InboundClipboardApplyPort>,
    owners: tokio::sync::Mutex<Option<ApplicationRuntimeOwners>>,
}

struct ApplicationRuntimeOwners {
    history_maintenance: HistoryMaintenanceRuntime,
    file_transfer_timeout: FileTransferTimeoutRuntime,
    search: SearchAssembly,
    space: Arc<SpaceFacade>,
    clipboard: ClipboardSession,
    active_clipboard: ActiveClipboardSession,
}

struct FileTransferTimeoutRuntime {
    cancel: tokio::sync::watch::Sender<bool>,
    handle: tokio::task::JoinHandle<()>,
}

impl FileTransferTimeoutRuntime {
    fn start(
        file_transfer: Arc<crate::facade::FileTransferFacade>,
        blob_transfer: Arc<BlobTransferFacade>,
    ) -> Self {
        let (cancel, receiver) = tokio::sync::watch::channel(false);
        let handle = file_transfer.spawn_timeout_sweep(receiver, blob_transfer);
        Self { cancel, handle }
    }

    async fn shutdown(mut self) {
        let _ = self.cancel.send(true);
        if tokio::time::timeout(Duration::from_secs(1), &mut self.handle)
            .await
            .is_err()
        {
            self.handle.abort();
        }
    }
}

impl ApplicationRuntime {
    pub async fn start(
        assembly: &ApplicationAssembly,
        adapters: ApplicationAdapters,
    ) -> Result<Self, ApplicationStartError> {
        assembly.start_runtime(adapters).await
    }

    pub fn facade(&self) -> Arc<AppFacade> {
        Arc::clone(&self.facade)
    }

    pub async fn process_local_clipboard(
        &self,
        request: LocalClipboardRequest,
    ) -> Result<LocalClipboardOutcome, ApplicationRuntimeError> {
        let clipboard = {
            let owners = self.owners.lock().await;
            owners
                .as_ref()
                .map(|owners| owners.clipboard.local_processor())
        };
        let clipboard = clipboard.ok_or(ApplicationRuntimeError::Unavailable)?;
        clipboard
            .process(request)
            .await
            .map_err(|source| ApplicationRuntimeError::LocalClipboard { source })
    }

    pub fn inbound_clipboard(&self) -> Arc<dyn InboundClipboardApplyPort> {
        Arc::clone(&self.inbound_clipboard)
    }

    pub async fn shutdown(&self) -> ApplicationShutdownReport {
        let Some(owners) = self.owners.lock().await.take() else {
            return ApplicationShutdownReport {
                history: None,
                search: None,
            };
        };
        let history = owners.history_maintenance.shutdown().await.err();
        owners.file_transfer_timeout.shutdown().await;
        owners.clipboard.shutdown().await;
        owners.active_clipboard.shutdown().await;
        let search = owners.search.shutdown().await.err();
        owners.space.on_shutdown().await;
        ApplicationShutdownReport { history, search }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ApplicationRuntimeError {
    #[error("application runtime is unavailable")]
    Unavailable,
    #[error("local clipboard processing failed")]
    LocalClipboard {
        #[source]
        source: LocalClipboardProcessError,
    },
}

/// 关闭会尝试所有领域；报告保留每个下层的类型化失败。
pub struct ApplicationShutdownReport {
    pub history: Option<HistoryMaintenanceRuntimeError>,
    pub search: Option<SearchShutdownError>,
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use super::*;

    #[test]
    fn runtime_error_keeps_typed_source_and_redacts_public_text() {
        let error = ApplicationRuntimeError::LocalClipboard {
            source: LocalClipboardProcessError::Capture {
                source: crate::facade::ClipboardCaptureFacadeError::Internal(
                    "/private/clipboard.txt".to_owned(),
                ),
            },
        };

        assert!(error.source().is_some());
        assert_eq!(error.to_string(), "local clipboard processing failed");
        assert!(!error.to_string().contains("clipboard.txt"));
    }

    #[test]
    fn startup_error_keeps_primary_source_without_exposing_details() {
        let error = ApplicationStartError::ActiveClipboard {
            source: ActiveClipboardStartError::BackgroundNotReady,
            search_rollback: None,
        };

        assert!(error.source().is_some());
        assert_eq!(error.to_string(), "active clipboard startup failed");
    }
}
