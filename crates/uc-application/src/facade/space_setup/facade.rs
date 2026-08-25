//! The only public application entry for Space lifecycle, admission, member
//! trust, roster, reset, and session actions. Network adapters receive the two
//! authenticated endpoints exposed here; all workflow state remains private.

use std::net::IpAddr;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::instrument;

use crate::facade::roster::{MemberRosterDeps, MemberRosterFacade};
use crate::facade::space_setup::commands::{
    InitializeSpaceInput, IssuePairingInvitationResult, UnlockSpaceInput, UnlockSpaceResult,
};
use crate::facade::space_setup::deps::{
    SpaceAdmissionDeps, SpaceFacadeDeps, SpaceSessionDeps, SpaceTransitionDeps,
};
use crate::facade::space_setup::errors::IssuePairingInvitationError;
use crate::space::admission::complete_pending_space_transition::{
    CompletePendingSpaceTransitionError, CompletePendingSpaceTransitionUseCase,
};
use crate::space::admission::invitation::cancel::{
    CancelInvitationError, CancelPairingInvitationUseCase,
};
use crate::space::admission::invitation::issue::IssuePairingInvitationUseCase;
use crate::space::admission::invitation::issue_for_address::IssuePairingInvitationForAddressUseCase;
use crate::space::admission::invitation::query_addresses::{
    PairingInvitationAddressCandidate, QueryPairingInvitationAddressesError,
    QueryPairingInvitationAddressesUseCase,
};
use crate::space::admission::invitation::InMemoryPairingInvitationHolder;
use crate::space::admission::invitation::PairingInvitationIssuer;
use crate::space::admission::join_space::{JoinSpaceError, JoinSpaceInput, JoinSpaceResult};
use crate::space::admission::query_pending_space_transition::{
    QueryPendingSpaceTransitionError, QueryPendingSpaceTransitionUseCase,
};
use crate::space::application::SpaceApplication;
use crate::space::initialize_space::{
    InitializeSpaceError, InitializeSpaceRequest, InitializeSpaceResult, InitializeSpaceUseCase,
};
use crate::space::lock_space_session::{LockSpaceSessionError, LockSpaceSessionUseCase};
use crate::space::query_space_access_state::{
    QuerySpaceAccessStateError, QuerySpaceAccessStateUseCase, SpaceAccessState,
};
use crate::space::query_space_setup_state::{
    QuerySetupStateError, QuerySpaceSetupStateUseCase, SetupStateView,
};
use crate::space::re_pairing::RePairingState;
use crate::space::rebuild_space::{
    RebuildSpaceUseCase, SpaceMembershipRebuilder, SpaceRebuildTransition,
};
use crate::space::recover_space_session::{
    RecoverSpaceSessionError, RecoverSpaceSessionResult, RecoverSpaceSessionUseCase,
};
use crate::space::reset_space::{
    QueryCommittedDeviceManagementResetUseCase, ResetSpaceError, ResetSpaceUseCase,
};
use crate::space::session::combine_space_session_activity;
use crate::space::unlock_space::{PostSessionReadiness, UnlockSpaceError, UnlockSpaceUseCase};
use crate::space::upgrade_space::UpgradeSpaceUseCase;
use uc_core::ids::DeviceId;

