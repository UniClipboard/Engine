use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast;
use uc_core::membership::{
    CurrentMembershipAnnouncementPort, CurrentMembershipIdentityPort, GroupBootstrapPort,
    HistoricalMembershipSignatureVerifier, MembershipHistoryExchangeEndpointPort,
    MembershipHistoryExchangePort,
};
use uc_core::ports::PresenceEvent;

use crate::space::admission::cancel_space_join::CancelSpaceJoinUseCase;
use crate::space::admission::complete_pending_space_transition::CompletePendingSpaceTransitionUseCase;
use crate::space::admission::handle_space_admission_message::{
    HandleSpaceAdmissionMessagePort, HandleSpaceAdmissionMessageUseCase,
    PrepareSpaceAdmissionMessagePort,
};
use crate::space::admission::invitation::InMemoryPairingInvitationHolder;
use crate::space::admission::join_space::{JoinSpaceUseCase, PrepareJoinSpacePort};
use crate::space::admission::query_pending_space_transition::QueryPendingSpaceTransitionUseCase;
use crate::space::admission::recover_space_admissions::RecoverSpaceAdmissionsUseCase;
use crate::space::current_member_signing::CurrentMemberSignaturePort;
use crate::space::decide_device_trust_change::DecideDeviceTrustChangeUseCase;
use crate::space::handle_membership_history_message::HandleMembershipHistoryMessageUseCase;
use crate::space::maintain_space_membership::{
    CleanupLegacyMembershipDataPort, MaintainSpaceMembershipDeps, MaintainSpaceMembershipUseCase,
    MembershipNetworkActivityPort, SpaceMembershipRuntime,
};
use crate::space::membership_ledger::{
    ActivateMembershipEffectPort, ApplyMembershipMemberFactsPort, ApplyMembershipSecurityPort,
    CommitMembershipLedgerPort, CurrentSpaceMemberScopePort, DeliverRestrictedMembershipUseCase,
    InitializeSpaceMembershipUseCase, LoadMembershipLedgerPort, MembershipLedger,
    RePairingAwareMembershipActivation, RecoverMembershipEffectsUseCase,
    RestrictedMembershipDeliveryPort,
};
use crate::space::query_device_trust::{LoadDeviceTrustObservationsPort, QueryDeviceTrustUseCase};
use crate::space::query_membership_admission::QueryMembershipAdmissionUseCase;
use crate::space::remove_space_member::RemoveSpaceMemberUseCase;
use crate::space::synchronize_membership_history::SynchronizeMembershipHistoryUseCase;

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
    pub prepare_join_space: Arc<dyn PrepareJoinSpacePort>,
    pub prepare_space_admission_message: Arc<dyn PrepareSpaceAdmissionMessagePort>,
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
    join_space: Arc<JoinSpaceUseCase>,
    complete_pending_space_transition: Arc<CompletePendingSpaceTransitionUseCase>,
    query_pending_space_transition: Arc<QueryPendingSpaceTransitionUseCase>,
    membership_history_endpoint: Arc<HandleMembershipHistoryMessageUseCase>,
    space_admission_endpoint: Arc<HandleSpaceAdmissionMessageUseCase>,
    initialize_membership: Arc<InitializeSpaceMembershipUseCase>,
    membership_activity: crate::space::maintain_space_membership::SpaceMembershipActivity,
    runtime: Option<SpaceMembershipRuntime>,
}

impl SpaceApplication {
    pub(crate) fn start(
        deps: SpaceApplicationDeps,
        presence_events: broadcast::Receiver<PresenceEvent>,
        invitations: Arc<InMemoryPairingInvitationHolder>,
        re_pairing: Arc<dyn crate::space::re_pairing::ResolveRePairingPort>,
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
        let recover_admissions = Arc::new(RecoverSpaceAdmissionsUseCase::new(
            Arc::clone(&ledger),
            deps.admission_outbox_delivery,
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
                admissions: recover_admissions,
                effects: Arc::clone(&recover_membership_effects)
                    as Arc<
                        dyn crate::space::maintain_space_membership::RecoverMembershipEffectsPort,
                    >,
                restricted_delivery: deliver_restricted_membership,
                synchronization: synchronize_membership,
                cleanup: deps.cleanup_legacy_membership_data,
            },
        ));
        let runtime = SpaceMembershipRuntime::start(
            maintain,
            presence_events,
            Duration::from_secs(30),
            deps.membership_network_activity,
        );
        let membership_activity = runtime.activity();
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
        let join_space = Arc::new(JoinSpaceUseCase::new(
            deps.settings,
            deps.prepare_join_space,
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
        let space_admission_endpoint = Arc::new(HandleSpaceAdmissionMessageUseCase::new(
            Arc::clone(&ledger),
            deps.prepare_space_admission_message,
            Arc::new(membership_activity.clone()),
            invitations,
            deps.clock,
        ));
        Self {
            ledger,
            current_scope,
            query_device_trust,
            query_membership_admission,
            remove_space_member,
            decide_device_trust_change,
            cancel_space_join,
            join_space,
            complete_pending_space_transition,
            query_pending_space_transition,
            membership_history_endpoint,
            space_admission_endpoint,
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
    ) -> Arc<dyn crate::space::session::MembershipSessionActivityPort> {
        Arc::new(self.membership_activity.clone())
    }

    pub(crate) fn membership_maintenance_wake(
        &self,
    ) -> Arc<dyn crate::space::remove_space_member::WakeSpaceMembershipMaintenancePort> {
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

    pub(crate) fn join_space(&self) -> Arc<JoinSpaceUseCase> {
        Arc::clone(&self.join_space)
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

    pub(crate) fn space_admission_endpoint(&self) -> Arc<dyn HandleSpaceAdmissionMessagePort> {
        self.space_admission_endpoint.clone()
    }

    pub(crate) fn initialize_membership(
        &self,
    ) -> Arc<dyn uc_core::membership::SpaceMembershipInitializerPort> {
        self.initialize_membership.clone()
    }

    pub(crate) fn membership_reset(
        &self,
    ) -> Arc<dyn crate::space::rebuild_space::SpaceMembershipResetPort> {
        self.ledger.clone()
    }

    pub(crate) async fn shutdown(mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown().await;
        }
    }
}
