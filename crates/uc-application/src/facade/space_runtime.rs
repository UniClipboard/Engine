use std::sync::Arc;

use crate::membership::{
    MembershipConvergence, MembershipConvergenceActivity, MembershipConvergenceRuntime,
};

use super::{MembershipConnectivityRuntime, SpaceFacade};

#[derive(Clone)]
pub struct SpaceApplicationHandle {
    activity: MembershipConvergenceActivity,
}

impl SpaceApplicationHandle {
    pub fn membership_activity(&self) -> MembershipConvergenceActivity {
        self.activity.clone()
    }
}

pub struct SpaceApplicationRuntime {
    handle: SpaceApplicationHandle,
    membership_runtime: MembershipConvergenceRuntime,
    connectivity_runtime: MembershipConnectivityRuntime,
    setup: Arc<SpaceFacade>,
}

impl SpaceApplicationRuntime {
    pub fn new(
        _membership: Arc<MembershipConvergence>,
        membership_runtime: MembershipConvergenceRuntime,
        connectivity_runtime: MembershipConnectivityRuntime,
        setup: Arc<SpaceFacade>,
    ) -> Self {
        let handle = SpaceApplicationHandle {
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