/// Space-lifecycle facade (A1 initialise, A2 unlock, B1 issue invitation,
/// B2 redeem invitation, P7e inbound subscriber, F2 shutdown).
pub struct SpaceFacade {
    // 初始化空间
    initialize_space: Arc<InitializeSpaceUseCase>,
    // 解锁空间
    unlock_space: Arc<UnlockSpaceUseCase>,
    cancel_pairing_invitation: Arc<CancelPairingInvitationUseCase>,
    issue_pairing_invitation: Arc<IssuePairingInvitationUseCase>,
    issue_pairing_invitation_for_address: Arc<IssuePairingInvitationForAddressUseCase>,
    query_pairing_invitation_addresses: Arc<QueryPairingInvitationAddressesUseCase>,
    complete_pending_space_transition: Arc<CompletePendingSpaceTransitionUseCase>,
    query_pending_space_transition: Arc<QueryPendingSpaceTransitionUseCase>,
    session_activity: Arc<dyn crate::space::session::SpaceSessionActivityPort>,
    membership_maintenance:
        Arc<dyn crate::space::remove_space_member::WakeSpaceMembershipMaintenancePort>,
    lock_space_session: Arc<LockSpaceSessionUseCase>,
    recover_space_session: Arc<RecoverSpaceSessionUseCase>,
    query_space_access_state: Arc<QuerySpaceAccessStateUseCase>,
    query_space_setup_state: Arc<QuerySpaceSetupStateUseCase>,
    member_roster: MemberRosterFacade,
    reset_space: Arc<ResetSpaceUseCase>,
    query_committed_device_management_reset: Arc<QueryCommittedDeviceManagementResetUseCase>,
    membership_history_endpoint:
        Arc<dyn uc_core::membership::MembershipHistoryExchangeEndpointPort>,
    space_admission_endpoint: Arc<dyn crate::deps::HandleSpaceAdmissionMessagePort>,
    application: Mutex<Option<SpaceApplication>>,
}

impl SpaceFacade {
    /// Wire the complete Space application from one passive dependency bundle.
    pub fn new(deps: SpaceFacadeDeps) -> Self {
        Self::new_internal(deps)
    }

