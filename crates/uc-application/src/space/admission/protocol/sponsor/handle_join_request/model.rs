use uc_core::membership::{
    AdmissionBaseSnapshot, AdmissionContinuationCredential, AdmissionInvitationClaim,
    AdmissionPeerBinding, AdmissionStagedSecurityState, AdmissionTransition,
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

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SponsorJoinRequestCommitToken([u8; 32]);

impl SponsorJoinRequestCommitToken {
    pub fn from_bytes(bytes: [u8; 32]) -> Option<Self> {
        (bytes != [0; 32]).then_some(Self(bytes))
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

pub enum SponsorJoinRequestState {
    Fresh {
        invitation_claim: AdmissionInvitationClaim,
        base_snapshot: AdmissionBaseSnapshot,
    },
    Existing(SpaceAdmissionAggregate),
}

pub struct LoadedSponsorJoinRequest {
    state: SponsorJoinRequestState,
    commit_token: SponsorJoinRequestCommitToken,
}

impl LoadedSponsorJoinRequest {
    pub fn new(
        state: SponsorJoinRequestState,
        commit_token: SponsorJoinRequestCommitToken,
    ) -> Self {
        Self {
            state,
            commit_token,
        }
    }

    pub fn into_parts(self) -> (SponsorJoinRequestState, SponsorJoinRequestCommitToken) {
        (self.state, self.commit_token)
    }
}

pub struct CommittedSponsorAdmission {
    aggregate: SpaceAdmissionAggregate,
    commit_token: SponsorJoinRequestCommitToken,
}

impl CommittedSponsorAdmission {
    pub fn new(
        aggregate: SpaceAdmissionAggregate,
        commit_token: SponsorJoinRequestCommitToken,
    ) -> Self {
        Self {
            aggregate,
            commit_token,
        }
    }

    pub fn into_parts(self) -> (SpaceAdmissionAggregate, SponsorJoinRequestCommitToken) {
        (self.aggregate, self.commit_token)
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

pub struct SponsorAdmissionMutation {
    transition: AdmissionTransition,
}

impl SponsorAdmissionMutation {
    pub const fn new(transition: AdmissionTransition) -> Self {
        Self { transition }
    }

    pub fn into_transition(self) -> AdmissionTransition {
        self.transition
    }
}
