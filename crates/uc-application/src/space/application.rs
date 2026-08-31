use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use tokio::sync::broadcast;
use uc_core::membership::{
    CurrentMembershipAnnouncementPort, CurrentMembershipIdentityPort, GroupBootstrapPort,
    GroupRevocationPort, GroupUpdateDispatchPort, HistoricalMembershipSignatureVerifier,
    MembershipHistoryExchangeEndpointPort, MembershipHistoryExchangePort,
};
use uc_core::ports::PeerReachabilityChanged;

use crate::space::admission::{
    ActivateSponsorAdmissionPort, AdmissionRecoveryService, CurrentJoinAdmissionStatePort,
    ExecuteJoinerActivationPort, HandleAuthenticatedSpaceAdmissionMessagePort,
    JoinerActivationStatePort, JoinerAdmissionService, JoinerStartMaterialPort,
    JoinerStartStatePort, PendingAdmissionRecoveryStatePort, PrepareJoinerActivationPort,
    PrepareJoinerAppliedPort, PrepareJoinerCancellationPort, PrepareJoinerCandidatePort,
    PrepareJoinerInvitationPort, PrepareSponsorCandidatePort, PrepareSponsorCommitPort,
    PrepareSponsorCompletePort, PrepareSponsorSettledPort, ResolveJoinerInvitationPort,
    SpaceAdmissionProtocol, SpaceAdmissionTransportPort, SponsorAdmissionService,
    SponsorAdmissionStatePort,
};
use crate::space::membership::CurrentMemberSignaturePort;
use crate::space::membership::DecideDeviceTrustChangeUseCase;
use crate::space::membership::DeliverPendingGroupUpdatesUseCase;
use crate::space::membership::IssueMembershipBranchRecoveryUseCase;
use crate::space::membership::MembershipHistoryAntiEntropy;
use crate::space::membership::PreparedSpaceMembershipMaintenanceRuntime;
use crate::space::membership::QueryMembershipAdmissionUseCase;
use crate::space::membership::QueryMembershipConflictStatusPort;
use crate::space::membership::RecoverMembershipConflictUseCase;
use crate::space::membership::RemoveSpaceMemberUseCase;
use crate::space::membership::ResolveMembershipConflictUseCase;
use crate::space::membership::{
    ActivateMembershipEffectPort, ApplyMembershipMemberFactsPort, ApplyMembershipSecurityPort,
    CommitMembershipLedgerPort, CurrentSpaceMemberScopePort, DeliverRestrictedMembershipUseCase,
    InitializeSpaceMembershipUseCase, LoadMembershipLedgerPort, MembershipLedger,
    RePairingAwareMembershipActivation, RecoverMembershipEffectsUseCase,
    RestrictedMembershipDeliveryPort,
};
use crate::space::membership::{
    CleanupLegacyMembershipDataPort, MaintainSpaceMembershipDeps, MaintainSpaceMembershipUseCase,
    MembershipNetworkActivityPort, SpaceMembershipMaintenanceRuntime,
};
use crate::space::membership::{
    LoadCurrentJoinStatusPort, LoadDeviceTrustObservationsPort, QueryDeviceTrustUseCase,
};

struct DeferredMaintenanceWake {
    target: OnceLock<Arc<dyn crate::space::membership::WakeSpaceMembershipMaintenancePort>>,
    pending: AtomicBool,
}

impl DeferredMaintenanceWake {
    fn new() -> Self {
        Self {
            target: OnceLock::new(),
            pending: AtomicBool::new(false),
        }
    }

    fn bind(&self, target: Arc<dyn crate::space::membership::WakeSpaceMembershipMaintenancePort>) {
        if self.target.set(target).is_ok() && self.pending.swap(false, Ordering::AcqRel) {
            if let Some(target) = self.target.get() {
                target.wake();
            }
        }
    }
}

impl crate::space::membership::WakeSpaceMembershipMaintenancePort for DeferredMaintenanceWake {
    fn wake(&self) {
        if let Some(target) = self.target.get() {
            target.wake();
        } else {
            self.pending.store(true, Ordering::Release);
        }
    }
}