    fn new_internal(deps: SpaceFacadeDeps) -> Self {
        let SpaceFacadeDeps {
            session,
            admission,
            transition,
            application,
            membership_presence_events,
        } = deps;
        let SpaceTransitionDeps {
            device_management_reset_data,
            relationship_reset,
            space_security_reset,
            space_rebuild_progress,
            re_pairing_state_store,
        } = transition;
        let re_pairing_state = Arc::new(RePairingState::new(re_pairing_state_store));
        let invitation_holder = Arc::new(InMemoryPairingInvitationHolder::new());
        let application = SpaceApplication::start(
            application,
            membership_presence_events,
            Arc::clone(&invitation_holder),
            Arc::clone(&re_pairing_state)
                as Arc<dyn crate::space::re_pairing::ResolveRePairingPort>,
        );
        let membership_initializer = application.initialize_membership();
        let membership_admission = application.query_membership_admission();
        let peer_scope = application.current_scope();
        let membership_reset = application.membership_reset();
        let membership_history_endpoint = application.membership_history_endpoint();
        let space_admission_endpoint = application.space_admission_endpoint();
        let complete_pending_space_transition = application.complete_pending_space_transition();
        let query_pending_space_transition = application.query_pending_space_transition();
        let membership_session_activity = application.membership_session_activity();
        let membership_maintenance = application.membership_maintenance_wake();
        let SpaceSessionDeps {
            space_access,
            mobile_consumable_backfill,
            engine_version_state,
            current_engine_version,
            current_space_identity,
            initial_space_activation,
            activity: application_activity,
        } = session;
        let activity =
            combine_space_session_activity(membership_session_activity, application_activity);
        let SpaceAdmissionDeps {
            local_identity,
            device_identity,
            member_repo,
            settings,
            clock,
            pairing_invitation,
            pairing_invitation_addresses,
            pairing_invitation_by_address,
            presence,
            analytics,
            connection_channel,
        } = admission;
        let member_roster = MemberRosterFacade::new(MemberRosterDeps {
            member_repo: Arc::clone(&member_repo),
            local_identity: Arc::clone(&local_identity),
            presence: Arc::clone(&presence),
            connection_channel,
            peer_scope: Arc::clone(&peer_scope),
        })
        .with_space_protection(Arc::clone(&space_access.space_protection));
        // Invitation holder is purely an internal flow-state component
        // (§11.4) — construct it here so bootstrap never sees the type.
        // Slice4 P3 T3.2 · facade-local handle for `cancel_invitation`
        // / `query_setup_state` snapshots; the use case + orchestrator
        // already own their own `Arc::clone`s below.
        let invitation_holder_for_facade = Arc::clone(&invitation_holder);
        let membership_rebuilder = Arc::new(SpaceMembershipRebuilder::new(
            Arc::clone(&member_repo),
            Arc::clone(&relationship_reset),
            Arc::clone(&membership_initializer),
        ));
        let space_rebuild_progress_for_facade = Arc::clone(&space_rebuild_progress);
        let query_space_setup_state = Arc::new(QuerySpaceSetupStateUseCase::new(
            Arc::clone(&current_space_identity),
            Arc::clone(&invitation_holder_for_facade),
            Arc::clone(&settings),
            Arc::clone(&re_pairing_state),
        ));
        let cancel_pairing_invitation = Arc::new(CancelPairingInvitationUseCase::new(Arc::clone(
            &invitation_holder_for_facade,
        )));
        let rebuild_transition = Arc::new(SpaceRebuildTransition::new(
            device_management_reset_data,
            space_security_reset,
            Arc::clone(&current_space_identity),
            space_rebuild_progress,
            Arc::clone(&re_pairing_state),
        ));
        let rebuild_space = Arc::new(RebuildSpaceUseCase::new(
            Arc::clone(&settings),
            Arc::clone(&local_identity),
            Arc::clone(&device_identity),
            rebuild_transition,
            Arc::clone(&space_access.adopt_isolated_space),
            membership_reset,
            membership_rebuilder,
            Arc::clone(&clock),
        ));
        let reset_space = Arc::new(ResetSpaceUseCase::new(
            Arc::clone(&rebuild_space),
            Arc::clone(&invitation_holder)
                as Arc<dyn crate::space::reset_space::ports::PendingSpaceInvitationResetPort>,
        ));
        let query_committed_device_management_reset =
            Arc::new(QueryCommittedDeviceManagementResetUseCase::new(
                Arc::clone(&space_rebuild_progress_for_facade),
                Arc::clone(&current_space_identity),
            ));
        let upgrade_space = Arc::new(UpgradeSpaceUseCase::new(
            current_engine_version,
            rebuild_space,
            engine_version_state,
            Arc::clone(&current_space_identity),
        ));

        let initialize_space = Arc::new(InitializeSpaceUseCase::new(
            Arc::clone(&space_access.initialize),
            Arc::clone(&local_identity),
            Arc::clone(&device_identity),
            Arc::clone(&member_repo),
            membership_initializer,
            Arc::clone(&current_space_identity),
            initial_space_activation,
            Arc::clone(&settings),
            Arc::clone(&clock),
            Arc::clone(&analytics),
        ));
        let pairing_invitation_issuer = Arc::new(PairingInvitationIssuer::new(
            Arc::clone(&device_identity),
            Arc::clone(&clock),
            Arc::clone(&invitation_holder),
            Arc::clone(&analytics),
            membership_admission,
        ));
        let issue_pairing_invitation = Arc::new(IssuePairingInvitationUseCase::new(
            Arc::clone(&pairing_invitation),
            Arc::clone(&pairing_invitation_issuer),
        ));
        let issue_pairing_invitation_for_address =
            Arc::new(IssuePairingInvitationForAddressUseCase::new(
                pairing_invitation_by_address,
                pairing_invitation_issuer,
            ));
        let query_pairing_invitation_addresses = Arc::new(
            QueryPairingInvitationAddressesUseCase::new(pairing_invitation_addresses),
        );
        let session_readiness = Arc::new(PostSessionReadiness::new(
            Arc::clone(&upgrade_space),
            Arc::clone(&mobile_consumable_backfill),
            Arc::clone(&member_repo),
        ));
        let unlock_space = Arc::new(UnlockSpaceUseCase::new(
            Arc::clone(&space_access.unlock),
            Arc::clone(&current_space_identity),
            Arc::clone(&session_readiness),
            Arc::clone(&analytics),
        ));
        let lock_space_session = Arc::new(LockSpaceSessionUseCase::new(
            Arc::clone(&current_space_identity),
            Arc::clone(&space_access.lock),
            Arc::clone(&activity),
        ));
        let recover_space_session = Arc::new(RecoverSpaceSessionUseCase::new(
            Arc::clone(&current_space_identity),
            Arc::clone(&space_access.resume_session),
            Arc::clone(&session_readiness),
            Arc::clone(&activity),
        ));
        let query_space_access_state = Arc::new(QuerySpaceAccessStateUseCase::new(
            Arc::clone(&current_space_identity),
            Arc::clone(&space_access.is_unlocked),
        ));

        Self {
            initialize_space,
            unlock_space,
            cancel_pairing_invitation,
            issue_pairing_invitation,
            issue_pairing_invitation_for_address,
            query_pairing_invitation_addresses,
            complete_pending_space_transition,
            query_pending_space_transition,
            session_activity: activity,
            membership_maintenance,
            lock_space_session,
            recover_space_session,
            query_space_access_state,
            query_space_setup_state,
            member_roster,
            reset_space,
            query_committed_device_management_reset,
            membership_history_endpoint,
            space_admission_endpoint,
            application: Mutex::new(Some(application)),
        }
    }

