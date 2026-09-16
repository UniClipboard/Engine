use async_trait::async_trait;
use uc_core::ids::DeviceId;
use uc_core::membership::{MemberInstanceId, MembershipCredential};

use super::CurrentMemberSignatureError;

#[async_trait]
pub trait CurrentMemberSignaturePort: Send + Sync {
    async fn current_member_epoch(&self) -> Result<u64, CurrentMemberSignatureError>;

    /// Historical-verification credential for the exact active local member.
    async fn current_membership_credential(
        &self,
        _device_id: &DeviceId,
    ) -> Result<MembershipCredential, CurrentMemberSignatureError> {
        Err(CurrentMemberSignatureError::Unavailable)
    }

    /// Stable local member instance derived from the active signing identity.
    async fn current_member_instance(
        &self,
        device_id: &DeviceId,
    ) -> Result<MemberInstanceId, CurrentMemberSignatureError>;

    /// Sign `payload` using the local identity from the current active member set.
    async fn sign_current_member_payload(
        &self,
        payload: &[u8],
    ) -> Result<Vec<u8>, CurrentMemberSignatureError>;

    /// Verify that `signature` was produced by `member` over `payload` using
    /// the member's identity from the current active member set.
    async fn verify_current_member_payload(
        &self,
        member: &DeviceId,
        payload: &[u8],
        signature: &[u8],
    ) -> Result<bool, CurrentMemberSignatureError>;

    /// Verify a payload against one exact member instance when the same
    /// stable device has more than one credential in the current group.
    async fn verify_member_instance_payload(
        &self,
        member: &DeviceId,
        _member_instance: MemberInstanceId,
        payload: &[u8],
        signature: &[u8],
    ) -> Result<bool, CurrentMemberSignatureError> {
        self.verify_current_member_payload(member, payload, signature)
            .await
    }
}
