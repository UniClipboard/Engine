mod error;
mod model;
mod ports;
mod use_case;

pub use error::QueryDeviceTrustError;
pub use model::{
    DeviceTrustDevice, DeviceTrustMembership, DeviceTrustObservation, DeviceTrustRelationship,
    DeviceTrustStatus, DeviceTrustSyncState, PendingDeviceTrustChange,
};
pub use ports::{LoadCurrentJoinStatusPort, LoadDeviceTrustObservationsPort};
pub(crate) use use_case::QueryDeviceTrustUseCase;

#[cfg(test)]
pub(crate) struct NoCurrentJoinStatus;

#[cfg(test)]
#[async_trait::async_trait]
impl LoadCurrentJoinStatusPort for NoCurrentJoinStatus {
    async fn load_current_join(
        &self,
    ) -> Result<Option<crate::space::admission::CurrentJoinStatus>, QueryDeviceTrustError> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests;
