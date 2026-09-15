use uc_core::ids::DeviceId;
use uc_core::membership::{JoinerAdmission, SpaceAdmissionEnvelopeV1, SponsorAdmission};

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
    /// 已先于网络动作在本机保存到期终态的数量
    pub terminated_count: usize,
    /// 对端版本无法继续当前交换，等待明确升级处理的数量
    pub peer_upgrade_required_count: usize,
    /// 状态损坏或违反规则，必须进入恢复处理的数量
    pub recovery_required_count: usize,
    /// 本次准入推进结束后，成员维护是否可以继续执行普通同步
    pub(crate) disposition: AdmissionRecoveryDisposition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum AdmissionRecoveryDisposition {
    #[default]
    ContinueMaintenance,
    YieldMaintenance,
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
    aggregate: JoinerAdmission,
    commit_token: AdmissionRecoveryCommitToken,
}

pub struct LoadedSponsorDeadline {
    aggregate: SponsorAdmission,
    commit_token: AdmissionRecoveryCommitToken,
}

pub struct LoadedSponsorAbandonment {
    aggregate: SponsorAdmission,
    commit_token: AdmissionRecoveryCommitToken,
}

#[derive(Default)]
pub struct LoadedAdmissionRecovery {
    pending_admissions: Vec<LoadedPendingAdmission>,
    sponsor_deadlines: Vec<LoadedSponsorDeadline>,
    sponsor_abandonments: Vec<LoadedSponsorAbandonment>,
    next_deadline_ms: Option<i64>,
    /// 邀请方仍在等待加入方的最终确认
    sponsor_confirmation_pending: bool,
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

    pub fn into_parts(self) -> (SpaceAdmissionEnvelopeV1, [u8; 32]) {
        (self.envelope, self.canonical_digest)
    }
}

impl LoadedPendingAdmission {
    pub fn new(aggregate: JoinerAdmission, commit_token: AdmissionRecoveryCommitToken) -> Self {
        Self {
            aggregate,
            commit_token,
        }
    }

    pub fn into_parts(self) -> (JoinerAdmission, AdmissionRecoveryCommitToken) {
        (self.aggregate, self.commit_token)
    }
}

impl LoadedSponsorDeadline {
    pub fn new(aggregate: SponsorAdmission, commit_token: AdmissionRecoveryCommitToken) -> Self {
        Self {
            aggregate,
            commit_token,
        }
    }

    pub fn into_parts(self) -> (SponsorAdmission, AdmissionRecoveryCommitToken) {
        (self.aggregate, self.commit_token)
    }
}

impl LoadedSponsorAbandonment {
    pub fn new(aggregate: SponsorAdmission, commit_token: AdmissionRecoveryCommitToken) -> Self {
        Self {
            aggregate,
            commit_token,
        }
    }

    pub fn into_parts(self) -> (SponsorAdmission, AdmissionRecoveryCommitToken) {
        (self.aggregate, self.commit_token)
    }
}

impl LoadedAdmissionRecovery {
    pub fn new(
        pending_admissions: Vec<LoadedPendingAdmission>,
        sponsor_deadlines: Vec<LoadedSponsorDeadline>,
        sponsor_abandonments: Vec<LoadedSponsorAbandonment>,
        next_deadline_ms: Option<i64>,
        sponsor_confirmation_pending: bool,
    ) -> Self {
        Self {
            pending_admissions,
            sponsor_deadlines,
            sponsor_abandonments,
            next_deadline_ms,
            sponsor_confirmation_pending,
        }
    }

    pub fn into_parts(
        self,
    ) -> (
        Vec<LoadedPendingAdmission>,
        Vec<LoadedSponsorDeadline>,
        Vec<LoadedSponsorAbandonment>,
        Option<i64>,
        bool,
    ) {
        (
            self.pending_admissions,
            self.sponsor_deadlines,
            self.sponsor_abandonments,
            self.next_deadline_ms,
            self.sponsor_confirmation_pending,
        )
    }

    pub fn is_empty(&self) -> bool {
        self.pending_admissions.is_empty()
            && self.sponsor_deadlines.is_empty()
            && self.sponsor_abandonments.is_empty()
    }

    pub fn len(&self) -> usize {
        self.pending_admissions.len()
            + self.sponsor_deadlines.len()
            + self.sponsor_abandonments.len()
    }

    pub fn into_pending_admissions(self) -> Vec<LoadedPendingAdmission> {
        self.pending_admissions
    }
}
