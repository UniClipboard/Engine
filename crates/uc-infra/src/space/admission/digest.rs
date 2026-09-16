use sha2::{Digest, Sha256};
use uc_core::membership::{AdmissionCompleteAckV1, AdmissionCompletionV1};

pub(super) fn completion_digest(completion: &AdmissionCompletionV1) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"uniclipboard/admission-completion-digest/v1\0");
    hasher.update(completion.signing_payload());
    hasher.update((completion.signature.len() as u64).to_be_bytes());
    hasher.update(&completion.signature);
    hasher.finalize().into()
}

pub(super) fn complete_ack_digest(acknowledgment: &AdmissionCompleteAckV1) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"uniclipboard/admission-complete-ack-digest/v1\0");
    hasher.update(acknowledgment.completion_digest());
    hasher.finalize().into()
}

#[cfg(test)]
mod tests {
    use uc_core::membership::{
        BaseMembershipHistoryPosition, MemberInstanceId, MembershipCredential, MembershipEventId,
    };

    use super::*;

    #[test]
    fn completion_and_ack_digests_are_domain_separated_and_fact_bound() {
        let event_id: MembershipEventId =
            postcard::from_bytes(&[0x31; 32]).expect("membership event id fixture");
        let credential = MembershipCredential::new(1, vec![0x32; 32]);
        let mut completion = AdmissionCompletionV1::new(
            [0x33; 32],
            event_id,
            [0x34; 32],
            [0x35; 32],
            MemberInstanceId::from_bytes([0x36; 32]),
            credential.credential_id,
            BaseMembershipHistoryPosition {
                event_id: Some(event_id),
                depth: 1,
                history_digest: [0x37; 32],
            },
            vec![0x38; 64],
        );
        let first = completion_digest(&completion);
        completion.signature[0] ^= 1;
        let second = completion_digest(&completion);
        let acknowledgment = AdmissionCompleteAckV1::new(first).expect("non-zero digest fixture");

        assert_ne!(first, second);
        assert_ne!(complete_ack_digest(&acknowledgment), first);
    }
}