    pub fn membership_history_endpoint(
        &self,
    ) -> Arc<dyn uc_core::membership::MembershipHistoryExchangeEndpointPort> {
        Arc::clone(&self.membership_history_endpoint)
    }

    pub fn space_admission_endpoint(
        &self,
    ) -> Arc<dyn crate::deps::HandleSpaceAdmissionMessagePort> {
        Arc::clone(&self.space_admission_endpoint)
    }

    pub async fn lock_space_session(&self) -> Result<(), LockSpaceSessionError> {
        self.lock_space_session.execute().await
    }

    pub async fn recover_space_session(
        &self,
    ) -> Result<RecoverSpaceSessionResult, RecoverSpaceSessionError> {
        let result = self.recover_space_session.execute().await?;
        if result.resumed {
            self.membership_maintenance.wake();
        }
        Ok(result)
    }

    pub async fn query_space_access_state(
        &self,
    ) -> Result<SpaceAccessState, QuerySpaceAccessStateError> {
        self.query_space_access_state.execute().await
    }

    /// A1 · Create the encrypted space on a fresh device. On success the
    /// presence cache is primed (F1).
    #[instrument(skip_all)]
    pub async fn initialize_space(
        &self,
        input: InitializeSpaceInput,
    ) -> Result<InitializeSpaceResult, InitializeSpaceError> {
        let request = InitializeSpaceRequest {
            passphrase: input.passphrase,
            passphrase_confirm: input.passphrase_confirm,
            device_name: input.device_name,
        };
        let out = self.initialize_space.execute(request).await?;
        self.session_activity
            .resume_after_session_ready()
            .await
            .map_err(|error| InitializeSpaceError::internal(anyhow::anyhow!(error)))?;
        self.membership_maintenance.wake();
        Ok(out)
    }

    /// A2 · Unlock the encrypted space after a restart. On success the
    /// presence cache is primed (F1).
    #[instrument(skip_all)]
    pub async fn unlock_space(
        &self,
        input: UnlockSpaceInput,
    ) -> Result<UnlockSpaceResult, UnlockSpaceError> {
        let space_id = self.unlock_space.execute(input.passphrase).await?;
        self.session_activity
            .resume_after_session_ready()
            .await
            .map_err(|error| UnlockSpaceError::Internal(error.to_string()))?;
        self.membership_maintenance.wake();
        Ok(UnlockSpaceResult { space_id })
    }

    /// B1 · Ask the rendezvous service for a fresh invitation code and
    /// park the resulting aggregate in the application-layer holder.
    ///
    /// Does **not** auto-start the network: the adapter surfaces
    /// [`IssuePairingInvitationError::NetworkNotStarted`] if the runtime
    /// isn't up, letting the UI prompt the user to complete A1/A2 first.
    #[instrument(skip_all)]
    pub async fn issue_pairing_invitation(
        &self,
    ) -> Result<IssuePairingInvitationResult, IssuePairingInvitationError> {
        self.issue_pairing_invitation.execute().await
    }

