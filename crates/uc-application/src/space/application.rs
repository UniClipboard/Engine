use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use tokio::sync::broadcast;
use uc_core::membership::{GroupBootstrapPort, MembershipHistoryExchangeEndpointPort};
use uc_core::ports::PeerReachabilityChanged;

use crate::deps::ApplicationDeps;
use crate::space::adapters::{
    SpaceAdmissionAdapters, SpaceMembershipAdapters, SpaceRuntimeAdapters,
};
use crate::space::admission::{
    AdmissionRecoveryService, HandleAuthenticatedSpaceAdmissionMessagePort, JoinerAdmissionService,
    SpaceAdmissionProtocol, SponsorAdmissionService,
};
use crate::space::membership::DecideDeviceTrustChangeUseCase;
use crate::space::membership::DeliverPendingGroupUpdatesUseCase;
use crate::space::membership::IssueMembershipBranchRecoveryUseCase;
use crate::space::membership::MembershipHistoryAntiEntropy;
use crate::space::membership::PreparedSpaceMembershipMaintenanceRuntime;
use crate::space::membership::QueryDeviceTrustUseCase;
use crate::space::membership::QueryMembershipAdmissionUseCase;
use crate::space::membership::QueryMembershipConflictStatusPort;
use crate::space::membership::QueryMembershipDiagnosticsUseCase;
use crate::space::membership::RecoverMembershipConflictUseCase;
use crate::space::membership::RemoveSpaceMemberUseCase;
use crate::space::membership::ResolveMembershipConflictUseCase;
use crate::space::membership::{
    CurrentSpaceMemberScopePort, DeliverRestrictedMembershipUseCase,
    InitializeSpaceMembershipUseCase, MembershipLedger, RePairingAwareMembershipActivation,
    RecoverMembershipEffectsUseCase,
};
use crate::space::membership::{
    MaintainSpaceMembershipDeps, MaintainSpaceMembershipUseCase, SpaceMembershipMaintenanceRuntime,
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

struct SpaceApplicationDeps {
    adapters: SpaceRuntimeAdapters,
    device_identity: Arc<dyn uc_core::ports::DeviceIdentityPort>,
    group_bootstrap: Arc<dyn GroupBootstrapPort>,
    clock: Arc<dyn uc_core::ports::ClockPort>,
    settings: Arc<dyn uc_core::ports::SettingsPort>,
}

impl SpaceApplicationDeps {
    fn from_application(application: &ApplicationDeps, adapters: SpaceRuntimeAdapters) -> Self {
        Self {
            adapters,
            device_identity: Arc::clone(&application.device.device_identity),
            group_bootstrap: Arc::clone(&application.security.space_access_ports.group_bootstrap),
            clock: Arc::clone(&application.system.clock),
            settings: Arc::clone(&application.settings),
        }
    }
}

pub(crate) struct SpaceApplication {
    ledger: Arc<MembershipLedger>,
    current_scope: Arc<dyn CurrentSpaceMemberScopePort>,
    query_device_trust: Arc<QueryDeviceTrustUseCase>,
    query_membership_admission: Arc<QueryMembershipAdmissionUseCase>,
    remove_space_member: Arc<RemoveSpaceMemberUseCase>,
    decide_device_trust_change: Arc<DecideDeviceTrustChangeUseCase>,
    resolve_membership_conflict: Arc<ResolveMembershipConflictUseCase>,
    query_membership_diagnostics: Arc<QueryMembershipDiagnosticsUseCase>,
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
        application: &ApplicationDeps,
        adapters: SpaceRuntimeAdapters,
        peer_reachability_changed_events: broadcast::Receiver<PeerReachabilityChanged>,
        re_pairing: Arc<dyn crate::space::membership::ResolveRePairingPort>,
    ) -> Self {
        Self::build_from_deps(
            SpaceApplicationDeps::from_application(application, adapters),
            peer_reachability_changed_events,
            re_pairing,
        )
    }

    #[cfg(test)]
    pub(crate) fn build_for_test(
        adapters: SpaceRuntimeAdapters,
        device_identity: Arc<dyn uc_core::ports::DeviceIdentityPort>,
        group_bootstrap: Arc<dyn GroupBootstrapPort>,
        clock: Arc<dyn uc_core::ports::ClockPort>,
        settings: Arc<dyn uc_core::ports::SettingsPort>,
        peer_reachability_changed_events: broadcast::Receiver<PeerReachabilityChanged>,
        re_pairing: Arc<dyn crate::space::membership::ResolveRePairingPort>,
    ) -> Self {
        Self::build_from_deps(
            SpaceApplicationDeps {
                adapters,
                device_identity,
                group_bootstrap,
                clock,
                settings,
            },
            peer_reachability_changed_events,
            re_pairing,
        )
    }

    fn build_from_deps(
        deps: SpaceApplicationDeps,
        peer_reachability_changed_events: broadcast::Receiver<PeerReachabilityChanged>,
        re_pairing: Arc<dyn crate::space::membership::ResolveRePairingPort>,
    ) -> Self {
        let SpaceApplicationDeps {
            adapters:
                SpaceRuntimeAdapters {
                    admission,
                    membership,
                },
            device_identity,
            group_bootstrap,
            clock,
            settings,
        } = deps;
        let SpaceAdmissionAdapters {
            prepare_joiner_invitation,
            resolve_joiner_invitation,
            joiner_start_material,
            joiner_start_state,
            current_join_admission_state,
            prepare_joiner_cancellation,
            pending_admission_recovery_state,
            space_admission_transport,
            sponsor_admission_state,
            prepare_sponsor_candidate,
            prepare_sponsor_commit,
            prepare_sponsor_complete,
            activate_sponsor_admission,
            prepare_sponsor_settled,
            prepare_joiner_candidate,
            prepare_joiner_applied,
            prepare_joiner_activation,
            joiner_activation_state,
            execute_joiner_activation,
            current_join_status,
        } = admission;
        let SpaceMembershipAdapters {
            load_membership_ledger,
            commit_membership_ledger,
            historical_membership_signatures,
            current_member_signatures,
            membership_identity,
            membership_announcement,
            device_trust_observations,
            membership_history_transport,
            membership_branch_recovery_channel,
            membership_branch_recovery_recipient,
            membership_branch_transition,
            membership_branch_transition_executor,
            membership_branch_recovery_material,
            apply_membership_member_facts,
            apply_membership_security,
            activate_membership_effect,
            restricted_membership_delivery,
            group_update_store,
            group_update_dispatch,
            cleanup_legacy_membership_data,
            membership_network_activity,
        } = membership;
        let branch_recovery_signatures = Arc::clone(&current_member_signatures);
        let diagnostics_signatures = Arc::clone(&current_member_signatures);
        let ledger = Arc::new(MembershipLedger::new(
            load_membership_ledger,
            commit_membership_ledger,
            Arc::clone(&historical_membership_signatures),
        ));
        let query_device_trust = Arc::new(QueryDeviceTrustUseCase::new(
            Arc::clone(&ledger),
            device_trust_observations,
            current_join_status,
        ));
        let initialize_membership = Arc::new(InitializeSpaceMembershipUseCase::new(
            Arc::clone(&ledger),
            membership_identity,
            membership_announcement,
            Arc::clone(&current_member_signatures),
            device_identity,
            group_bootstrap,
            Arc::clone(&clock),
        ));
        let query_membership_admission =
            Arc::new(QueryMembershipAdmissionUseCase::new(Arc::clone(&ledger)));
        let current_scope: Arc<dyn CurrentSpaceMemberScopePort> = ledger.clone();
        let deferred_maintenance_wake = Arc::new(DeferredMaintenanceWake::new());
        let membership_history_endpoint = Arc::new(MembershipHistoryAntiEntropy::new(
            Arc::clone(&ledger),
            Arc::clone(&current_scope),
            membership_history_transport,
            Arc::clone(&clock),
            deferred_maintenance_wake.clone(),
        ));
        let joiner_admission = JoinerAdmissionService::new(
            settings,
            prepare_joiner_invitation,
            resolve_joiner_invitation,
            joiner_start_material,
            joiner_start_state,
            current_join_admission_state,
            prepare_joiner_cancellation,
            prepare_joiner_candidate,
            prepare_joiner_applied,
            prepare_joiner_activation,
            joiner_activation_state,
            execute_joiner_activation,
            deferred_maintenance_wake.clone(),
        );
        let sponsor_admission = SponsorAdmissionService::new(
            sponsor_admission_state,
            prepare_sponsor_candidate,
            prepare_sponsor_commit,
            prepare_sponsor_complete,
            activate_sponsor_admission,
            prepare_sponsor_settled,
        );
        let admission_recovery = AdmissionRecoveryService::new(
            pending_admission_recovery_state,
            space_admission_transport,
        );
        let space_admission = Arc::new(SpaceAdmissionProtocol::new(
            joiner_admission,
            sponsor_admission,
            admission_recovery,
        ));
        let membership_activation = Arc::new(RePairingAwareMembershipActivation::new(
            activate_membership_effect,
            re_pairing,
        ));
        let recover_membership_effects = Arc::new(RecoverMembershipEffectsUseCase::new(
            Arc::clone(&ledger),
            apply_membership_member_facts,
            apply_membership_security,
            membership_activation,
        ));
        let deliver_restricted_membership = Arc::new(DeliverRestrictedMembershipUseCase::new(
            Arc::clone(&ledger),
            restricted_membership_delivery,
        ));
        let deliver_group_updates = Arc::new(DeliverPendingGroupUpdatesUseCase::new(
            group_update_store,
            group_update_dispatch,
            Arc::clone(&clock),
        ));
        let recover_membership_conflicts = Arc::new(RecoverMembershipConflictUseCase::new(
            Arc::clone(&ledger),
            membership_branch_recovery_channel,
            membership_branch_recovery_recipient,
            membership_branch_transition,
            membership_branch_transition_executor,
            historical_membership_signatures,
            Arc::clone(&clock),
        ));
        let issue_membership_branch_recovery = Arc::new(IssueMembershipBranchRecoveryUseCase::new(
            Arc::clone(&ledger),
            membership_branch_recovery_material,
            branch_recovery_signatures,
            Arc::clone(&clock),
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
                cleanup: cleanup_legacy_membership_data,
            },
        ));
        let prepared_runtime = SpaceMembershipMaintenanceRuntime::prepare(
            maintain,
            peer_reachability_changed_events,
            Duration::from_secs(30),
            membership_network_activity,
        );
        let membership_activity = prepared_runtime.activity();
        deferred_maintenance_wake.bind(Arc::new(membership_activity.clone()));
        let activity = Arc::new(membership_activity.clone());
        let remove_space_member = Arc::new(RemoveSpaceMemberUseCase::new(
            Arc::clone(&ledger),
            Arc::clone(&current_member_signatures),
            Arc::clone(&query_device_trust),
            recover_membership_effects.clone(),
            activity.clone(),
        ));
        let decide_device_trust_change = Arc::new(DecideDeviceTrustChangeUseCase::new(
            Arc::clone(&ledger),
            current_member_signatures,
            Arc::clone(&query_device_trust),
            recover_membership_effects,
            activity,
        ));
        let resolve_membership_conflict = Arc::new(ResolveMembershipConflictUseCase::new(
            Arc::clone(&ledger),
            Arc::clone(&query_device_trust) as Arc<dyn QueryMembershipConflictStatusPort>,
        ));
        let query_membership_diagnostics = Arc::new(QueryMembershipDiagnosticsUseCase::new(
            Arc::clone(&ledger),
            diagnostics_signatures,
        ));
        Self {
            ledger,
            current_scope,
            query_device_trust,
            query_membership_admission,
            remove_space_member,
            decide_device_trust_change,
            resolve_membership_conflict,
            query_membership_diagnostics,
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

    pub(crate) fn query_membership_diagnostics(&self) -> Arc<QueryMembershipDiagnosticsUseCase> {
        Arc::clone(&self.query_membership_diagnostics)
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
