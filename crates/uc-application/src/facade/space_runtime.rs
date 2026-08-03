use std::sync::Arc;

use super::{
    MembershipConnectivityRuntime, MembershipConvergenceStatus, SpaceFacade, SpaceMembershipGossip,
    SpaceMembershipGossipActivity, SpaceMembershipGossipError, SpaceMembershipGossipRuntime,
};

#[derive(Debug, thiserror::Error)]
pub enum MembershipConvergenceFacadeError {
    #[error("space application runtime is unavailable")]
    Unavailable,
    #[error(transparent)]
    Query(#[from] SpaceMembershipGossipError),
}

#[derive(Clone)]
pub struct SpaceApplicationHandle {
    membership: Arc<SpaceMembershipGossip>,
    activity: SpaceMembershipGossipActivity,
}

impl SpaceApplicationHandle {
    pub fn membership_activity(&self) -> SpaceMembershipGossipActivity {
        self.activity.clone()
    }

    pub async fn membership_convergence(
        &self,
    ) -> Result<MembershipConvergenceStatus, SpaceMembershipGossipError> {
        self.membership.current_convergence_status().await
    }
}

pub struct SpaceApplicationRuntime {
    handle: SpaceApplicationHandle,
    membership_runtime: SpaceMembershipGossipRuntime,
    connectivity_runtime: MembershipConnectivityRuntime,
    setup: Arc<SpaceFacade>,
}

impl SpaceApplicationRuntime {
    pub fn new(
        membership: Arc<SpaceMembershipGossip>,
        membership_runtime: SpaceMembershipGossipRuntime,
        connectivity_runtime: MembershipConnectivityRuntime,
        setup: Arc<SpaceFacade>,
    ) -> Self {
        let handle = SpaceApplicationHandle {
            membership,
            activity: membership_runtime.activity(),
        };
        Self {
            handle,
            membership_runtime,
            connectivity_runtime,
            setup,
        }
    }

    pub fn handle(&self) -> SpaceApplicationHandle {
        self.handle.clone()
    }

    pub async fn shutdown(self) {
        self.connectivity_runtime.shutdown().await;
        self.membership_runtime.shutdown().await;
        self.setup.on_shutdown().await;
    }
}