    /// 按指定本机地址签发配对邀请。
    #[instrument(skip_all)]
    pub async fn issue_pairing_invitation_for_address(
        &self,
        selected_ip: IpAddr,
    ) -> Result<IssuePairingInvitationResult, IssuePairingInvitationError> {
        self.issue_pairing_invitation_for_address
            .execute(selected_ip)
            .await
    }

    /// 列出当前可用于签发配对邀请的本机地址。
    #[instrument(skip_all)]
    pub async fn list_pairing_invitation_addresses(
        &self,
    ) -> Result<Vec<PairingInvitationAddressCandidate>, QueryPairingInvitationAddressesError> {
        self.query_pairing_invitation_addresses.execute().await
    }

    /// B2 · Redeem a sponsor-issued invitation (joiner side).
    #[instrument(skip_all)]
    pub async fn join_space(
        &self,
        input: JoinSpaceInput,
    ) -> Result<JoinSpaceResult, JoinSpaceError> {
        let join = self
            .application
            .lock()
            .await
            .as_ref()
            .map(SpaceApplication::join_space)
            .ok_or_else(|| JoinSpaceError::SavedState("space application is closed".to_owned()))?;
        join.execute(input).await
    }

    pub async fn query_device_trust(
        &self,
    ) -> Result<
        crate::space::query_device_trust::DeviceTrustStatus,
        crate::space::query_device_trust::QueryDeviceTrustError,
    > {
        let query = self
            .application
            .lock()
            .await
            .as_ref()
            .map(SpaceApplication::query_device_trust)
            .ok_or(crate::space::query_device_trust::QueryDeviceTrustError::Unavailable)?;
        query.execute().await
    }

    pub async fn remove_space_member(
        &self,
        target: &DeviceId,
    ) -> Result<
        crate::space::remove_space_member::RemoveSpaceMemberResult,
        crate::space::remove_space_member::RemoveSpaceMemberError,
    > {
        let remove = self
            .application
            .lock()
            .await
            .as_ref()
            .map(SpaceApplication::remove_space_member)
            .ok_or(crate::space::remove_space_member::RemoveSpaceMemberError::Unavailable)?;
        remove.execute(target).await
    }

    pub async fn decide_device_trust_change(
        &self,
        input: crate::space::decide_device_trust_change::DecideDeviceTrustChange,
    ) -> Result<
        crate::space::decide_device_trust_change::DecideDeviceTrustChangeResult,
        crate::space::decide_device_trust_change::DecideDeviceTrustChangeError,
    > {
        let decide = self
            .application
            .lock()
            .await
            .as_ref()
            .map(SpaceApplication::decide_device_trust_change)
            .ok_or(
                crate::space::decide_device_trust_change::DecideDeviceTrustChangeError::Unavailable,
            )?;
        decide.execute(input).await
    }

    pub async fn cancel_space_join(
        &self,
        join_id: [u8; 16],
    ) -> Result<crate::space::admission::CurrentJoinStatus, crate::facade::CancelSpaceJoinError>
    {
        let cancel = self
            .application
            .lock()
            .await
            .as_ref()
            .map(SpaceApplication::cancel_space_join)
            .ok_or_else(|| {
                crate::facade::CancelSpaceJoinError::State("space application is closed".to_owned())
            })?;
        cancel.execute(join_id).await
    }

    pub async fn has_pending_space_transition(
        &self,
    ) -> Result<bool, QueryPendingSpaceTransitionError> {
        self.query_pending_space_transition.execute().await
    }

    pub async fn complete_pending_space_transition(
        &self,
    ) -> Result<crate::space::admission::CurrentJoinStatus, CompletePendingSpaceTransitionError>
    {
        let result = self.complete_pending_space_transition.execute().await?;
        self.membership_maintenance.wake();
        Ok(result)
    }

