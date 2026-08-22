use std::sync::Arc;

use tokio::sync::broadcast;

use crate::facade::space_setup::SpaceFacade;
use crate::space::assembly::SpaceModules;
use crate::space::connectivity::membership::{
    start_membership_connectivity, MembershipConnectivityActivity, MembershipConnectivityDeps,
    MembershipConnectivityRuntime,
};
use crate::space::workspace_membership::discovery::{
    MembershipConvergenceActivity, MembershipConvergenceRuntime,
};
use crate::space::workspace_membership::{WorkspaceMembershipActivity, WorkspaceMembershipRuntime};
use uc_core::ports::PresenceEvent;

#[derive(Clone)]
pub struct SpaceApplicationHandle {
    activity: SpaceMembershipActivity,
}

impl SpaceApplicationHandle {
    pub fn membership_activity(&self) -> SpaceMembershipActivity {
        self.activity.clone()
    }
}

#[derive(Clone)]
pub struct SpaceMembershipActivity {
    membership: MembershipConvergenceActivity,
    workspace: WorkspaceMembershipActivity,
    connectivity: MembershipConnectivityActivity,
}

#[async_trait::async_trait]
impl crate::space::workspace_membership::discovery::MembershipConvergenceActivityPort
    for SpaceMembershipActivity
{
    async fn pause(&self) -> Result<(), String> {
        self.workspace.pause().await?;
        self.membership
            .pause()
            .await
            .map_err(|error| error.to_string())
    }

    async fn resume(&self) -> Result<(), String> {
        self.connectivity.resume();
        self.membership
            .resume()
            .await
            .map_err(|error| error.to_string())?;
        self.workspace.resume().await
    }
}

/// Runtimes for every continuous space process, owned and shut down by
/// [`SpaceApplicationRuntime`] so callers issue one lifecycle action instead
/// of starting and stopping each owner themselves (ADR-018).
pub struct SpaceApplicationRuntime {
    handle: SpaceApplicationHandle,
    membership_runtime: MembershipConvergenceRuntime,
    connectivity_runtime: MembershipConnectivityRuntime,
    convergence_runtime: WorkspaceMembershipRuntime,
    setup: Arc<SpaceFacade>,
}

impl SpaceApplicationRuntime {
    /// Start every space runtime from one assembly. `presence_events` is
    /// subscribed by each runtime; the assembly owns the convergence,
    /// membership gossip, connectivity and legacy-upgrade owners.
    pub fn start(
        assembly: Arc<SpaceModules>,
        connectivity: MembershipConnectivityDeps,
        presence_events: broadcast::Receiver<PresenceEvent>,
        setup: Arc<SpaceFacade>,
    ) -> Self {
        let convergence_runtime = assembly.start_workspace_runtime(presence_events.resubscribe());
        let membership_runtime = assembly.start_membership_runtime(presence_events.resubscribe());
        let connectivity_runtime =
            start_membership_connectivity(connectivity, presence_events.resubscribe());
        let handle = SpaceApplicationHandle {
            activity: SpaceMembershipActivity {
                membership: membership_runtime.activity(),
                workspace: convergence_runtime.activity(),
                connectivity: connectivity_runtime.activity(),
            },
        };
        Self {
            handle,
            membership_runtime,
            connectivity_runtime,
            convergence_runtime,
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
        self.setup.on_shutdown().await;
    }
}
