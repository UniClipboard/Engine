use async_trait::async_trait;

use uc_core::ids::DeviceId;
use uc_core::membership::{
    AdmissionCompletionRecoveryChallengeV1, AdmissionCompletionRecoveryHelloV1,
    AdmissionCompletionRecoveryResponseV1, AdmissionCompletionRecoveryTransportBindingV1,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionCompletionRecoveryTransportError {
    Offline,
    Transport,
    Rejected,
}

#[async_trait]
pub trait AdmissionCompletionRecoveryPort: Send + Sync {
    async fn request_completion_recovery_challenge(
        &self,
        helper: &DeviceId,
        route: &[u8],
        hello: AdmissionCompletionRecoveryHelloV1,
        joiner_last_message_id: [u8; 32],
    ) -> Result<AdmissionCompletionRecoveryChallengeV1, AdmissionCompletionRecoveryTransportError>;

    async fn submit_completion_recovery_response(
        &self,
        helper: &DeviceId,
        route: &[u8],
        hello: AdmissionCompletionRecoveryHelloV1,
        response: AdmissionCompletionRecoveryResponseV1,
    ) -> Result<uc_core::pairing::DurableAdmissionFrame, AdmissionCompletionRecoveryTransportError>;
}

#[async_trait]
pub trait AdmissionCompletionRecoveryEndpointPort: Send + Sync {
    async fn handle_completion_recovery_hello(
        &self,
        hello: AdmissionCompletionRecoveryHelloV1,
        transport_binding: AdmissionCompletionRecoveryTransportBindingV1,
        joiner_last_message_id: [u8; 32],
        helper_last_message_id: [u8; 32],
    ) -> Result<AdmissionCompletionRecoveryChallengeV1, AdmissionCompletionRecoveryTransportError>;

    async fn handle_completion_recovery_response(
        &self,
        hello: AdmissionCompletionRecoveryHelloV1,
        response: AdmissionCompletionRecoveryResponseV1,
        transport_binding: AdmissionCompletionRecoveryTransportBindingV1,
    ) -> Result<uc_core::pairing::DurableAdmissionFrame, AdmissionCompletionRecoveryTransportError>;
}
