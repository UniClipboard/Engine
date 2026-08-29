use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use tokio::sync::broadcast;
use uc_core::membership::{
    CurrentMembershipAnnouncementPort, CurrentMembershipIdentityPort, GroupBootstrapPort,
    HistoricalMembershipSignatureVerifier, MembershipHistoryExchangeEndpointPort,
    MembershipHistoryExchangePort,
};
use uc_core::ports::PresenceEvent;

use crate::space::admission::{
    AdmissionRecoveryService, CancelSpaceJoinUseCase, CompletePendingSpaceTransitionUseCase,
    ExecuteJoinerActivationPort, HandleAuthenticatedSpaceAdmissionMessagePort,
    JoinerActivationStatePort, JoinerAdmissionService, JoinerStartMaterialPort,
    JoinerStartStatePort, PendingAdmissionRecoveryStatePort, PrepareJoinerActivationPort,
    PrepareJoinerAppliedPort, PrepareJoinerCandidatePort, PrepareJoinerInvitationPort,
    PrepareSponsorCandidatePort, PrepareSponsorCommitPort, PrepareSponsorCompletePort,
    PrepareSponsorSettledPort, QueryPendingSpaceTransitionUseCase, ResolveJoinerInvitationPort,
    SpaceAdmissionProtocol, SpaceAdmissionTransportPort, SponsorAdmissionService,
    SponsorAdmissionStatePort,
};
use crate::space::membership::CurrentMemberSignaturePort;
use crate::space::membership::DecideDeviceTrustChangeUseCase;
use crate::space::membership::HandleMembershipHistoryMessageUseCase;
use crate::space::membership::QueryMembershipAdmissionUseCase;
use crate::space::membership::RemoveSpaceMemberUseCase;
use crate::space::membership::SynchronizeMembershipHistoryUseCase;
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
use crate::space::membership::{LoadDeviceTrustObservationsPort, QueryDeviceTrustUseCase};

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
    pub pending_admission_recovery_state: Arc<dyn PendingAdmissionRecoveryStatePort>,
    pub space_admission_transport: Arc<dyn SpaceAdmissionTransportPort>,
    pub sponsor_admission_state: Arc<dyn SponsorAdmissionStatePort>,
    pub prepare_sponsor_candidate: Arc<dyn PrepareSponsorCandidatePort>,
    pub prepare_sponsor_commit: Arc<dyn PrepareSponsorCommitPort>,
    pub prepare_sponsor_complete: Arc<dyn PrepareSponsorCompletePort>,
    pub prepare_sponsor_settled: Arc<dyn PrepareSponsorSettledPort>,
    pub prepare_joiner_candidate: Arc<dyn PrepareJoinerCandidatePort>,
    pub prepare_joiner_applied: Arc<dyn PrepareJoinerAppliedPort>,
    pub prepare_joiner_activation: Arc<dyn PrepareJoinerActivationPort>,
    pub joiner_activation_state: Arc<dyn JoinerActivationStatePort>,
    pub execute_joiner_activation: Arc<dyn ExecuteJoinerActivationPort>,
    pub device_trust_observations: Arc<dyn LoadDeviceTrustObservationsPort>,
    pub membership_history_transport: Arc<dyn MembershipHistoryExchangePort>,
    pub admission_outbox_delivery: Arc<dyn crate::deps::AdmissionOutboxDeliveryPort>,
    pub admission_space_transition: Arc<dyn crate::deps::AdmissionSpaceTransitionPort>,
    pub apply_membership_member_facts: Arc<dyn ApplyMembershipMemberFactsPort>,
    pub apply_membership_security: Arc<dyn ApplyMembershipSecurityPort>,
    pub activate_membership_effect: Arc<dyn ActivateMembershipEffectPort>,
    pub restricted_membership_delivery: Arc<dyn RestrictedMembershipDeliveryPort>,
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
    cancel_space_join: Arc<CancelSpaceJoinUseCase>,
    space_admission: Arc<SpaceAdmissionProtocol>,
    complete_pending_space_transition: Arc<CompletePendingSpaceTransitionUseCase>,
    query_pending_space_transition: Arc<QueryPendingSpaceTransitionUseCase>,
    membership_history_endpoint: Arc<HandleMembershipHistoryMessageUseCase>,
    initialize_membership: Arc<InitializeSpaceMembershipUseCase>,
    membership_activity: crate::space::membership::SpaceMembershipMaintenanceActivity,
    runtime: Option<SpaceMembershipMaintenanceRuntime>,
}