pub struct SpaceApplicationDeps {
    pub load_membership_ledger: Arc<dyn LoadMembershipLedgerPort>,
    pub commit_membership_ledger: Arc<dyn CommitMembershipLedgerPort>,
    pub historical_membership_signatures: Arc<dyn HistoricalMembershipSignatureVerifier>,
    pub current_member_signatures: Arc<dyn CurrentMemberSignaturePort>,
    pub membership_identity: Arc<dyn CurrentMembershipIdentityPort>,
    pub membership_announcement: Arc<dyn CurrentMembershipAnnouncementPort>,
    pub device_identity: Arc<dyn uc_core::ports::DeviceIdentityPort>,
    pub group_bootstrap: Arc<dyn GroupBootstrapPort>,
    pub clock: Arc<dyn uc_core::ports::ClockPort>,
    pub settings: Arc<dyn uc_core::ports::SettingsPort>,
    pub prepare_joiner_invitation: Arc<dyn PrepareJoinerInvitationPort>,
    pub resolve_joiner_invitation: Arc<dyn ResolveJoinerInvitationPort>,
    pub joiner_start_material: Arc<dyn JoinerStartMaterialPort>,
    pub joiner_start_state: Arc<dyn JoinerStartStatePort>,
    pub current_join_admission_state: Arc<dyn CurrentJoinAdmissionStatePort>,
    pub prepare_joiner_cancellation: Arc<dyn PrepareJoinerCancellationPort>,
    pub pending_admission_recovery_state: Arc<dyn PendingAdmissionRecoveryStatePort>,
    pub space_admission_transport: Arc<dyn SpaceAdmissionTransportPort>,
    pub sponsor_admission_state: Arc<dyn SponsorAdmissionStatePort>,
    pub prepare_sponsor_candidate: Arc<dyn PrepareSponsorCandidatePort>,
    pub prepare_sponsor_commit: Arc<dyn PrepareSponsorCommitPort>,
    pub prepare_sponsor_complete: Arc<dyn PrepareSponsorCompletePort>,
    pub activate_sponsor_admission: Arc<dyn ActivateSponsorAdmissionPort>,
    pub prepare_sponsor_settled: Arc<dyn PrepareSponsorSettledPort>,
    pub prepare_joiner_candidate: Arc<dyn PrepareJoinerCandidatePort>,
    pub prepare_joiner_applied: Arc<dyn PrepareJoinerAppliedPort>,
    pub prepare_joiner_activation: Arc<dyn PrepareJoinerActivationPort>,
    pub joiner_activation_state: Arc<dyn JoinerActivationStatePort>,
    pub execute_joiner_activation: Arc<dyn ExecuteJoinerActivationPort>,
    pub device_trust_observations: Arc<dyn LoadDeviceTrustObservationsPort>,
    pub current_join_status: Arc<dyn LoadCurrentJoinStatusPort>,
    pub membership_history_transport: Arc<dyn MembershipHistoryExchangePort>,
    pub membership_branch_recovery_channel:
        Arc<dyn crate::space::membership::MembershipBranchRecoveryChannelPort>,
    pub membership_branch_recovery_recipient:
        Arc<dyn crate::space::membership::PrepareMembershipBranchRecoveryRecipientPort>,
    pub membership_branch_transition:
        Arc<dyn crate::space::membership::PrepareMembershipBranchTransitionPort>,
    pub membership_branch_recovery_material:
        Arc<dyn crate::space::membership::PrepareMembershipBranchRecoveryMaterialPort>,
    pub apply_membership_member_facts: Arc<dyn ApplyMembershipMemberFactsPort>,
    pub apply_membership_security: Arc<dyn ApplyMembershipSecurityPort>,
    pub activate_membership_effect: Arc<dyn ActivateMembershipEffectPort>,
    pub restricted_membership_delivery: Arc<dyn RestrictedMembershipDeliveryPort>,
    pub group_update_store: Arc<dyn GroupRevocationPort>,
    pub group_update_dispatch: Arc<dyn GroupUpdateDispatchPort>,
    pub cleanup_legacy_membership_data: Arc<dyn CleanupLegacyMembershipDataPort>,
    pub membership_network_activity: Arc<dyn MembershipNetworkActivityPort>,
}

pub(crate) struct SpaceApplication {
    ledger: Arc<MembershipLedger>,
    current_scope: Arc<dyn CurrentSpaceMemberScopePort>,
    query_device_trust: Arc<QueryDeviceTrustUseCase>,
    query_membership_admission: Arc<QueryMembershipAdmissionUseCase>,
    remove_space_member: Arc<RemoveSpaceMemberUseCase>,
    decide_device_trust_change: Arc<DecideDeviceTrustChangeUseCase>,
    resolve_membership_conflict: Arc<ResolveMembershipConflictUseCase>,
    issue_membership_branch_recovery: Arc<IssueMembershipBranchRecoveryUseCase>,
    space_admission: Arc<SpaceAdmissionProtocol>,
    membership_history_endpoint: Arc<MembershipHistoryAntiEntropy>,
    initialize_membership: Arc<InitializeSpaceMembershipUseCase>,
    membership_activity: crate::space::membership::SpaceMembershipMaintenanceActivity,
    prepared_runtime: Option<PreparedSpaceMembershipMaintenanceRuntime>,
    runtime: Option<SpaceMembershipMaintenanceRuntime>,
}

