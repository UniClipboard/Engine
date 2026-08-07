use std::sync::Arc;

use crate::membership::{
    MembershipConvergence, MembershipConvergenceActivity, MembershipConvergenceError,
    MembershipConvergenceRuntime, MembershipConvergenceStatus, SharedDeviceRefreshStarted,
    SharedDeviceRefreshStatus,
};

use super::{MembershipConnectivityRuntime, SpaceFacade};

#[derive(Debug, thiserror::Error)]
pub enum MembershipConvergenceFacadeError {
    #[error("space application runtime is unavailable")]
    Unavailable,
    #[error(transparent)]
    Query(#[from] MembershipConvergenceError),
}

#[derive(Clone)]
pub struct SpaceApplicationHandle {
    membership: Arc<MembershipConvergence>,
    activity: MembershipConvergenceActivity,
}

impl SpaceApplicationHandle {
    pub fn membership_activity(&self) -> MembershipConvergenceActivity {
        self.activity.clone()
    }

    pub async fn membership_convergence(
        &self,
    ) -> Result<MembershipConvergenceStatus, MembershipConvergenceError> {
        self.membership.current_convergence_status().await
    }

    pub async fn start_shared_device_refresh(
        &self,
    ) -> Result<SharedDeviceRefreshStarted, MembershipConvergenceError> {
        self.membership.start_shared_device_refresh().await
    }

    pub async fn shared_device_refresh_status(
        &self,
        request_id: &str,
    ) -> Option<SharedDeviceRefreshStatus> {
        self.membership
            .shared_device_refresh_status(request_id)
            .await
    }

    pub fn subscribe_shared_device_refresh(
        &self,
    ) -> tokio::sync::broadcast::Receiver<SharedDeviceRefreshStatus> {
        self.membership.subscribe_shared_device_refresh()
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
        membership: Arc<MembershipConvergence>,
        membership_runtime: MembershipConvergenceRuntime,
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
