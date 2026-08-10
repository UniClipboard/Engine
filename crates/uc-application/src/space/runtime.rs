use std::sync::Arc;

use tokio::sync::broadcast;

use crate::facade::space_setup::SpaceFacade;
use crate::space::convergence::assembly::SpaceConvergenceAssembly;
use crate::space::convergence::discovery::{
    MembershipConvergenceActivity, MembershipConvergenceRuntime,
};
use crate::space::convergence::legacy_upgrade::AutomaticLegacyUpgradeRuntime;
use crate::space::convergence::membership_connectivity::{
    start_membership_connectivity, MembershipConnectivityDeps, MembershipConnectivityRuntime,
};
use crate::space::convergence::WorkspaceConvergenceRuntime;
use uc_core::ports::PresenceEvent;

#[derive(Clone)]
pub struct SpaceApplicationHandle {
    activity: MembershipConvergenceActivity,
}

impl SpaceApplicationHandle {
    pub fn membership_activity(&self) -> MembershipConvergenceActivity {
        self.activity.clone()
    }
}

/// Runtimes for every continuous space process, owned and shut down by
/// [`SpaceApplicationRuntime`] so callers issue one lifecycle action instead
/// of starting and stopping each owner themselves (ADR-018).
pub struct SpaceApplicationRuntime {
    handle: SpaceApplicationHandle,
    membership_runtime: MembershipConvergenceRuntime,
    connectivity_runtime: MembershipConnectivityRuntime,
    convergence_runtime: WorkspaceConvergenceRuntime,
    legacy_upgrade_runtime: AutomaticLegacyUpgradeRuntime,
    setup: Arc<SpaceFacade>,
}

impl SpaceApplicationRuntime {
    /// Start every space runtime from one assembly. `presence_events` is
    /// subscribed by each runtime; the assembly owns the convergence,
    /// membership gossip, connectivity and legacy-upgrade owners.
    pub fn start(
        assembly: Arc<SpaceConvergenceAssembly>,
        connectivity: MembershipConnectivityDeps,
        presence_events: broadcast::Receiver<PresenceEvent>,
        setup: Arc<SpaceFacade>,
    ) -> Self {
        let convergence_runtime = assembly.start_workspace_runtime(presence_events.resubscribe());
        let membership_runtime = assembly.start_membership_runtime(presence_events.resubscribe());
        let connectivity_runtime =
            start_membership_connectivity(connectivity, presence_events.resubscribe());
        let legacy_upgrade_runtime = assembly.start_legacy_upgrade_runtime(presence_events);
        let handle = SpaceApplicationHandle {
            activity: membership_runtime.activity(),
        };
        Self {
            handle,
            membership_runtime,
            connectivity_runtime,
            convergence_runtime,
            legacy_upgrade_runtime,
            setup,
        }
    }

    pub fn handle(&self) -> SpaceApplicationHandle {
        self.handle.clone()
    }

    pub async fn shutdown(self) {
        self.connectivity_runtime.shutdown().await;
        self.membership_runtime.shutdown().await;
        self.convergence_runtime.shutdown().await;
        self.legacy_upgrade_runtime.shutdown().await;
        self.setup.on_shutdown().await;
    }
}