    /// Slice4 P3 T3.2 · Cancel any in-flight pairing invitation parked
    /// in the in-memory holder.
    ///
    /// Maps to `POST /v2/setup/cancel`. Returns
    /// [`CancelInvitationError::NotIssued`] when the holder is empty so
    /// the daemon can surface HTTP 409 and the UI can distinguish
    /// "nothing to cancel" from a transport failure.
    ///
    /// Does **not** change profile readiness — only Pending invitation
    /// aggregates are cleared. The rendezvous server is **not**
    /// notified: stateless v2 model treats invitations as pure local
    /// state, and any joiner that races a redeem against this cancel
    /// will simply hit `take_matching → NotFound` on the sponsor side.
    #[instrument(skip_all)]
    pub async fn cancel_invitation(&self) -> Result<(), CancelInvitationError> {
        self.cancel_pairing_invitation.execute().await
    }

    /// Rebuild this profile as a single-device space while retaining local
    /// content, settings, identity, and unlock material.
    #[instrument(skip_all)]
    pub async fn reset(&self) -> Result<(), ResetSpaceError> {
        self.reset_space.execute().await?;
        self.membership_maintenance.wake();
        Ok(())
    }

    pub async fn has_committed_device_management_reset(&self) -> Result<bool, ResetSpaceError> {
        self.query_committed_device_management_reset.execute().await
    }

    /// Slice4 P3 T3.2 · Read-only snapshot of setup state for the
    /// stateless v2 UI flow.
    ///
    /// Maps to `GET /v2/setup/state`. Composes three independent
    /// reads into a single response so the UI doesn't have to
    /// orchestrate them itself:
    /// * `has_completed` from the current Space identity.
    /// * `current_invitation` from the in-memory holder
    ///   (earliest-expiring Pending entry; `None` when the holder is
    ///   empty).
    /// * `device_name` from `Settings.general.device_name`.
    #[instrument(skip_all)]
    pub async fn query_setup_state(&self) -> Result<SetupStateView, QuerySetupStateError> {
        self.query_space_setup_state.execute().await
    }

    pub async fn list_members(
        &self,
    ) -> Result<Vec<crate::facade::roster::MemberSummary>, crate::facade::roster::RosterError> {
        self.member_roster.list_members().await
    }

    pub async fn list_roster_entries(
        &self,
    ) -> Result<Vec<crate::facade::roster::RosterEntry>, crate::facade::roster::RosterError> {
        self.member_roster.list_with_presence().await
    }

    pub async fn member_sync_preferences(
        &self,
        device_id: &str,
    ) -> Result<crate::facade::roster::MemberSyncPreferencesView, crate::facade::roster::RosterError>
    {
        self.member_roster.get_sync_preferences(device_id).await
    }

    pub async fn update_member_sync_preferences(
        &self,
        device_id: &str,
        patch: crate::facade::roster::MemberSyncPreferencesPatch,
    ) -> Result<crate::facade::roster::MemberSyncPreferencesView, crate::facade::roster::RosterError>
    {
        self.member_roster
            .update_sync_preferences(device_id, patch)
            .await
    }

    pub async fn space_protection(
        &self,
    ) -> Result<crate::facade::roster::SpaceProtectionView, crate::facade::roster::RosterError>
    {
        self.member_roster.query_space_protection().await
    }

    pub async fn list_peer_snapshots(
        &self,
    ) -> Result<Vec<crate::facade::roster::PeerSnapshotView>, crate::facade::roster::RosterError>
    {
        self.member_roster.list_peer_snapshots().await
    }

    pub fn subscribe_presence_events(
        &self,
    ) -> tokio::sync::broadcast::Receiver<uc_core::ports::PresenceEvent> {
        self.member_roster.subscribe_presence_events()
    }

    /// F2 · Tear down facade-owned background work cleanly on app exit.
    ///
    /// Slice 4 P5c: 历史上还会调 `network_control.stop_network()`,libp2p 走
    /// 完后 iroh router 由 `SyncEngineAssembly::shutdown` 直接收口,本入口
    /// 现在只剩 abort 入站 pairing orchestrator——让它的 `subscribe` receiver
    /// 立刻 drop,底层 adapter 才能释放事件 channel。
    #[instrument(skip_all)]
    pub async fn on_shutdown(&self) {
        if let Some(application) = self.application.lock().await.take() {
            application.shutdown().await;
        }
    }
}
