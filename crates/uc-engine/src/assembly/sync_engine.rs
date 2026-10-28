//! Engine 网络组合根。
//!
//! 本模块选择并安装 Iroh、Infra 与观测 adapter，再通过一次性
//! [`ApplicationNetworkBinding`] 取得 Router 必需的窄 endpoint。Space、
//! Clipboard、Blob 与文件传输对象图及其关闭顺序均由 Application 持有；
//! 进度、剪贴板与文件业务运行期统一交给Application持有。

use super::observability::{self, observe_peer_reachability};
#[cfg(all(test, feature = "dev-tools"))]
use crate::testing::active_pull_publication::{decorate_blob, decorate_preparation};
#[cfg(all(test, feature = "dev-tools", target_os = "linux"))]
use crate::testing::directory_probe;
#[cfg(all(test, feature = "dev-tools"))]
use crate::testing::receive_exports;
#[cfg(all(test, feature = "dev-tools", target_os = "linux"))]
use crate::testing::receive_streams;

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;

use tracing::instrument;

use crate::assembly::deps::SyncEngineDeps;
#[cfg(feature = "dev-tools")]
use crate::dev::{
    ControlledSpaceAdmissionTransport, FaultableGroupUpdateReceiver, FaultableGroupUpdateWire,
    GatedJoinerActivation, GatedMembershipHistoryEndpoint, JoinerFinalConfirmationGate,
    RecordedGroupUpdateDispatch, RecordedMembershipHistoryExchange,
};
use uc_application::deps::{
    ApplicationClipboardAdapters, ApplicationNetworkAdapters, ApplicationNetworkBinding,
    ApplicationSpaceAdapters, ApplyGroupUpdatePort, BlobDownloadResources, BlobTransport,
    BranchRecoveryReceiver, BranchRecoveryTransport, BranchRecoveryWirePort, ClipboardReceiverPort,
    ConnectivityManager, ConnectivityPolicy, GroupUpdateEndpoint, GroupUpdateLocalWorkPort,
    GroupUpdateReceiverPort, GroupUpdateWirePort, HistoryWirePort, IdleRetentionPolicy,
    MembershipHistoryReceiver, MembershipHistoryTransport, PeerIdentityDirectoryPort,
    PeerReachabilityTransport, PeerReachabilityTransportPorts, ProgressWirePort,
    SpaceAdmissionAdapters, SpaceMembershipAdapters, SpaceRuntimeAdapters, TransferProgressNetwork,
};
use uc_application::facade::ApplicationAssembly;
use uc_core::membership::{GroupUpdateDispatchPort, PeerAdmissionPort};
use uc_core::ports::{
    ActiveClipboardDispatchPort, ActiveClipboardReceiverPort, ClipboardDispatchPort,
    ConnectionChannelPort, LocalIdentityPort,
};
use uc_infra_local::fs::{
    FsAtomicPublisher, FsDirectoryStagingCleaner, FsHiddenPathMarker, FsInboundFileTarget,
};
use uc_infra_local::SystemElapsedClock;
use uc_infra_p2p::network::iroh::transfer_progress_v2::{
    TransferProgressInboundAdapter, TransferProgressWire,
};
use uc_infra_p2p::network::iroh::{
    blob_exchange::IrohBlobSourceResolver,
    connectivity::{
        BranchRecoveryLocalWork, BranchRecoveryWire, ConnectivityProfile, PeerReachabilityWire,
    },
    group_update_adapter::{GroupUpdateWire, IrohGroupUpdateAdapter, SessionGroupUpdateLocalWork},
    ActiveClipboardHandlers, ActiveClipboardPullHandlers, BlobHandlers, ClipboardHandlers,
    IrohIdentityStore, IrohNodeError, IrohSessionBuilder, MembershipHistoryWire,
    PreparedIrohSession, SessionMembershipHistoryLocalWork,
};
use uc_infra_profile::security::Sha256IdentityFingerprintFactory;
use uc_infra_profile::space::{
    DefaultJoinerActivationExecutor, DefaultJoinerActivationPreparation,
    DefaultJoinerAppliedPreparation, DefaultJoinerCancellationPreparation,
    DefaultJoinerCandidatePreparation, DefaultJoinerInvitationPreparation,
    DefaultJoinerStartMaterial, DefaultMembershipBranchTransitionPreparation,
    DefaultMembershipSecurityUpdateAdapter, DefaultSponsorAdmissionActivation,
    DefaultSponsorCandidatePreparation, DefaultSponsorCommitPreparation,
    DefaultSponsorCompletePreparation, DefaultSponsorSettledPreparation,
    DeviceTrustObservationsAdapter, GatedMembershipHistoryExchange, GatedSpaceAdmissionTransport,
    MembershipActivationAdapter, MembershipMemberFactsAdapter, MembershipNetworkGate,
    OpenMlsHistoricalSignatureVerifier,
};
use uc_observability_contract::{diagnostics::DiagnosticTaskKind, uc_info};

