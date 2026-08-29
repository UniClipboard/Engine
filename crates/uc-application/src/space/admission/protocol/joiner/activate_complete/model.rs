use uc_core::membership::{
    AdmissionSpaceTransitionResult, JoinerAdmission, JoinerAdmissionTransition,
    PendingAdmissionExchange,
};
use uc_core::security::IdentityFingerprint;
use uc_core::DeviceId;

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
    aggregate: JoinerAdmission,
    commit_token: JoinerActivationCommitToken,
}

impl LoadedJoinerActivation {
    pub fn new(aggregate: JoinerAdmission, commit_token: JoinerActivationCommitToken) -> Self {
        Self {
            aggregate,
            commit_token,
        }
    }

    pub fn into_parts(self) -> (JoinerAdmission, JoinerActivationCommitToken) {
        (self.aggregate, self.commit_token)
    }
}

pub struct CompletedJoinerActivation {
    transition_result: AdmissionSpaceTransitionResult,
    pending_exchange: PendingAdmissionExchange,
    outcome: JoinerActivationOutcome,
}

impl CompletedJoinerActivation {
    pub fn new(
        transition_result: AdmissionSpaceTransitionResult,
        pending_exchange: PendingAdmissionExchange,
        outcome: JoinerActivationOutcome,
    ) -> Self {
        Self {
            transition_result,
            pending_exchange,
            outcome,
        }
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        AdmissionSpaceTransitionResult,
        PendingAdmissionExchange,
        JoinerActivationOutcome,
    ) {
        (self.transition_result, self.pending_exchange, self.outcome)
    }
}

/// 激活完成后返回给产品的稳定摘要；不暴露切换步骤或持久化表示。
pub struct JoinerActivationOutcome {
    pub join_id: [u8; 16],
    pub sponsor_device_id: DeviceId,
    pub sponsor_identity_fingerprint: IdentityFingerprint,
    pub space_id: String,
    pub self_device_id: DeviceId,
    pub self_identity_fingerprint: IdentityFingerprint,
    pub migrated_records: Option<u64>,
    pub preserved_unreadable_records: Option<u64>,
}

pub struct JoinerActivationMutation {
    transition: JoinerAdmissionTransition,
}

impl JoinerActivationMutation {
    pub const fn new(transition: JoinerAdmissionTransition) -> Self {
        Self { transition }
    }

    pub fn into_transition(self) -> JoinerAdmissionTransition {
        self.transition
    }
}
