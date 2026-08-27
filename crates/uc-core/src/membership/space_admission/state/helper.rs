use super::*;

#[derive(PartialEq, Eq)]
pub struct SpaceAdmissionCompletionHelperChallenged {
    pub(super) peer_binding: AdmissionPeerBinding,
    pub(super) continuation_credential: AdmissionContinuationCredential,
    pub(super) challenge_counter: u64,
    pub(super) nonce: AdmissionHelperNonce,
    pub(super) last_joiner_message_id: AdmissionMessageId,
    pub(super) last_sponsor_message_id: AdmissionMessageId,
}

#[derive(PartialEq, Eq)]
pub struct SpaceAdmissionCompletionHelperApplied {
    pub(super) peer_binding: AdmissionPeerBinding,
    pub(super) continuation_credential: AdmissionContinuationCredential,
    pub(super) verified_commit: SpaceAdmissionEnvelopeV1,
    pub(super) activation_receipt: AdmissionActivationReceipt,
    pub(super) helper_security: AdmissionHelperSecurityState,
    pub(super) saved_reply: SavedAdmissionReply,
}

#[cfg(test)]
#[allow(dead_code)]
impl SpaceAdmissionCompletionHelperChallenged {
    pub const fn peer_binding(&self) -> AdmissionPeerBinding {
        self.peer_binding
    }

    pub const fn continuation_credential(&self) -> &AdmissionContinuationCredential {
        &self.continuation_credential
    }

    pub const fn challenge_counter(&self) -> u64 {
        self.challenge_counter
    }

    pub const fn nonce(&self) -> &AdmissionHelperNonce {
        &self.nonce
    }

    pub const fn last_joiner_message_id(&self) -> AdmissionMessageId {
        self.last_joiner_message_id
    }

    pub const fn last_sponsor_message_id(&self) -> AdmissionMessageId {
        self.last_sponsor_message_id
    }
}

#[cfg(test)]
#[allow(dead_code)]
impl SpaceAdmissionCompletionHelperApplied {
    pub const fn peer_binding(&self) -> AdmissionPeerBinding {
        self.peer_binding
    }

    pub const fn continuation_credential(&self) -> &AdmissionContinuationCredential {
        &self.continuation_credential
    }

    pub const fn verified_commit(&self) -> &SpaceAdmissionEnvelopeV1 {
        &self.verified_commit
    }

    pub const fn activation_receipt(&self) -> &AdmissionActivationReceipt {
        &self.activation_receipt
    }

    pub const fn helper_security(&self) -> &AdmissionHelperSecurityState {
        &self.helper_security
    }

    pub const fn saved_reply(&self) -> &SavedAdmissionReply {
        &self.saved_reply
    }
}

#[derive(PartialEq, Eq)]
pub enum SpaceAdmissionCompletionHelperState {
    Challenged(SpaceAdmissionCompletionHelperChallenged),
    Applied(SpaceAdmissionCompletionHelperApplied),
}