impl SpaceApplication {
    pub(crate) fn build(
        deps: SpaceApplicationDeps,
        peer_reachability_changed_events: broadcast::Receiver<PeerReachabilityChanged>,
        re_pairing: Arc<dyn crate::space::membership::ResolveRePairingPort>,
    ) -> Self {
        let historical_membership_signatures = Arc::clone(&deps.historical_membership_signatures);
        let branch_recovery_signatures = Arc::clone(&deps.current_member_signatures);
        let ledger = Arc::new(MembershipLedger::new(
            deps.load_membership_ledger,
            deps.commit_membership_ledger,
            deps.historical_membership_signatures,
        ));
        let query_device_trust = Arc::new(QueryDeviceTrustUseCase::new(
            Arc::clone(&ledger),
            deps.device_trust_observations,
            deps.current_join_status,
        ));
        let initialize_membership = Arc::new(InitializeSpaceMembershipUseCase::new(
            Arc::clone(&ledger),
            deps.membership_identity,
            deps.membership_announcement,
            Arc::clone(&deps.current_member_signatures),
            deps.device_identity,
            deps.group_bootstrap,
            Arc::clone(&deps.clock),
        ));
        let query_membership_admission =
            Arc::new(QueryMembershipAdmissionUseCase::new(Arc::clone(&ledger)));
        let current_scope: Arc<dyn CurrentSpaceMemberScopePort> = ledger.clone();
        let deferred_maintenance_wake = Arc::new(DeferredMaintenanceWake::new());
        let membership_history_endpoint = Arc::new(MembershipHistoryAntiEntropy::new(
            Arc::clone(&ledger),
            Arc::clone(&current_scope),
            deps.membership_history_transport,
            Arc::clone(&deps.clock),
            deferred_maintenance_wake.clone(),
        ));
        let joiner_admission = JoinerAdmissionService::new(
            deps.settings,
            deps.prepare_joiner_invitation,
            deps.resolve_joiner_invitation,
            deps.joiner_start_material,
            deps.joiner_start_state,
            deps.current_join_admission_state,
            deps.prepare_joiner_cancellation,
            deps.prepare_joiner_candidate,
            deps.prepare_joiner_applied,
            deps.prepare_joiner_activation,
            deps.joiner_activation_state,
            deps.execute_joiner_activation,
            deferred_maintenance_wake.clone(),
        );
        let sponsor_admission = SponsorAdmissionService::new(
            deps.sponsor_admission_state,
            deps.prepare_sponsor_candidate,
            deps.prepare_sponsor_commit,
            deps.prepare_sponsor_complete,
            deps.activate_sponsor_admission,
            deps.prepare_sponsor_settled,
        );
        let admission_recovery = AdmissionRecoveryService::new(
            deps.pending_admission_recovery_state,
            deps.space_admission_transport,
        );
        let space_admission = Arc::new(SpaceAdmissionProtocol::new(
            joiner_admission,
            sponsor_admission,
            admission_recovery,
        ));
        let membership_activation = Arc::new(RePairingAwareMembershipActivation::new(
            deps.activate_membership_effect,
            re_pairing,
        ));
        let recover_membership_effects = Arc::new(RecoverMembershipEffectsUseCase::new(
            Arc::clone(&ledger),
            deps.apply_membership_member_facts,
            deps.apply_membership_security,
            membership_activation,
        ));
        let deliver_restricted_membership = Arc::new(DeliverRestrictedMembershipUseCase::new(
            Arc::clone(&ledger),
            deps.restricted_membership_delivery,
        ));
        let deliver_group_updates = Arc::new(DeliverPendingGroupUpdatesUseCase::new(
            deps.group_update_store,
            deps.group_update_dispatch,
            Arc::clone(&deps.clock),
        ));
        let recover_membership_conflicts = Arc::new(RecoverMembershipConflictUseCase::new(
            Arc::clone(&ledger),
            deps.membership_branch_recovery_channel,
            deps.membership_branch_recovery_recipient,
            deps.membership_branch_transition,
            historical_membership_signatures,
            Arc::clone(&deps.clock),
        ));
        let issue_membership_branch_recovery = Arc::new(IssueMembershipBranchRecoveryUseCase::new(
            Arc::clone(&ledger),
            deps.membership_branch_recovery_material,
            branch_recovery_signatures,
            Arc::clone(&deps.clock),
        ));
        let maintain = Arc::new(MaintainSpaceMembershipUseCase::new(
            MaintainSpaceMembershipDeps {
                admissions: space_admission.clone(),
                effects: Arc::clone(&recover_membership_effects)
                    as Arc<dyn crate::space::membership::RecoverMembershipEffectsPort>,
                conflicts: recover_membership_conflicts,
                group_update_delivery: deliver_group_updates,
                restricted_delivery: deliver_restricted_membership,
                synchronization: membership_history_endpoint.clone(),
                cleanup: deps.cleanup_legacy_membership_data,
            },
        ));
        let prepared_runtime = SpaceMembershipMaintenanceRuntime::prepare(
            maintain,
            peer_reachability_changed_events,
            Duration::from_secs(30),
            deps.membership_network_activity,
        );
        let membership_activity = prepared_runtime.activity();
        deferred_maintenance_wake.bind(Arc::new(membership_activity.clone()));
        let activity = Arc::new(membership_activity.clone());
        let remove_space_member = Arc::new(RemoveSpaceMemberUseCase::new(
            Arc::clone(&ledger),
            Arc::clone(&deps.current_member_signatures),
            Arc::clone(&query_device_trust),
            recover_membership_effects.clone(),
            activity.clone(),
        ));
        let decide_device_trust_change = Arc::new(DecideDeviceTrustChangeUseCase::new(
            Arc::clone(&ledger),
            deps.current_member_signatures,
            Arc::clone(&query_device_trust),
            recover_membership_effects,
            activity,
        ));
        let resolve_membership_conflict = Arc::new(ResolveMembershipConflictUseCase::new(
            Arc::clone(&ledger),
            Arc::clone(&query_device_trust) as Arc<dyn QueryMembershipConflictStatusPort>,
        ));
        Self {
            ledger,
            current_scope,
            query_device_trust,
            query_membership_admission,
            remove_space_member,
            decide_device_trust_change,
            resolve_membership_conflict,
            issue_membership_branch_recovery,
            space_admission,
            membership_history_endpoint,
            initialize_membership,
            membership_activity,
            prepared_runtime: Some(prepared_runtime),
            runtime: None,
        }
    }