impl SpaceApplication {
    pub(crate) fn start(
        deps: SpaceApplicationDeps,
        presence_events: broadcast::Receiver<PresenceEvent>,
        re_pairing: Arc<dyn crate::space::membership::ResolveRePairingPort>,
    ) -> Self {
        let ledger = Arc::new(MembershipLedger::new(
            deps.load_membership_ledger,
            deps.commit_membership_ledger,
            deps.historical_membership_signatures,
        ));
        let query_device_trust = Arc::new(QueryDeviceTrustUseCase::new(
            Arc::clone(&ledger),
            deps.device_trust_observations,
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
        let synchronize_membership = Arc::new(SynchronizeMembershipHistoryUseCase::new(
            Arc::clone(&ledger),
            Arc::clone(&current_scope),
            deps.membership_history_transport,
        ));
        let deferred_maintenance_wake = Arc::new(DeferredMaintenanceWake::new());
        let joiner_admission = JoinerAdmissionService::new(
            deps.settings,
            deps.prepare_joiner_invitation,
            deps.resolve_joiner_invitation,
            deps.joiner_start_material,
            deps.joiner_start_state,
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
        let maintain = Arc::new(MaintainSpaceMembershipUseCase::new(
            MaintainSpaceMembershipDeps {
                admissions: space_admission.clone(),
                effects: Arc::clone(&recover_membership_effects)
                    as Arc<dyn crate::space::membership::RecoverMembershipEffectsPort>,
                restricted_delivery: deliver_restricted_membership,
                synchronization: synchronize_membership,
                cleanup: deps.cleanup_legacy_membership_data,
            },
        ));
        let runtime = SpaceMembershipMaintenanceRuntime::start(
            maintain,
            presence_events,
            Duration::from_secs(30),
            deps.membership_network_activity,
        );
        let membership_activity = runtime.activity();
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
        let cancel_space_join = Arc::new(CancelSpaceJoinUseCase::new(
            Arc::clone(&ledger),
            Arc::new(membership_activity.clone()),
        ));
        let complete_pending_space_transition =
            Arc::new(CompletePendingSpaceTransitionUseCase::new(
                Arc::clone(&ledger),
                deps.admission_space_transition,
            ));
        let query_pending_space_transition =
            Arc::new(QueryPendingSpaceTransitionUseCase::new(Arc::clone(&ledger)));
        let membership_history_endpoint = Arc::new(HandleMembershipHistoryMessageUseCase::new(
            Arc::clone(&ledger),
        ));
        Self {
            ledger,
            current_scope,
            query_device_trust,
            query_membership_admission,
            remove_space_member,
            decide_device_trust_change,
            cancel_space_join,
            space_admission,
            complete_pending_space_transition,
            query_pending_space_transition,
            membership_history_endpoint,
            initialize_membership,
            membership_activity,
            runtime: Some(runtime),
        }
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

    pub(crate) fn cancel_space_join(&self) -> Arc<CancelSpaceJoinUseCase> {
        Arc::clone(&self.cancel_space_join)
    }

    pub(crate) fn space_admission(&self) -> Arc<SpaceAdmissionProtocol> {
        Arc::clone(&self.space_admission)
    }

    pub(crate) fn complete_pending_space_transition(
        &self,
    ) -> Arc<CompletePendingSpaceTransitionUseCase> {
        Arc::clone(&self.complete_pending_space_transition)
    }

    pub(crate) fn query_pending_space_transition(&self) -> Arc<QueryPendingSpaceTransitionUseCase> {
        Arc::clone(&self.query_pending_space_transition)
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
