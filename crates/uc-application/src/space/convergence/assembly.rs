//! Space convergence assembly — the single application-layer construction
//! point for the workspace convergence owner, the membership gossip, the
//! group-update delivery and the automatic legacy upgrade.
//!
//! Engine callers never construct these owners directly (ADR-018): they fill
//! [`SpaceConvergenceDeps`] with uc-core ports and already-built
//! infrastructure adapters, call [`SpaceConvergenceAssembly::new`], and then
//! install the exposed uc-core endpoint ports on the network node before
//! starting the space runtimes through the facade lifecycle actions.

use std::sync::Arc;

use tokio::sync::broadcast;

use uc_core::membership::{
    ContentExchangeGatePort, CurrentWorkspacePeerScopePort, GroupRevocationPort,
    GroupUpdateDispatchPort, LegacyUpgradeEndpointPort, MembershipAttestationEndpointPort,
    MembershipGossipEndpointPort, MembershipHistoryExchangeEndpointPort,
};
use uc_core::ports::PresenceEvent;

use crate::space::convergence::discovery::{
    build_membership_convergence, MembershipConvergence, MembershipConvergenceDeps,
    MembershipConvergenceRuntime,
};
use crate::space::convergence::group_update_delivery::{
    GroupUpdateDelivery, GroupUpdateDeliveryPort,
};
use crate::space::convergence::legacy_upgrade::{
    AutomaticLegacyUpgrade, AutomaticLegacyUpgradeDeps, AutomaticLegacyUpgradeRuntime,
};
use crate::space::convergence::WorkspaceConvergence;

/// Passive dependency bundle for the space convergence owners. All fields
/// are uc-core ports or already-built infrastructure adapters; the owners
/// themselves are constructed inside [`SpaceConvergenceAssembly`].
pub struct SpaceConvergenceDeps {
    pub workspace: crate::space::convergence::WorkspaceConvergenceDeps,
    pub membership: MembershipConvergenceDeps,
    pub group_revocation: Arc<dyn GroupRevocationPort>,
    pub group_update_dispatch: Arc<dyn GroupUpdateDispatchPort>,
    pub legacy_upgrade: AutomaticLegacyUpgradeDeps,
}

/// The assembled space convergence owners. Internal fields stay
/// `pub(crate)`; external code only sees uc-core endpoint ports and the
/// lifecycle actions exposed below.
pub struct SpaceConvergenceAssembly {
    pub(crate) workspace: Arc<WorkspaceConvergence>,
    pub(crate) membership: Arc<MembershipConvergence>,
    pub(crate) group_update_delivery: Arc<dyn GroupUpdateDeliveryPort>,
    pub(crate) legacy_upgrade: Arc<AutomaticLegacyUpgrade>,
}

impl SpaceConvergenceAssembly {
    /// Construct the workspace convergence owner, the membership gossip, the
    /// group-update delivery and the automatic legacy upgrade, and wire the
    /// delivery into the gossip.
    ///
    /// Must run before the network node spawns: the endpoint ports returned
    /// by the accessor methods are installed on the iroh builder.
    pub fn new(deps: SpaceConvergenceDeps) -> Self {
        let SpaceConvergenceDeps {
            workspace,
            membership,
            group_revocation,
            group_update_dispatch,
            legacy_upgrade,
        } = deps;
        let workspace = WorkspaceConvergence::new(workspace);
        let membership = build_membership_convergence(membership);
        let group_update_delivery: Arc<dyn GroupUpdateDeliveryPort> = Arc::new(
            GroupUpdateDelivery::new(group_revocation, group_update_dispatch),
        );
        membership.install_group_update_delivery(Arc::clone(&group_update_delivery));
        let legacy_upgrade = Arc::new(
            AutomaticLegacyUpgrade::new(legacy_upgrade).with_convergence(Arc::clone(&workspace)),
        );
        Self {
            workspace,
            membership,
            group_update_delivery,
            legacy_upgrade,
        }
    }

    /// Member-history endpoint installed on the authenticated member channel.
    pub fn membership_history_exchange(&self) -> Arc<dyn MembershipHistoryExchangeEndpointPort> {
        Arc::clone(&self.workspace) as Arc<dyn MembershipHistoryExchangeEndpointPort>
    }

    /// Removal gate used by clipboard / keepalive callers to self-filter
    /// removed devices.
    pub fn removal_gate(&self) -> Arc<dyn ContentExchangeGatePort> {
        Arc::clone(&self.workspace) as Arc<dyn ContentExchangeGatePort>
    }

    pub fn current_peer_scope(&self) -> Arc<dyn CurrentWorkspacePeerScopePort> {
        Arc::clone(&self.workspace) as Arc<dyn CurrentWorkspacePeerScopePort>
    }

    /// Membership attestation endpoint installed on the shared node.
    pub fn membership_attestation_endpoint(&self) -> Arc<dyn MembershipAttestationEndpointPort> {
        Arc::clone(&self.membership) as Arc<dyn MembershipAttestationEndpointPort>
    }

    /// Membership gossip endpoint installed on the shared node.
    pub fn membership_gossip_endpoint(&self) -> Arc<dyn MembershipGossipEndpointPort> {
        Arc::clone(&self.membership) as Arc<dyn MembershipGossipEndpointPort>
    }

    /// Legacy-upgrade endpoint installed on the shared node.
    pub fn legacy_upgrade_endpoint(&self) -> Arc<dyn LegacyUpgradeEndpointPort> {
        Arc::clone(&self.legacy_upgrade) as Arc<dyn LegacyUpgradeEndpointPort>
    }

    /// Group-update delivery consumed by the sponsor handshake.
    pub fn group_update_delivery(&self) -> Arc<dyn GroupUpdateDeliveryPort> {
        Arc::clone(&self.group_update_delivery)
    }

    /// Start the event-driven workspace convergence runtime.
    pub fn start_workspace_runtime(
        &self,
        presence_events: broadcast::Receiver<PresenceEvent>,
    ) -> crate::space::convergence::WorkspaceConvergenceRuntime {
        self.workspace.clone().start(presence_events)
    }

    /// Start the membership gossip runtime.
    pub fn start_membership_runtime(
        &self,
        presence_events: broadcast::Receiver<PresenceEvent>,
    ) -> MembershipConvergenceRuntime {
        self.membership.clone().start(presence_events)
    }

    /// Start the automatic legacy upgrade runtime.
    pub fn start_legacy_upgrade_runtime(
        &self,
        presence_events: broadcast::Receiver<PresenceEvent>,
    ) -> AutomaticLegacyUpgradeRuntime {
        self.legacy_upgrade.clone().start(presence_events)
    }
}
