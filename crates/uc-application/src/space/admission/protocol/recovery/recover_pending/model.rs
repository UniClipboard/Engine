use uc_core::ids::DeviceId;
use uc_core::membership::{SpaceAdmissionAggregate, SpaceAdmissionEnvelopeV1};

/// 是什么事情唤醒了恢复流程
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AdmissionRecoveryTrigger {
    /// 应用启动后检查未完成的加入
    Startup,
    /// 应用或会话恢复运行
    Resume,
    /// 定时检查
    Periodic,
    /// 刚保存了新的加入状态， 需要立即继续
    StateChanged,
    /// 观察到设备重新可达
    PeerOnline(DeviceId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AdmissionRecoveryReport {
    /// 成功向前推进了一个阶段的加入数量, 不代表全部加入成功
    pub advanced_count: usize,
    /// 暂时无法继续，以后需要重试的数量
    pub deferred_count: usize,
    /// 已经得到稳定拒绝结果的数量
    pub rejected_count: usize,
    /// 状态损坏或违反规则，必须进入恢复处理的数量
    pub recovery_required_count: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct AdmissionRecoveryCommitToken([u8; 32]);

impl AdmissionRecoveryCommitToken {
    pub fn from_bytes(bytes: [u8; 32]) -> Option<Self> {
        if bytes == [0; 32] {
            None
        } else {
            Some(Self(bytes))
        }
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

pub struct LoadedPendingAdmission {
    aggregate: SpaceAdmissionAggregate,
    commit_token: AdmissionRecoveryCommitToken,
}

pub struct AuthenticatedAdmissionReply {
    envelope: SpaceAdmissionEnvelopeV1,
    canonical_digest: [u8; 32],
}

impl AuthenticatedAdmissionReply {
    pub fn new(envelope: SpaceAdmissionEnvelopeV1, canonical_digest: [u8; 32]) -> Option<Self> {
        (canonical_digest != [0; 32]).then_some(Self {
            envelope,
            canonical_digest,
        })
    }

    pub(crate) fn into_parts(self) -> (SpaceAdmissionEnvelopeV1, [u8; 32]) {
        (self.envelope, self.canonical_digest)
    }
}

impl LoadedPendingAdmission {
    pub fn new(
        aggregate: SpaceAdmissionAggregate,
        commit_token: AdmissionRecoveryCommitToken,
    ) -> Self {
        Self {
            aggregate,
            commit_token,
        }
    }

    pub(crate) fn into_parts(self) -> (SpaceAdmissionAggregate, AdmissionRecoveryCommitToken) {
        (self.aggregate, self.commit_token)
    }
}
