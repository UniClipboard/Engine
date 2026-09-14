use uc_core::membership::{
    AdmissionSpaceTransitionResult, JoinerAdmission, JoinerAdmissionTransition,
    PendingAdmissionExchange, SpaceAdmissionId,
};
use uc_core::security::IdentityFingerprint;
use uc_core::DeviceId;

const ACTIVATION_INTENT_DOMAIN: &[u8] = b"uniclipboard/joiner-activation-intent/v1\0";

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct JoinerActivationIntent {
    admission_id: SpaceAdmissionId,
    plan_digest: [u8; 32],
}

impl JoinerActivationIntent {
    pub fn from_saved_plan(admission_id: SpaceAdmissionId, plan: &[u8]) -> Option<Self> {
        use sha2::{Digest as _, Sha256};

        if plan.is_empty() {
            return None;
        }
        let mut hasher = Sha256::new();
        hasher.update(ACTIVATION_INTENT_DOMAIN);
        hasher.update(admission_id.as_bytes());
        hasher.update((plan.len() as u64).to_be_bytes());
        hasher.update(plan);
        Some(Self {
            admission_id,
            plan_digest: hasher.finalize().into(),
        })
    }

    pub const fn admission_id(&self) -> SpaceAdmissionId {
        self.admission_id
    }

    pub const fn plan_digest(&self) -> &[u8; 32] {
        &self.plan_digest
    }
}

impl std::fmt::Debug for JoinerActivationIntent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("JoinerActivationIntent([REDACTED])")
    }
}

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
