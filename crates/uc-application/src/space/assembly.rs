//! Space convergence assembly — the single application-layer construction
//! point for the workspace convergence owner, the membership gossip, the
//! group-update delivery.
//!
//! Engine callers never construct these owners directly (ADR-018): they fill
//! [`SpaceModulesDeps`] with uc-core ports and already-built
//! infrastructure adapters, call [`SpaceModules::new`], and then
//! install the exposed uc-core endpoint ports on the network node before
//! starting the space runtimes through the facade lifecycle actions.

use std::sync::Arc;

use tokio::sync::broadcast;

use uc_core::membership::{
    AdmissionCompletionRecoveryEndpointPort, ContentExchangeGatePort,
    CurrentWorkspacePeerScopePort, GroupRevocationPort, GroupUpdateDispatchPort,
    MembershipAttestationEndpointPort, MembershipGossipEndpointPort,
    MembershipHistoryExchangeEndpointPort,
};
use uc_core::ports::PresenceEvent;

use crate::space::admission::SpaceAdmission;
use crate::space::workspace_membership::discovery::{
    build_membership_convergence, MembershipConvergence, MembershipConvergenceDeps,
    MembershipConvergenceRuntime,
};
use crate::space::workspace_membership::membership::group_update_delivery::{
    GroupUpdateDelivery, GroupUpdateDeliveryPort,
};
use crate::space::workspace_membership::WorkspaceMembership;

/// Passive dependency bundle for the space convergence owners. All fields
/// are uc-core ports or already-built infrastructure adapters; the owners
/// themselves are constructed inside [`SpaceModules`].
pub struct SpaceModulesDeps {
    pub workspace: crate::space::workspace_membership::WorkspaceMembershipDeps,
    pub membership: MembershipConvergenceDeps,
    pub group_revocation: Arc<dyn GroupRevocationPort>,
    pub group_update_dispatch: Arc<dyn GroupUpdateDispatchPort>,
}

/// The assembled space convergence owners. Internal fields stay
/// `pub(crate)`; external code only sees uc-core endpoint ports and the
/// lifecycle actions exposed below.
pub struct SpaceModules {
    pub(crate) membership_owner: Arc<WorkspaceMembership>,
    pub(crate) admission_owner: Arc<SpaceAdmission>,
    pub(crate) membership: Arc<MembershipConvergence>,
    pub(crate) group_update_delivery: Arc<dyn GroupUpdateDeliveryPort>,
}

impl SpaceModules {
    /// Construct the workspace convergence owner, the membership gossip, the
    /// group-update delivery and the automatic legacy upgrade, and wire the
    /// delivery into the gossip.
    ///
    /// Must run before the network node spawns: the endpoint ports returned
    /// by the accessor methods are installed on the iroh builder.
    pub fn new(deps: SpaceModulesDeps) -> Self {
        let SpaceModulesDeps {
            workspace,
            membership,
            group_revocation,
            group_update_dispatch,
        } = deps;
        let membership_owner = WorkspaceMembership::new(workspace);
        let admission_owner = SpaceAdmission::new(Arc::clone(&membership_owner));
        let membership = build_membership_convergence(membership);
        let group_update_delivery: Arc<dyn GroupUpdateDeliveryPort> = Arc::new(
            GroupUpdateDelivery::new(
                group_revocation,
                group_update_dispatch,
                Arc::clone(&membership_owner) as Arc<dyn crate::space::workspace_membership::membership::group_update_delivery::GroupUpdateRecipientPreparationPort>,
            ),
        );
        membership.install_group_update_delivery(Arc::clone(&group_update_delivery));
        Self {
            membership_owner,
            admission_owner,
            membership,
            group_update_delivery,
        }
    }

    /// Member-history endpoint installed on the authenticated member channel.
    pub fn membership_history_exchange(&self) -> Arc<dyn MembershipHistoryExchangeEndpointPort> {
        Arc::clone(&self.membership_owner) as Arc<dyn MembershipHistoryExchangeEndpointPort>
    }

    pub fn admission_completion_recovery(
        &self,
    ) -> Arc<dyn AdmissionCompletionRecoveryEndpointPort> {
        Arc::clone(&self.admission_owner) as Arc<dyn AdmissionCompletionRecoveryEndpointPort>
    }

    /// Removal gate used by clipboard / keepalive callers to self-filter
    /// removed devices.
    pub fn removal_gate(&self) -> Arc<dyn ContentExchangeGatePort> {
        Arc::clone(&self.membership_owner) as Arc<dyn ContentExchangeGatePort>
    }

    pub fn current_peer_scope(&self) -> Arc<dyn CurrentWorkspacePeerScopePort> {
        Arc::clone(&self.membership_owner) as Arc<dyn CurrentWorkspacePeerScopePort>
    }

    pub fn workspace_membership(
        &self,
    ) -> Arc<crate::space::workspace_membership::WorkspaceMembership> {
        Arc::clone(&self.membership_owner)
    }

    pub fn space_admission(&self) -> Arc<SpaceAdmission> {
        Arc::clone(&self.admission_owner)
    }

    /// Membership attestation endpoint installed on the shared node.
    pub fn membership_attestation_endpoint(&self) -> Arc<dyn MembershipAttestationEndpointPort> {
        Arc::clone(&self.membership) as Arc<dyn MembershipAttestationEndpointPort>
    }

    /// Membership gossip endpoint installed on the shared node.
    pub fn membership_gossip_endpoint(&self) -> Arc<dyn MembershipGossipEndpointPort> {
        Arc::clone(&self.membership) as Arc<dyn MembershipGossipEndpointPort>
    }

    /// Group-update delivery consumed by the sponsor handshake.
    pub fn group_update_delivery(&self) -> Arc<dyn GroupUpdateDeliveryPort> {
        Arc::clone(&self.group_update_delivery)
    }

    pub fn space_transition_recovery(&self) -> Arc<dyn crate::facade::SpaceTransitionRecoveryPort> {
        Arc::clone(&self.admission_owner) as Arc<dyn crate::facade::SpaceTransitionRecoveryPort>
    }

    /// Start the event-driven workspace convergence runtime.
    pub fn start_workspace_runtime(
        &self,
        presence_events: broadcast::Receiver<PresenceEvent>,
    ) -> crate::space::workspace_membership::WorkspaceMembershipRuntime {
        self.membership_owner.clone().start(presence_events)
    }

    /// Start the membership gossip runtime.
    pub fn start_membership_runtime(
        &self,
        presence_events: broadcast::Receiver<PresenceEvent>,
    ) -> MembershipConvergenceRuntime {
        self.membership.clone().start(presence_events)
    }
}
