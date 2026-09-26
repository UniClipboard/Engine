use std::sync::Arc;

use super::{DeviceGroupChoicesView, QueryDeviceGroupChoicesError};
use crate::space::membership::{
    MembershipOwner, QueryDeviceTrustUseCase, ResolveMembershipConflictUseCase,
};

pub(crate) struct QueryDeviceGroupChoicesUseCase {
    owner: Arc<MembershipOwner>,
    trust: Arc<QueryDeviceTrustUseCase>,
    conflicts: Arc<ResolveMembershipConflictUseCase>,
}

impl QueryDeviceGroupChoicesUseCase {
    pub(crate) fn new(
        owner: Arc<MembershipOwner>,
        trust: Arc<QueryDeviceTrustUseCase>,
        conflicts: Arc<ResolveMembershipConflictUseCase>,
    ) -> Self {
        Self {
            owner,
            trust,
            conflicts,
        }
    }

    pub(crate) async fn execute(
        &self,
    ) -> Result<DeviceGroupChoicesView, QueryDeviceGroupChoicesError> {
        let view = self.owner.load().await.map_err(|source| {
            QueryDeviceGroupChoicesError::DeviceTrust {
                source: source.into(),
            }
        })?;
        let device_trust = self
            .trust
            .query_view(&view)
            .await
            .map_err(|source| QueryDeviceGroupChoicesError::DeviceTrust { source })?;
        let conflicts = self
            .conflicts
            .query_view(&view)
            .map_err(|source| QueryDeviceGroupChoicesError::MembershipConflict { source })?;
        Ok(DeviceGroupChoicesView {
            revision: device_trust.revision,
            device_trust,
            conflicts,
        })
    }
}
