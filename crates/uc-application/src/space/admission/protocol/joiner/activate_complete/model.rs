use uc_core::membership::{
    AdmissionSpaceTransitionResult, JoinerAdmission, JoinerAdmissionTransition, MemberInstanceId,
    PendingAdmissionExchange, SpaceAdmissionId, VersionedMembershipHistory,
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
    membership: JoinerMembershipStart,
}

impl CompletedJoinerActivation {
    pub fn new(
        transition_result: AdmissionSpaceTransitionResult,
        pending_exchange: PendingAdmissionExchange,
        outcome: JoinerActivationOutcome,
        membership: JoinerMembershipStart,
    ) -> Self {
        Self {
            transition_result,
            pending_exchange,
            outcome,
            membership,
        }
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        AdmissionSpaceTransitionResult,
        PendingAdmissionExchange,
        JoinerActivationOutcome,
        JoinerMembershipStart,
    ) {
        (
            self.transition_result,
            self.pending_exchange,
            self.outcome,
            self.membership,
        )
    }
}

/// 加入方本机成员状态的起点，由激活执行从已保存的准入资料重建并校验。
///
/// 目标控制世代生效后交给成员状态负责人建立本机成员状态，先于准入终态保存。
pub struct JoinerMembershipStart {
    pub space_id: String,
    pub local_device_id: DeviceId,
    pub local_member: MemberInstanceId,
    /// 已记入本机激活回执的加入后历史。为空表示该激活由旧版本准备，目标控制世代已带有同一加入的
    /// 成员记录，只需核对一致。
    pub history: Option<VersionedMembershipHistory>,
}

impl std::fmt::Debug for JoinerMembershipStart {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("JoinerMembershipStart")
            .field("has_history", &self.history.is_some())
            .finish_non_exhaustive()
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
