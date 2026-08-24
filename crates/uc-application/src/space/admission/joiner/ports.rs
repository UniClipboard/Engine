use async_trait::async_trait;

use uc_core::ids::DeviceId;
use uc_core::membership::MembershipCredential;
use uc_core::ports::space::SpaceAccessError;
use uc_core::space_access::PreparedGroupJoin;

#[async_trait]
pub trait GroupAdmissionPort: Send + Sync {
    async fn prepare_group_join(
        &self,
        device_id: &DeviceId,
    ) -> Result<PreparedGroupJoin, SpaceAccessError>;

    async fn prepared_join_membership_credential(
        &self,
        _pending: &PreparedGroupJoin,
    ) -> Result<MembershipCredential, SpaceAccessError> {
        Err(SpaceAccessError::Internal(
            "prepared join credential is unavailable".to_owned(),
        ))
    }

    async fn sign_prepared_join_payload(
        &self,
        _pending: &PreparedGroupJoin,
        _payload: &[u8],
    ) -> Result<Vec<u8>, SpaceAccessError> {
        Err(SpaceAccessError::Internal(
            "prepared join signing is unavailable".to_owned(),
        ))
    }
}
