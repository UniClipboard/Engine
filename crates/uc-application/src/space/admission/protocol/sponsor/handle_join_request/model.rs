use uc_core::membership::{
    AdmissionAttemptContractV2, AdmissionContinuationCredential, AdmissionPeerBinding,
    AdmissionStagedSecurityState, SpaceAdmissionEnvelopeV1, SponsorAdmission,
};

pub struct AuthenticatedSpaceAdmissionMessage {
    peer_binding: AdmissionPeerBinding,
    envelope: SpaceAdmissionEnvelopeV1,
    canonical_digest: [u8; 32],
    newly_established_continuation: Option<AdmissionContinuationCredential>,
    attempt_contract: Option<AdmissionAttemptContractV2>,
}

impl AuthenticatedSpaceAdmissionMessage {
    pub fn new(
        peer_binding: AdmissionPeerBinding,
        envelope: SpaceAdmissionEnvelopeV1,
        canonical_digest: [u8; 32],
        newly_established_continuation: Option<AdmissionContinuationCredential>,
        attempt_contract: Option<AdmissionAttemptContractV2>,
    ) -> Option<Self> {
        if canonical_digest == [0; 32] {
            return None;
        }
        Some(Self {
            peer_binding,
            envelope,
            canonical_digest,
            newly_established_continuation,
            attempt_contract,
        })
    }

    pub const fn envelope(&self) -> &SpaceAdmissionEnvelopeV1 {
        &self.envelope
    }

    pub const fn peer_binding(&self) -> AdmissionPeerBinding {
        self.peer_binding
    }

    pub const fn attempt_contract(&self) -> Option<&AdmissionAttemptContractV2> {
        self.attempt_contract.as_ref()
    }

    pub fn into_parts(
        self,
    ) -> (
        AdmissionPeerBinding,
        SpaceAdmissionEnvelopeV1,
        [u8; 32],
        Option<AdmissionContinuationCredential>,
        Option<AdmissionAttemptContractV2>,
    ) {
        (
            self.peer_binding,
            self.envelope,
            self.canonical_digest,
            self.newly_established_continuation,
            self.attempt_contract,
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

    pub fn into_parts(self) -> (SpaceAdmissionEnvelopeV1, AdmissionStagedSecurityState) {
        (self.candidate_reply, self.staged_security)
    }
}

pub struct SpaceAdmissionMessageReply {
    committed: SponsorAdmission,
}

impl SpaceAdmissionMessageReply {
    pub fn new(committed: SponsorAdmission) -> Option<Self> {
        committed
            .current_exact_reply()
            .is_some()
            .then_some(Self { committed })
    }

    pub fn envelope(&self) -> Option<&SpaceAdmissionEnvelopeV1> {
        self.committed.current_exact_reply()
    }

    pub const fn expires_at_ms(&self) -> Option<i64> {
        self.committed.expires_at_ms()
    }

    pub const fn has_pairing_confirmation(&self) -> bool {
        self.committed.pairing_confirmation().is_some()
    }

    #[cfg(test)]
    pub(crate) fn into_admission(self) -> SponsorAdmission {
        self.committed
    }
}
