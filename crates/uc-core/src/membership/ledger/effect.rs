use crate::ids::DeviceId;
use crate::membership::{MembershipDecisionV2, MembershipEventId, MembershipEventV2};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberEffectKind {
    AddDevice,
    RemoveDevice,
}

/// 成员效果正在等待执行的阶段；最后一步完成即激活并删除该效果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MemberEffectPhase {
    /// 待应用成员资料。
    Prepared,
    /// 待应用安全状态。
    MemberFactsApplied,
    /// 待激活。
    SecurityApplied,
}

impl MemberEffectPhase {
    pub(super) fn next(self) -> Option<Self> {
        match self {
            Self::Prepared => Some(Self::MemberFactsApplied),
            Self::MemberFactsApplied => Some(Self::SecurityApplied),
            Self::SecurityApplied => None,
        }
    }
}

/// 执行效果所需的已签名材料。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemberEffectMaterial {
    /// 从对端历史中采用的成员事件。
    Event(MembershipEventV2),
    /// 本机发起的移除，以及移除后保留的其他成员。
    InitiatedRemoval {
        event: MembershipEventV2,
        retained_device_ids: Vec<DeviceId>,
    },
    /// 本机接受的远端移除决定。
    Decision(MembershipDecisionV2),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnfinishedMemberEffect {
    event_id: MembershipEventId,
    kind: MemberEffectKind,
    phase: MemberEffectPhase,
    affected_device_ids: Vec<DeviceId>,
    material: MemberEffectMaterial,
}

impl UnfinishedMemberEffect {
    pub(super) fn prepared(
        event_id: MembershipEventId,
        kind: MemberEffectKind,
        affected_device_ids: Vec<DeviceId>,
        material: MemberEffectMaterial,
    ) -> Self {
        Self {
            event_id,
            kind,
            phase: MemberEffectPhase::Prepared,
            affected_device_ids,
            material,
        }
    }

    /// 从持久快照或已提交资料还原一项效果；效果只有经账本恢复或推进才具备执行资格。
    pub fn from_parts(
        event_id: MembershipEventId,
        kind: MemberEffectKind,
        phase: MemberEffectPhase,
        affected_device_ids: Vec<DeviceId>,
        material: MemberEffectMaterial,
    ) -> Self {
        Self {
            event_id,
            kind,
            phase,
            affected_device_ids,
            material,
        }
    }

    pub fn event_id(&self) -> MembershipEventId {
        self.event_id
    }

    pub fn kind(&self) -> MemberEffectKind {
        self.kind
    }

    pub fn phase(&self) -> MemberEffectPhase {
        self.phase
    }

    pub fn affected_device_ids(&self) -> &[DeviceId] {
        &self.affected_device_ids
    }

    pub fn material(&self) -> &MemberEffectMaterial {
        &self.material
    }

    pub(super) fn affects(&self, device_id: &DeviceId) -> bool {
        self.affected_device_ids.contains(device_id)
    }

    pub(super) fn advance(&mut self, next: MemberEffectPhase) {
        self.phase = next;
    }
}
