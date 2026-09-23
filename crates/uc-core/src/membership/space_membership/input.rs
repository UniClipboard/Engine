use crate::ids::DeviceId;
use crate::membership::{
    BaseMembershipHistoryPosition, MembershipEventId, VersionedMembershipHistory,
};

use super::{MemberEffectPhase, SpaceMembership};

/// 一次完整的外部事实。改变历史的输入携带 Application 已经用历史规则与验签器产生的新历史，
/// 聚合只核对它与当前状态一致后采用。
#[derive(Debug, Clone)]
pub enum MembershipInput {
    /// 本机签名并追加了一项移除；新历史的当前头就是该移除。
    LocalRemovalSigned {
        history: VersionedMembershipHistory,
        retained_device_ids: Vec<DeviceId>,
    },
    /// 本机对一项远端移除作出并签名了决定；新历史包含该决定。
    LocalDecisionSigned {
        history: VersionedMembershipHistory,
        removal_event_id: MembershipEventId,
    },
    /// 邀请方在准入唯一提交点写入新成员；新历史的当前头就是该加入。
    AdmissionCommitted { history: VersionedMembershipHistory },
    /// 已认证对端的历史证据已按历史规则核对；`history` 为本机因此采用的新历史。
    PeerEvidenceReconciled {
        source: DeviceId,
        history: Option<VersionedMembershipHistory>,
        evidence: PeerEvidence,
    },
    /// 执行器在网络调用前选定本轮要同步的对端。
    HistorySyncSelected { peers: Vec<DeviceId> },
    /// 向一个对端同步 `synced_position` 的结果。
    HistorySyncFinished {
        peer: DeviceId,
        synced_position: BaseMembershipHistoryPosition,
        result: HistorySyncResult,
    },
    /// 一项受限投递的结果。
    DeliveryFinished {
        peer: DeviceId,
        delivery: DeliveryKind,
        result: DeliveryResult,
    },
    /// 离开窗口已到期。
    DepartureWindowElapsed { peer: DeviceId },
    /// 成员效果从 `from` 阶段出发的一步已完成。
    EffectStepFinished {
        event_id: MembershipEventId,
        from: MemberEffectPhase,
    },
    /// 分叉恢复已选定并准备好目标分支。
    BranchRecovered { history: VersionedMembershipHistory },
}

/// 对端历史证据的核对结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerEvidence {
    /// 对端确认了本机采用证据后的当前位置。
    Confirmed,
    Diverged,
    Invalid,
    /// 证据不足，关系保持原状。
    NeedsEvidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistorySyncResult {
    Confirmed,
    Diverged,
    Invalid,
    /// 暂时无法完成，按退避重试。
    Deferred,
    /// 对端稳定拒绝。
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryKind {
    RemovalNotice,
    Decision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryResult {
    Delivered,
    Deferred,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MembershipOutcome {
    /// 状态已改变，调用方必须保存新状态。
    Applied,
    /// 输入已被处理过或不带来变化。
    Unchanged,
    /// 输入基于已经过时的状态，被忽略。
    Stale,
}

/// 保存新状态后必须履行的后续动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MembershipFollowUp {
    PublishDeviceTrustChange,
    WakeWorker,
}

/// 与保存新状态的先后关系。本聚合只有保存后执行的效果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MembershipEffect {
    AfterCommit(MembershipFollowUp),
}

#[derive(Debug, Clone)]
pub struct MembershipTransition {
    replacement: SpaceMembership,
    outcome: MembershipOutcome,
    effects: Vec<MembershipEffect>,
}

impl MembershipTransition {
    pub(super) fn new(
        replacement: SpaceMembership,
        outcome: MembershipOutcome,
        effects: Vec<MembershipEffect>,
    ) -> Self {
        Self {
            replacement,
            outcome,
            effects,
        }
    }

    pub fn replacement(&self) -> &SpaceMembership {
        &self.replacement
    }

    pub fn outcome(&self) -> MembershipOutcome {
        self.outcome
    }

    pub fn effects(&self) -> &[MembershipEffect] {
        &self.effects
    }

    pub fn into_parts(self) -> (SpaceMembership, MembershipOutcome, Vec<MembershipEffect>) {
        (self.replacement, self.outcome, self.effects)
    }
}