#[cfg(not(feature = "lan-compat"))]
struct UnavailableMobileDeviceLookup;

#[cfg(not(feature = "lan-compat"))]
#[async_trait::async_trait]
impl uc_core::ports::FindMobileDeviceByIdPort for UnavailableMobileDeviceLookup {
    async fn find_by_device_id(
        &self,
        _device_id: &uc_core::mobile_sync::MobileDeviceId,
    ) -> Result<Option<uc_core::mobile_sync::MobileDevice>, uc_core::mobile_sync::MobileDeviceError>
    {
        Ok(None)
    }
}

/// Engine 完成网络装配后一次性交给 Application 的被动 adapter 集合。
///
/// 该集合按值移交，领域运行期只由Application持有。
pub(crate) struct SyncApplicationAdapters {
    pub binding: ApplicationNetworkBinding,
    pub active_pull_client: Arc<dyn uc_core::ports::ActiveClipboardPullClientPort>,
}

pub(crate) struct PreparedSyncSession {
    pub application: SyncApplicationAdapters,
    pub prepared_session: PreparedIrohSession,
}

/// 网络与 Application endpoint 装配失败；启动调用方将其作为致命错误处理。
#[derive(Debug, thiserror::Error)]
pub enum SyncSessionPreparationError {
    #[error(transparent)]
    IrohNode(#[from] IrohNodeError),
    #[error(transparent)]
    ApplicationUpgrade(#[from] uc_application::facade::ApplicationUpgradeError),
}

/// 从已完成的 Application factory 与 Engine adapter 构造共享 Iroh 网络。
/// 该函数绑定 endpoint 并启动 Router，必须在 Tokio runtime 内调用。
#[instrument(skip_all)]
pub async fn prepare_sync_session(
    application: &ApplicationAssembly,
    space_setup: &SyncEngineDeps,
    current_app_version: &str,
    #[cfg(feature = "lan-compat")] mobile_sync_ports: uc_mobile_lan::MobileSyncPorts,
    #[cfg(feature = "dev-tools")] joiner_final_confirmation_gate: Arc<JoinerFinalConfirmationGate>,
    mut builder: IrohSessionBuilder,
) -> Result<PreparedSyncSession, SyncSessionPreparationError> {
    application
        .ensure_current_version(current_app_version)
        .await?;
    let identity_store = Arc::new(IrohIdentityStore::new(
        Arc::clone(&space_setup.iroh_identity_storage),
        Arc::new(Sha256IdentityFingerprintFactory),
    ));
    // 当前 Space 的全部网络能力先完整准备，最后由监督器一次发布。
    let handlers = builder.install_pairing_invitation(
        Arc::clone(&space_setup.device_identity),
        Arc::clone(&space_setup.settings),
    );
    let membership_identity = builder.build_membership_identity_adapter(
        Arc::clone(&space_setup.membership_session),
        Arc::clone(&space_setup.device_identity),
        Arc::clone(&space_setup.settings),
        Arc::clone(&space_setup.fingerprint),
    );
    let verified_peer_address_refresh = builder.build_verified_peer_address_refresh(
        Arc::clone(&space_setup.peer_addr_repo),
        Arc::clone(&space_setup.clock),
    );
    let branch_deps = builder.branch_recovery_connectivity_deps(
        space_setup.peer_addr_repo.clone(),
        space_setup.peer_access.clone(),
        space_setup.peer_access.clone(),
        space_setup.fingerprint.clone(),
    );
    let branch_manager = ConnectivityManager::new(
        branch_deps.backend,
        branch_deps.admission,
        ConnectivityPolicy {
            task_kind: DiagnosticTaskKind::MembershipBranchRecoveryConnectivityWorker,
            outbound_per_peer: 1,
            inbound_per_connection: 1,
            inbound_per_session: 8,
            reject_excess_inbound: true,
            waiters_per_peer: 4,
            waiters_per_session: 16,
            flights_per_session: 4,
            supervise_local_work: true,
            ..ConnectivityPolicy::default()
        },
    );
    let branch_wire: Arc<dyn BranchRecoveryWirePort> = Arc::new(BranchRecoveryWire);
    let membership_branch_recovery_channel = Arc::new(BranchRecoveryTransport::new(
        branch_manager.clone(),
        branch_wire.clone(),
    ));
    let (known_peer_contact_tx, known_peer_contacts) = broadcast::channel(64);
    // Presence is installed before the convergence owner is assembled so the
    // owner can expose reachability as an independent product fact.
    let (presence_deps, presence_environment) = builder.presence_connectivity_deps(
        space_setup.peer_addr_repo.clone(),
        space_setup.peer_access.clone(),
        space_setup.peer_access.clone(),
        space_setup.fingerprint.clone(),
    );
    let presence_manager = ConnectivityManager::new(
        presence_deps.backend,
        presence_deps.admission,
        ConnectivityPolicy {
            presence_retention: true,
            task_kind: DiagnosticTaskKind::PresenceConnectivityWorker,
            inbound_per_connection: 2,
            flights_per_session: 4,
            ..ConnectivityPolicy::default()
        },
    );
    let presence_owner = PeerReachabilityTransport::new(
        presence_manager,
        space_setup.peer_access.clone(),
        PeerReachabilityTransportPorts {
            wire: Arc::new(PeerReachabilityWire),
            environment: presence_environment,
        },
        space_setup.clock.clone(),
        known_peer_contact_tx,
    );
    builder.install_peer_reachability(presence_owner.clone())?;
    let peer_reachability = observe_peer_reachability(presence_owner);
    let history_connectivity = builder.history_connectivity_deps(
        space_setup.peer_addr_repo.clone(),
        space_setup.peer_access.clone(),
        space_setup.peer_access.clone(),
        space_setup.fingerprint.clone(),
        peer_reachability.clone(),
    );
    let history_binding = history_connectivity.admission.clone();
    let history_manager = ConnectivityManager::new(
        history_connectivity.backend,
        history_connectivity.admission,
        ConnectivityPolicy {
            task_kind: DiagnosticTaskKind::MembershipHistoryConnectivityWorker,
            accepted_stream_timeout: Some(Duration::from_secs(10)),
            idle_retention: Some(IdleRetentionPolicy {
                elapsed_clock: None,
                timeout: Duration::from_secs(30),
                capacity: 16,
            }),
            ..ConnectivityPolicy::default()
        },
    );
    let history_wire: Arc<dyn HistoryWirePort> = Arc::new(MembershipHistoryWire);
    let complete_history_transport = Arc::new(MembershipHistoryTransport::new(
        history_manager.clone(),
        history_wire.clone(),
    ));
    // Phase 96 INDIC-01:连接通道单一真相源。复用同一 endpoint +
    // peer_addr_repo,纯读 adapter 不装 ALPN handler。
    let connection_channel: Arc<dyn ConnectionChannelPort> =
        builder.install_connection_channel(Arc::clone(&space_setup.peer_addr_repo));
    let group_update_receiver = GroupUpdateEndpoint::unbound();
    #[cfg(feature = "dev-tools")]
    let legacy_group_updates = joiner_final_confirmation_gate
        .space_work_control()
        .uses_legacy_group_protocol();
    #[cfg(not(feature = "dev-tools"))]
    let legacy_group_updates = false;
    let group_profile = if legacy_group_updates {
        ConnectivityProfile::LegacyGroupUpdate
    } else {
        ConnectivityProfile::GroupUpdate
    };
    let group_connectivity = builder.connectivity_deps(
        group_profile,
        space_setup.peer_addr_repo.clone(),
        space_setup.peer_access.clone(),
        space_setup.peer_access.clone(),
        space_setup.fingerprint.clone(),
        peer_reachability.clone(),
    );
    let group_manager = ConnectivityManager::new(
        group_connectivity.backend,
        group_connectivity.admission,
        ConnectivityPolicy::default(),
    );
    let wire: Arc<dyn GroupUpdateWirePort> = Arc::new(GroupUpdateWire);
    #[cfg(feature = "dev-tools")]
    let wire: Arc<dyn GroupUpdateWirePort> = Arc::new(FaultableGroupUpdateWire::new(
        wire,
        joiner_final_confirmation_gate.space_work_control(),
    ));
    let group_update_dispatch: Arc<dyn GroupUpdateDispatchPort> = Arc::new(
        IrohGroupUpdateAdapter::new(group_manager.clone(), wire.clone()),
    );
    let group_update_wire = wire;
    let group_update_local_work: Arc<dyn GroupUpdateLocalWorkPort> =
        Arc::new(SessionGroupUpdateLocalWork);
    let complete_group_receiver: Arc<dyn GroupUpdateReceiverPort> = group_update_receiver.clone();
    let group_update_apply: Arc<dyn ApplyGroupUpdatePort> = space_setup.security_lifecycle.clone();
    #[cfg(feature = "dev-tools")]
    let group_update_apply: Arc<dyn ApplyGroupUpdatePort> =
        Arc::new(FaultableGroupUpdateReceiver::new(
            group_update_apply,
            joiner_final_confirmation_gate.space_work_control(),
        ));
    // Engine 组装完整连接负责人；业务调用方只持有发送与接收意图端口。
    let connectivity = builder.connectivity_deps(
        ConnectivityProfile::Clipboard,
        Arc::clone(&space_setup.peer_addr_repo),
        Arc::clone(&space_setup.peer_access) as Arc<dyn PeerIdentityDirectoryPort>,
        Arc::clone(&space_setup.peer_access) as Arc<dyn PeerAdmissionPort>,
        Arc::clone(&space_setup.fingerprint),
        Arc::clone(&peer_reachability),
    );
    let manager = ConnectivityManager::new(
        connectivity.backend,
        connectivity.admission,
        ConnectivityPolicy::default(),
    );
    let ClipboardHandlers {
        dispatch: clipboard_dispatch,
        receiver: clipboard_receiver,
    } = builder.install_clipboard(manager)?;
    let clipboard_dispatch: Arc<dyn ClipboardDispatchPort> = clipboard_dispatch;
    let clipboard_receiver: Arc<dyn ClipboardReceiverPort> = clipboard_receiver;
    // Install the active-clipboard state ALPN (0xC3) as an independent
    // sibling on the same node. A lone `.accept()` deeper in the node would
    // not be reachable from here — the handler has to be installed on this
    // builder before `spawn()`, so the seam is threaded through here. Produces
    // both the inbound receiver (broadcast of peer observations) and the
    // outbound dispatch port (re-broadcast of converged state), sharing the
    // endpoint + peer_addr_repo like install_clipboard.
    let active_connectivity = builder.connectivity_deps(
        ConnectivityProfile::ActiveClipboardState,
        space_setup.peer_addr_repo.clone(),
        space_setup.peer_access.clone(),
        space_setup.peer_access.clone(),
        space_setup.fingerprint.clone(),
        peer_reachability.clone(),
    );
    let active_manager = ConnectivityManager::new(
        active_connectivity.backend,
        active_connectivity.admission,
        ConnectivityPolicy {
            inbound_per_connection: 1,
            ..ConnectivityPolicy::default()
        },
    );
    let ActiveClipboardHandlers {
        dispatch: active_clipboard_dispatch,
        receiver: active_clipboard_receiver,
    } = builder.install_active_clipboard(active_manager)?;
    let active_clipboard_dispatch: Arc<dyn ActiveClipboardDispatchPort> = active_clipboard_dispatch;
    let active_clipboard_receiver: Arc<dyn ActiveClipboardReceiverPort> = active_clipboard_receiver;
    // 同一存储上的交换能力交给唯一 Application 负责人；Router 不再由 SDK 自管。
    // SQLite 引用与纯本地存储能力交给 transfer，完整条目提交仍归既有接收权威。
    let blob_connectivity = builder.connectivity_deps(
        ConnectivityProfile::Blob,
        Arc::clone(&space_setup.peer_addr_repo),
        Arc::clone(&space_setup.peer_access) as Arc<dyn PeerIdentityDirectoryPort>,
        Arc::clone(&space_setup.peer_access) as Arc<dyn PeerAdmissionPort>,
        Arc::clone(&space_setup.fingerprint),
        Arc::clone(&peer_reachability),
    );
    #[cfg(all(test, feature = "dev-tools", target_os = "linux"))]
    let stream_fixture_path = space_setup.iroh_blob_store_dir.clone();
    let BlobHandlers {
        blob_transfer,
        transport: blob_transport,
        read_store: blob_read_store,
    } = builder
        .install_blobs(
            space_setup.iroh_blob_store_dir.clone(),
            move |exchange, lifecycle| {
                #[cfg(all(test, feature = "dev-tools", target_os = "linux"))]
                let exchange = receive_streams::decorate(&stream_fixture_path, exchange);
                BlobTransport::new(
                    blob_connectivity.backend,
                    blob_connectivity.admission,
                    exchange,
                    lifecycle,
                )
            },
        )
        .await?;
    #[cfg(all(test, feature = "dev-tools"))]
    let blob_transfer = decorate_blob(&space_setup.iroh_blob_store_dir, blob_transfer);
    #[cfg(all(test, feature = "dev-tools"))]
    let blob_read_store =
        receive_exports::decorate(&space_setup.iroh_blob_store_dir, blob_read_store);

    let blob_read_store = observability::observe_blob_exports(blob_read_store);

    // Application 先构造认证 endpoint；Space 持续维护要等 Router 就绪后
    // 才由 ApplicationRuntime 启动。
    let transport_material =
        builder.admission_transport_material(Arc::clone(&space_setup.fingerprint));
    let historical_signatures = Arc::new(OpenMlsHistoricalSignatureVerifier);
    let membership_network_gate = MembershipNetworkGate::active();
    let admission_transport: Arc<dyn uc_application::deps::SpaceAdmissionTransportPort> =
        Arc::new(GatedSpaceAdmissionTransport::new(
            Arc::clone(&membership_network_gate),
            builder.space_admission_transport(),
        ));
    #[cfg(feature = "dev-tools")]
    let admission_transport: Arc<dyn uc_application::deps::SpaceAdmissionTransportPort> =
        Arc::new(ControlledSpaceAdmissionTransport::new(
            admission_transport,
            joiner_final_confirmation_gate.space_work_control(),
        ));
    let membership_history_transport = Arc::new(GatedMembershipHistoryExchange::new(
        Arc::clone(&membership_network_gate),
        Arc::clone(&complete_history_transport),
    ));
    #[cfg(feature = "dev-tools")]
    let membership_history_synchronization_transport =
        Arc::new(RecordedMembershipHistoryExchange::new(
            membership_history_transport.clone(),
            joiner_final_confirmation_gate.space_work_control(),
        ));
    #[cfg(not(feature = "dev-tools"))]
    let membership_history_synchronization_transport = membership_history_transport.clone();
    let membership_security = Arc::new(DefaultMembershipSecurityUpdateAdapter::new(
        Arc::clone(&space_setup.space_access.group_revocation),
        Arc::clone(&space_setup.clock),
    ));
    let local_device_id = space_setup.device_identity.current_device_id();
    let local_identity: Arc<dyn LocalIdentityPort> = identity_store;
    let build_admission = || SpaceAdmissionAdapters {
        re_pairing_state_store: Arc::clone(&space_setup.re_pairing_state_store),
        prepare_joiner_invitation: Arc::new(DefaultJoinerInvitationPreparation),
        resolve_joiner_invitation: handlers.joiner_invitation_resolver,
        joiner_start_material: Arc::new(DefaultJoinerStartMaterial::new(
            local_device_id,
            Arc::clone(&space_setup.settings),
            transport_material.clone(),
        )),
        joiner_start_state: space_setup.admission_state.clone()
            as Arc<dyn uc_application::deps::JoinerStartStatePort>,
        current_join_admission_state: space_setup.admission_state.clone()
            as Arc<dyn uc_application::deps::CurrentJoinAdmissionStatePort>,
        prepare_joiner_cancellation: Arc::new(DefaultJoinerCancellationPreparation),
        pending_admission_recovery_state: space_setup.admission_state.clone()
            as Arc<dyn uc_application::deps::PendingAdmissionRecoveryStatePort>,
        space_admission_transport: admission_transport,
        sponsor_admission_state: space_setup.admission_state.clone()
            as Arc<dyn uc_application::deps::SponsorAdmissionStatePort>,
        prepare_sponsor_candidate: Arc::new(DefaultSponsorCandidatePreparation::new(
            local_device_id,
            transport_material.clone(),
            Arc::clone(&space_setup.current_member_signatures),
            historical_signatures.clone(),
            Arc::clone(&space_setup.space_access.prepare_sponsor_admission_security),
        )),
        prepare_sponsor_commit: Arc::new(DefaultSponsorCommitPreparation::new(
            historical_signatures.clone(),
        )),
        prepare_sponsor_complete: Arc::new(DefaultSponsorCompletePreparation::new(
            local_device_id,
            Arc::clone(&space_setup.current_member_signatures),
            historical_signatures.clone(),
        )),
        activate_sponsor_admission: Arc::new(DefaultSponsorAdmissionActivation::new(
            Arc::clone(&space_setup.space_access.activate_sponsor_admission_security),
            historical_signatures.clone(),
        )),
        prepare_sponsor_settled: Arc::new(DefaultSponsorSettledPreparation),
        prepare_joiner_candidate: Arc::new(DefaultJoinerCandidatePreparation::new(
            historical_signatures.clone(),
            Arc::clone(&space_setup.space_access.prepare_admission_target_access),
        )),
        prepare_joiner_applied: Arc::new(DefaultJoinerAppliedPreparation::new(
            historical_signatures.clone(),
        )),
        prepare_joiner_activation: Arc::new(DefaultJoinerActivationPreparation::new(
            historical_signatures.clone(),
            Arc::clone(&space_setup.admission_space_transition),
            Arc::clone(&space_setup.fingerprint),
        )),
        joiner_activation_state: space_setup.admission_state.clone()
            as Arc<dyn uc_application::deps::JoinerActivationStatePort>,
        execute_joiner_activation: {
            let activation = Arc::new(DefaultJoinerActivationExecutor::new(
                Arc::clone(&space_setup.admission_space_transition),
                historical_signatures.clone(),
            ))
                as Arc<dyn uc_application::deps::ExecuteJoinerActivationPort>;
            #[cfg(feature = "dev-tools")]
            let activation = Arc::new(GatedJoinerActivation::new(
                activation,
                Arc::clone(&joiner_final_confirmation_gate),
            ))
                as Arc<dyn uc_application::deps::ExecuteJoinerActivationPort>;
            activation
        },
        current_join_status: space_setup.admission_state.clone()
            as Arc<dyn uc_application::deps::LoadCurrentJoinStatusPort>,
    };
    let (membership, observed_group_receiver, observe_history_endpoint) =
        observability::observe_membership(
            SpaceMembershipAdapters {
                membership_records: space_setup.membership_ledger.clone()
                    as Arc<dyn uc_application::deps::MembershipRecordStorePort>,
                peer_access: Arc::clone(&space_setup.peer_access),
                historical_membership_signatures: historical_signatures.clone(),
                current_member_signatures: Arc::clone(&space_setup.current_member_signatures),
                membership_identity: membership_identity.clone(),
                membership_announcement: membership_identity,
                device_trust_observations: Arc::new(DeviceTrustObservationsAdapter::new(
                    Arc::clone(&space_setup.member_repo),
                    Arc::clone(&peer_reachability),
                )),
                membership_history_transport: membership_history_synchronization_transport,
                verified_peer_address_refresh: verified_peer_address_refresh.clone(),
                membership_branch_recovery_channel,
                membership_branch_recovery_recipient: Arc::clone(
                    &space_setup
                        .space_access
                        .prepare_membership_branch_recovery_recipient,
                ),
                membership_branch_transition: Arc::new(
                    DefaultMembershipBranchTransitionPreparation::new(Arc::clone(
                        &space_setup.active_generation_manifest_store,
                    )),
                ),
                membership_branch_transition_executor: Arc::clone(
                    &space_setup.membership_branch_transition_executor,
                ),
                membership_branch_recovery_material: Arc::clone(
                    &space_setup
                        .space_access
                        .prepare_membership_branch_recovery_material,
                ),
                apply_membership_member_facts: Arc::new(MembershipMemberFactsAdapter::new(
                    Arc::clone(&space_setup.member_repo),
                    Arc::clone(&space_setup.trusted_peer_repo),
                    Arc::clone(&space_setup.peer_addr_repo),
                    Arc::clone(&space_setup.device_identity),
                    Arc::clone(&space_setup.clock),
                )),
                apply_membership_security: membership_security,
                activate_membership_effect: Arc::new(MembershipActivationAdapter::new(Arc::clone(
                    &peer_reachability,
                ))),
                restricted_membership_delivery: membership_history_transport,
                group_update_store: Arc::clone(&space_setup.space_access.group_revocation),
                group_update_apply,
                group_update_receiver,
                group_update_wire,
                group_update_local_work,
                group_update_dispatch: {
                    #[cfg(feature = "dev-tools")]
                    let dispatch = Arc::new(RecordedGroupUpdateDispatch::new(
                        group_update_dispatch,
                        joiner_final_confirmation_gate.space_work_control(),
                    ));
                    #[cfg(not(feature = "dev-tools"))]
                    let dispatch = group_update_dispatch;
                    dispatch
                },
                membership_network_activity: membership_network_gate,
            },
            complete_group_receiver,
        );
    builder.install_group_updates(group_manager, observed_group_receiver, legacy_group_updates)?;
    let admission = build_admission();
    let space_runtime = SpaceRuntimeAdapters {
        admission,
        membership,
    };
    #[cfg(feature = "lan-compat")]
    let mobile_device_repo = Arc::clone(&mobile_sync_ports.devices.find_by_id);
    #[cfg(not(feature = "lan-compat"))]
    let mobile_device_repo: Arc<dyn uc_core::ports::FindMobileDeviceByIdPort> =
        Arc::new(UnavailableMobileDeviceLookup);
    let directory_publisher = FsAtomicPublisher::new();
    #[cfg(all(test, feature = "dev-tools", target_os = "linux"))]
    let directory_publisher = directory_probe::decorate(directory_publisher);
    let clipboard =
        crate::assembly::observability::observe_clipboard(ApplicationClipboardAdapters {
            peer_addresses: Arc::clone(&space_setup.peer_addr_repo),
            peer_reachability: Arc::clone(&peer_reachability),
            clipboard_dispatch,
            clipboard_receiver,
            local_identity: Arc::clone(&local_identity),
            mobile_device_repo,
            active_receiver: active_clipboard_receiver,
            active_dispatch: active_clipboard_dispatch,
            active_pull_publisher: directory_publisher,
            active_pull_target_reserver: FsInboundFileTarget::new(Arc::clone(
                &space_setup.settings,
            )),
            active_pull_hidden_marker: FsHiddenPathMarker::new(),
            staging_cleanup: FsDirectoryStagingCleaner::new(),
        });
    let progress_connectivity = builder.connectivity_deps(
        ConnectivityProfile::TransferProgress,
        Arc::clone(&space_setup.peer_addr_repo),
        Arc::clone(&space_setup.peer_access) as Arc<dyn PeerIdentityDirectoryPort>,
        Arc::clone(&space_setup.peer_access) as Arc<dyn PeerAdmissionPort>,
        Arc::clone(&space_setup.fingerprint),
        Arc::clone(&peer_reachability),
    );
    let progress_backend = progress_connectivity.backend;
    #[cfg(all(test, feature = "dev-tools"))]
    let progress_backend = receive_exports::decorate_progress_backend(
        &space_setup.iroh_blob_store_dir,
        progress_backend,
    );
    let progress_wire: Arc<dyn ProgressWirePort> = Arc::new(TransferProgressWire);
    #[cfg(all(test, feature = "dev-tools"))]
    let progress_wire =
        receive_exports::decorate_progress_wire(&space_setup.iroh_blob_store_dir, progress_wire);
    #[cfg(all(test, feature = "dev-tools"))]
    let progress_probe_path = space_setup.iroh_blob_store_dir.clone();
    let application_network = application.assemble_network(ApplicationNetworkAdapters {
        transfer_progress: TransferProgressNetwork {
            backend: progress_backend,
            admission: progress_connectivity.admission,
            wire: progress_wire,
            snapshot_wire: Arc::new(TransferProgressWire),
            inbound: Box::new(move |receiver, legacy, queries| {
                #[cfg(all(test, feature = "dev-tools"))]
                let receiver =
                    receive_exports::decorate_progress_receiver(&progress_probe_path, receiver);
                #[cfg(all(test, feature = "dev-tools"))]
                let queries =
                    receive_exports::decorate_progress_queries(&progress_probe_path, queries);
                Arc::new(TransferProgressInboundAdapter::new(
                    receiver, legacy, queries,
                ))
            }),
            clock: Arc::new(SystemElapsedClock),
        },
        blob_downloads: Arc::new(BlobDownloadResources {
            transport: blob_transport,
            store: blob_read_store,
            source: Arc::new(IrohBlobSourceResolver::new(
                space_setup.peer_access.clone(),
                space_setup.fingerprint.clone(),
            )),
        }),
        blob_transfer: Arc::clone(&blob_transfer),
        blob_reference: Arc::clone(&space_setup.blob_reference_repo),
        space: ApplicationSpaceAdapters {
            connection_hints: builder
                .connection_hints(
                    Arc::clone(&space_setup.peer_access) as Arc<dyn PeerIdentityDirectoryPort>,
                    Arc::clone(&space_setup.fingerprint),
                )
                .await,
            current_engine_version: env!("CARGO_PKG_VERSION").to_owned(),
            admission_credentials: space_setup.admission_credentials.clone()
                as Arc<dyn uc_application::deps::PrepareSpaceAdmissionCredentialsPort>,
            encryption_passphrase_change: space_setup.encryption_passphrase_change.clone()
                as Arc<dyn uc_application::deps::ApplyEncryptionPassphraseChangePort>,
            local_identity: Arc::clone(&local_identity),
            pairing_invitation: handlers.invitation,
            pairing_invitation_addresses: handlers.invitation_addresses,
            pairing_invitation_by_address: handlers.invitation_by_address,
            peer_reachability: Arc::clone(&peer_reachability),
            analytics: Arc::clone(&space_setup.analytics_facade),
            connection_channel: Some(Arc::clone(&connection_channel)),
            device_management_reset_data: Arc::clone(&space_setup.device_management_reset_data),
            relationship_reset: Arc::clone(&space_setup.relationship_reset),
            space_security_reset: Arc::clone(&space_setup.space_security_reset),
            runtime: space_runtime,
            peer_reachability_changed_events: peer_reachability.subscribe(),
            known_peer_contacts,
        },
        clipboard,
    });
    builder.install_space_admission(
        crate::assembly::observability::observe_admission_endpoint(
            application_network.space_admission_endpoint(),
        ),
        space_setup.admission_credentials.clone()
            as Arc<dyn uc_infra_p2p::network::iroh::SpaceAdmissionChannelCredentialPort>,
    )?;
    let history_endpoint = application_network.membership_history_endpoint();
    #[cfg(feature = "dev-tools")]
    let history_endpoint = Arc::new(GatedMembershipHistoryEndpoint::new(
        history_endpoint,
        joiner_final_confirmation_gate.space_work_control(),
    ));
    let history_receiver = Arc::new(MembershipHistoryReceiver::new(
        history_manager.clone(),
        history_wire,
        history_binding,
        Arc::new(SessionMembershipHistoryLocalWork),
        observe_history_endpoint(history_endpoint),
    ));
    builder.install_membership_history_exchange(history_manager, history_receiver)?;
    let branch_receiver = Arc::new(BranchRecoveryReceiver::new(
        branch_wire,
        Arc::new(BranchRecoveryLocalWork),
        application_network.membership_branch_recovery_endpoint(),
    ));
    builder.install_managed_branch_recovery(branch_manager, branch_receiver)?;

    // Install the active-clipboard pull ALPN (0xC2, issue #1017 PR8) as a
    // further independent sibling, before `spawn()`. The serve port reuses the
    // resend crypto chain (reconstruct → publish blobs re-signing self-pinned
    // tickets, D3 → encode V3 → encrypt, D4); the returned client port drives
    // the inbound seam's on-demand pull.
    let pull_connectivity = builder.connectivity_deps(
        ConnectivityProfile::ActiveClipboardPull,
        space_setup.peer_addr_repo.clone(),
        space_setup.peer_access.clone(),
        space_setup.peer_access.clone(),
        space_setup.fingerprint.clone(),
        peer_reachability.clone(),
    );
    let pull_manager = ConnectivityManager::new(
        pull_connectivity.backend,
        pull_connectivity.admission,
        ConnectivityPolicy {
            inbound_per_connection: 1,
            ..ConnectivityPolicy::default()
        },
    );
    builder.install_transfer_progress(application_network.transfer_progress_endpoint())?;
    let preparation = application_network.active_clipboard_pull_preparation();
    #[cfg(all(test, feature = "dev-tools"))]
    let preparation = decorate_preparation(&space_setup.iroh_blob_store_dir, preparation);
    let ActiveClipboardPullHandlers {
        client: active_clipboard_pull_client,
    } = builder.install_active_clipboard_pull(
        pull_manager,
        preparation,
        application_network.current_member_scope(),
    )?;

    let prepared_session = builder.finish();

    uc_info!("Iroh adapters registered against the Application network binding");
    Ok(PreparedSyncSession {
        application: SyncApplicationAdapters {
            binding: application_network,
            active_pull_client: active_clipboard_pull_client,
        },
        prepared_session,
    })
}
