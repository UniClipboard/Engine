use uc_core::membership::{
    AdmissionContinuationCredential, AdmissionPeerBinding, AdmissionStagedSecurityState,
    SpaceAdmissionAggregate, SpaceAdmissionEnvelopeV1,
};

pub struct AuthenticatedSpaceAdmissionMessage {
    peer_binding: AdmissionPeerBinding,
    envelope: SpaceAdmissionEnvelopeV1,
    canonical_digest: [u8; 32],
    newly_established_continuation: Option<AdmissionContinuationCredential>,
}

impl AuthenticatedSpaceAdmissionMessage {
    pub fn new(
        peer_binding: AdmissionPeerBinding,
        envelope: SpaceAdmissionEnvelopeV1,
        canonical_digest: [u8; 32],
        newly_established_continuation: Option<AdmissionContinuationCredential>,
    ) -> Option<Self> {
        if canonical_digest == [0; 32] {
            return None;
        }
        Some(Self {
            peer_binding,
            envelope,
            canonical_digest,
            newly_established_continuation,
        })
    }

    pub const fn envelope(&self) -> &SpaceAdmissionEnvelopeV1 {
        &self.envelope
    }

    pub fn into_parts(
        self,
    ) -> (
        AdmissionPeerBinding,
        SpaceAdmissionEnvelopeV1,
        [u8; 32],
        Option<AdmissionContinuationCredential>,
    ) {
        (
            self.peer_binding,
            self.envelope,
            self.canonical_digest,
            self.newly_established_continuation,
        )
    }
}

pub struct PreparedSponsorCandidate {
    candidate_reply: SpaceAdmissionEnvelopeV1,
    staged_security: AdmissionStagedSecurityState,
}

impl PreparedSponsorCandidate {
    pub fn new(
        candidate_reply: SpaceAdmissionEnvelopeV1,
        staged_security: AdmissionStagedSecurityState,
    ) -> Self {
        Self {
            candidate_reply,
            staged_security,
        }
    }

    pub(crate) fn into_parts(self) -> (SpaceAdmissionEnvelopeV1, AdmissionStagedSecurityState) {
        (self.candidate_reply, self.staged_security)
    }
}

pub struct SpaceAdmissionMessageReply {
    committed: SpaceAdmissionAggregate,
}

impl SpaceAdmissionMessageReply {
    pub(crate) fn new(committed: SpaceAdmissionAggregate) -> Option<Self> {
        committed
            .current_exact_reply()
            .is_some()
            .then_some(Self { committed })
    }

    pub fn envelope(&self) -> Option<&SpaceAdmissionEnvelopeV1> {
        self.committed.current_exact_reply()
    }

    #[cfg(test)]
    pub(crate) fn into_aggregate(self) -> SpaceAdmissionAggregate {
        self.committed
    }
}
