use async_trait::async_trait;
use sha2::{Digest, Sha256};
use uc_core::membership::{
    AdmissionInboxRecord, AdmissionOutboxMessage, AdmissionOutboxPurpose, SpaceJoinRecordId,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvitationConsumeDeliveryResult {
    Consumed,
    NotFound,
    Conflict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionOutboxDeliveryResult {
    Deferred,
    Persisted(AdmissionInboxRecord),
    InvitationConsume(InvitationConsumeDeliveryResult),
    Rejected(AdmissionOutboxMessage),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionOutboxDeliveryRoute {
    Invitation(Vec<u8>),
    Continuation(Vec<u8>),
}

#[derive(Debug, thiserror::Error)]
pub enum AdmissionOutboxDeliveryError {
    #[error("admission delivery is unavailable")]
    Unavailable,
    #[error("admission delivery was rejected")]
    Rejected,
}

#[async_trait]
pub trait AdmissionOutboxDeliveryPort: Send + Sync {
    async fn deliver(
        &self,
        attempt_id: SpaceJoinRecordId,
        message: &AdmissionOutboxMessage,
        route: Option<&AdmissionOutboxDeliveryRoute>,
    ) -> Result<AdmissionOutboxDeliveryResult, AdmissionOutboxDeliveryError>;
}

pub(crate) fn acknowledgment(message: &AdmissionOutboxMessage) -> AdmissionInboxRecord {
    let payload_digest: [u8; 32] = Sha256::digest(&message.payload).into();
    let mut hasher = Sha256::new();
    hasher.update(b"uniclipboard/admission-message-ack/v1\0");
    hasher.update(message.message_id);
    hasher.update(payload_digest);
    AdmissionInboxRecord {
        message_id: message.message_id,
        payload_digest,
        acknowledgment_payload: hasher.finalize().to_vec(),
    }
}

pub(crate) fn message(
    attempt_id: SpaceJoinRecordId,
    purpose: AdmissionOutboxPurpose,
    recipient: &[u8],
    predecessor_message_id: Option<[u8; 32]>,
    payload: &[u8],
) -> AdmissionOutboxMessage {
    let mut hasher = Sha256::new();
    hasher.update(b"uniclipboard/admission-message/v1\0");
    hasher.update(attempt_id.as_bytes());
    hasher.update([purpose as u8]);
    hasher.update(predecessor_message_id.unwrap_or([0; 32]));
    hasher.update((recipient.len() as u64).to_be_bytes());
    hasher.update(recipient);
    hasher.update((payload.len() as u64).to_be_bytes());
    hasher.update(payload);
    AdmissionOutboxMessage {
        purpose,
        recipient: recipient.to_vec(),
        message_id: hasher.finalize().into(),
        predecessor_message_id,
        payload: payload.to_vec(),
        superseded: false,
    }
}
