use crate::ids::DeviceId;
use crate::membership::{
    BaseMembershipHistoryPosition, MembershipEventId, VersionedMembershipHistory,
};

use super::{MemberEffectPhase, MembershipLedger};

/// 一次完整的外部事实。改变历史的输入携带 Application 已经用历史规则与验签器产生的新历史，
/// 聚合只核对它与当前状态一致后采用。
#[derive(Debug, Clone)]
pub enum LedgerInput {
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
    /// 已认证对端的历史证据已按历史规则核对；`history` 为本机因此采用的新历史。来源不是本机当前
    /// 成员时只采用已验证历史，不记录关系。
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
        result: PeerSyncResult,
    },
    /// 一项受限投递的结果。
    DeliveryFinished {
        peer: DeviceId,
        delivery: LedgerDeliveryKind,
        result: LedgerDeliveryResult,
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
    /// 与账本同存的非聚合资料（历史交换暂存、分叉恢复资料）已改变；只推进修订号。
    /// `presentation_changed` 表示这些资料会改变设备分组展示。
    CompanionDataChanged { presentation_changed: bool },
}

/// 对端历史证据的核对结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerEvidence {
    /// 对端确认了本机采用证据后的当前位置。
    Confirmed,
    /// 双方分支一致，但证据不能证明对端拥有本机当前位置；已确认位置保持不变。
    Consistent,
    Diverged,
    Invalid,
    /// 证据不足，关系保持原状。
    NeedsEvidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerSyncResult {
    Confirmed,
    Diverged,
    Invalid,
    /// 暂时无法完成，按退避重试。
    Deferred,
    /// 对端稳定拒绝。
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedgerDeliveryKind {
    RemovalNotice,
    Decision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedgerDeliveryResult {
    Delivered,
    Deferred,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedgerOutcome {
    /// 状态已改变，调用方必须保存新状态。
    Applied,
    /// 输入已被处理过或不带来变化。
    Unchanged,
    /// 输入基于已经过时的状态，被忽略。
    Stale,
}

/// 保存新状态后必须履行的后续动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedgerFollowUp {
    PublishDeviceTrustChange,
    WakeWorker,
}

/// 与保存新状态的先后关系。本聚合只有保存后执行的效果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedgerEffect {
    AfterCommit(LedgerFollowUp),
}

#[derive(Debug, Clone)]
pub struct LedgerTransition {
    replacement: MembershipLedger,
    outcome: LedgerOutcome,
    effects: Vec<LedgerEffect>,
}

impl LedgerTransition {
    pub(super) fn new(
        replacement: MembershipLedger,
        outcome: LedgerOutcome,
        effects: Vec<LedgerEffect>,
    ) -> Self {
        Self {
            replacement,
            outcome,
            effects,
        }
    }

    pub fn replacement(&self) -> &MembershipLedger {
        &self.replacement
    }

    pub fn outcome(&self) -> LedgerOutcome {
        self.outcome
    }

    pub fn effects(&self) -> &[LedgerEffect] {
        &self.effects
    }

    pub fn into_parts(self) -> (MembershipLedger, LedgerOutcome, Vec<LedgerEffect>) {
        (self.replacement, self.outcome, self.effects)
    }
}
