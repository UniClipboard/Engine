use uc_core::membership::{
    AdmissionBaseSnapshot, AdmissionInvitationClaim, SponsorAdmission, SponsorAdmissionTransition,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SponsorAdmissionCommitToken([u8; 32]);

impl SponsorAdmissionCommitToken {
    pub fn from_bytes(bytes: [u8; 32]) -> Option<Self> {
        (bytes != [0; 32]).then_some(Self(bytes))
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

pub enum SponsorAdmissionState {
    Fresh {
        invitation_claim: AdmissionInvitationClaim,
        base_snapshot: AdmissionBaseSnapshot,
    },
    Existing(SponsorAdmission),
}

pub struct LoadedSponsorAdmission {
    state: SponsorAdmissionState,
    commit_token: SponsorAdmissionCommitToken,
}

impl LoadedSponsorAdmission {
    pub fn new(state: SponsorAdmissionState, commit_token: SponsorAdmissionCommitToken) -> Self {
        Self {
            state,
            commit_token,
        }
    }

    pub fn into_parts(self) -> (SponsorAdmissionState, SponsorAdmissionCommitToken) {
        (self.state, self.commit_token)
    }
}

pub struct CommittedSponsorAdmission {
    aggregate: SponsorAdmission,
    commit_token: SponsorAdmissionCommitToken,
}

impl CommittedSponsorAdmission {
    pub fn new(aggregate: SponsorAdmission, commit_token: SponsorAdmissionCommitToken) -> Self {
        Self {
            aggregate,
            commit_token,
        }
    }

    pub fn into_parts(self) -> (SponsorAdmission, SponsorAdmissionCommitToken) {
        (self.aggregate, self.commit_token)
    }
}

pub struct SponsorAdmissionMutation {
    transition: SponsorAdmissionTransition,
}

impl SponsorAdmissionMutation {
    pub const fn new(transition: SponsorAdmissionTransition) -> Self {
        Self { transition }
    }

    pub fn into_transition(self) -> SponsorAdmissionTransition {
        self.transition
    }
}