    pub(crate) fn start_runtime(&mut self) -> bool {
        let Some(prepared) = self.prepared_runtime.take() else {
            return false;
        };
        self.runtime = Some(SpaceMembershipMaintenanceRuntime::start_prepared(prepared));
        true
    }

    pub(crate) fn query_device_trust(&self) -> Arc<QueryDeviceTrustUseCase> {
        Arc::clone(&self.query_device_trust)
    }

    pub(crate) fn current_scope(&self) -> Arc<dyn CurrentSpaceMemberScopePort> {
        Arc::clone(&self.current_scope)
    }

    pub(crate) fn membership_session_activity(
        &self,
    ) -> Arc<dyn crate::space::lifecycle::MembershipSessionActivityPort> {
        Arc::new(self.membership_activity.clone())
    }

    pub(crate) fn membership_maintenance_wake(
        &self,
    ) -> Arc<dyn crate::space::membership::WakeSpaceMembershipMaintenancePort> {
        Arc::new(self.membership_activity.clone())
    }

    pub(crate) fn remove_space_member(&self) -> Arc<RemoveSpaceMemberUseCase> {
        Arc::clone(&self.remove_space_member)
    }

    pub(crate) fn query_membership_admission(&self) -> Arc<QueryMembershipAdmissionUseCase> {
        Arc::clone(&self.query_membership_admission)
    }

    pub(crate) fn decide_device_trust_change(&self) -> Arc<DecideDeviceTrustChangeUseCase> {
        Arc::clone(&self.decide_device_trust_change)
    }

    pub(crate) fn resolve_membership_conflict(&self) -> Arc<ResolveMembershipConflictUseCase> {
        Arc::clone(&self.resolve_membership_conflict)
    }

    pub(crate) fn membership_branch_recovery_endpoint(
        &self,
    ) -> Arc<dyn crate::space::membership::IssueMembershipBranchRecoveryPort> {
        self.issue_membership_branch_recovery.clone()
    }

    pub(crate) fn space_admission_for_cancel(&self) -> Arc<SpaceAdmissionProtocol> {
        Arc::clone(&self.space_admission)
    }

    pub(crate) fn space_admission(&self) -> Arc<SpaceAdmissionProtocol> {
        Arc::clone(&self.space_admission)
    }

    pub(crate) fn membership_history_endpoint(
        &self,
    ) -> Arc<dyn MembershipHistoryExchangeEndpointPort> {
        self.membership_history_endpoint.clone()
    }

    pub(crate) fn space_admission_endpoint(
        &self,
    ) -> Arc<dyn HandleAuthenticatedSpaceAdmissionMessagePort> {
        self.space_admission.clone()
    }

    pub(crate) fn initialize_membership(
        &self,
    ) -> Arc<dyn uc_core::membership::SpaceMembershipInitializerPort> {
        self.initialize_membership.clone()
    }

    pub(crate) fn membership_reset(
        &self,
    ) -> Arc<dyn crate::space::lifecycle::SpaceMembershipResetPort> {
        self.ledger.clone()
    }

    pub(crate) async fn shutdown(mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown().await;
        }
    }
}
