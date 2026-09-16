use std::sync::Arc;

use super::{DeviceGroupChoicesView, QueryDeviceGroupChoicesError};
use crate::space::membership::{
    MembershipLedger, QueryDeviceTrustUseCase, ResolveMembershipConflictUseCase,
};

pub(crate) struct QueryDeviceGroupChoicesUseCase {
    ledger: Arc<MembershipLedger>,
    trust: Arc<QueryDeviceTrustUseCase>,
    conflicts: Arc<ResolveMembershipConflictUseCase>,
}

impl QueryDeviceGroupChoicesUseCase {
    pub(crate) fn new(
        ledger: Arc<MembershipLedger>,
        trust: Arc<QueryDeviceTrustUseCase>,
        conflicts: Arc<ResolveMembershipConflictUseCase>,
    ) -> Self {
        Self {
            ledger,
            trust,
            conflicts,
        }
    }

    pub(crate) async fn execute(
        &self,
    ) -> Result<DeviceGroupChoicesView, QueryDeviceGroupChoicesError> {
        let snapshot = self.ledger.load_verified().await.map_err(|source| {
            QueryDeviceGroupChoicesError::DeviceTrust {
                source: source.into(),
            }
        })?;
        let device_trust = self
            .trust
            .query_snapshot(&snapshot)
            .await
            .map_err(|source| QueryDeviceGroupChoicesError::DeviceTrust { source })?;
        let conflicts = self
            .conflicts
            .query_snapshot(&snapshot)
            .map_err(|source| QueryDeviceGroupChoicesError::MembershipConflict { source })?;
        Ok(DeviceGroupChoicesView {
            revision: device_trust.revision,
            device_trust,
            conflicts,
        })
    }
}
