use uc_core::membership::{
    AdmissionSpaceTransitionResult, AdmissionTransition, PendingAdmissionExchange,
    SpaceAdmissionAggregate,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct JoinerActivationCommitToken([u8; 32]);

impl JoinerActivationCommitToken {
    pub fn from_bytes(bytes: [u8; 32]) -> Option<Self> {
        (bytes != [0; 32]).then_some(Self(bytes))
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

pub struct LoadedJoinerActivation {
    aggregate: SpaceAdmissionAggregate,
    commit_token: JoinerActivationCommitToken,
}

impl LoadedJoinerActivation {
    pub fn new(
        aggregate: SpaceAdmissionAggregate,
        commit_token: JoinerActivationCommitToken,
    ) -> Self {
        Self {
            aggregate,
            commit_token,
        }
    }

    pub fn into_parts(self) -> (SpaceAdmissionAggregate, JoinerActivationCommitToken) {
        (self.aggregate, self.commit_token)
    }
}

pub struct CompletedJoinerActivation {
    transition_result: AdmissionSpaceTransitionResult,
    pending_exchange: PendingAdmissionExchange,
}

impl CompletedJoinerActivation {
    pub fn new(
        transition_result: AdmissionSpaceTransitionResult,
        pending_exchange: PendingAdmissionExchange,
    ) -> Self {
        Self {
            transition_result,
            pending_exchange,
        }
    }

    pub(crate) fn into_parts(self) -> (AdmissionSpaceTransitionResult, PendingAdmissionExchange) {
        (self.transition_result, self.pending_exchange)
    }
}

pub struct JoinerActivationMutation {
    transition: AdmissionTransition,
}

impl JoinerActivationMutation {
    pub const fn new(transition: AdmissionTransition) -> Self {
        Self { transition }
    }

    pub fn into_transition(self) -> AdmissionTransition {
        self.transition
    }
}
