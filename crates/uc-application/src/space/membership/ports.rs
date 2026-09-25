//! 成员状态负责人与执行器需要的外部能力。

use async_trait::async_trait;
use uc_core::ids::DeviceId;
use uc_core::membership::{MembershipDecisionV2, MembershipEventV2, UnfinishedMemberEffect};

use super::{MembershipProjectionPlan, MembershipRecord};

#[derive(Debug, thiserror::Error)]
pub enum MembershipLedgerError {
    #[error("space is locked")]
    Locked,
    #[error("membership ledger changed")]
    Conflict,
    #[error("membership ledger is corrupt")]
    Corrupt {
        #[source]
        source: Option<anyhow::Error>,
    },
    #[error("membership ledger is unavailable")]
    Unavailable {
        #[source]
        source: Option<anyhow::Error>,
    },
    #[error("membership recovery is required")]
    RecoveryRequired,
}

/// 纯状态或输入校验失败时 `source` 为空；有下层错误时保留为来源。
impl MembershipLedgerError {
    pub fn corrupt() -> Self {
        Self::Corrupt { source: None }
    }

    pub fn corrupt_from(source: impl Into<anyhow::Error>) -> Self {
        Self::Corrupt {
            source: Some(source.into()),
        }
    }

    pub fn unavailable() -> Self {
        Self::Unavailable { source: None }
    }

    pub fn unavailable_from(source: impl Into<anyhow::Error>) -> Self {
        Self::Unavailable {
            source: Some(source.into()),
        }
    }
}

/// 一次成员记录提交：记录本身与由它推导出的成员读模型必须同时成立。
#[derive(Clone)]
pub struct MembershipRecordCommit {
    pub expected_revision: u64,
    pub replacement: MembershipRecord,
    /// 没有当前 Space 时为空；成员读模型由重建流程另行清理。
    pub projection: Option<MembershipProjectionPlan>,
}

/// 成员记录的加密持久能力。
///
/// 实现必须以 profile 或 Space MasterKey 加密记录的全部字段，不得保留明文镜像。
#[async_trait]
pub trait MembershipRecordStorePort: Send + Sync {
    async fn load(&self) -> Result<MembershipRecord, MembershipLedgerError>;

    /// 仅当当前修订号等于 `expected_revision` 且替换记录的修订号更大时，在同一事务中写入记录并
    /// 落实成员读模型；任一部分失败时两者都不改变。
    async fn commit(&self, commit: MembershipRecordCommit) -> Result<(), MembershipLedgerError>;
}

#[derive(Debug, thiserror::Error)]
pub enum MembershipEffectExecutionError {
    #[error("membership effect is temporarily unavailable")]
    Deferred {
        #[source]
        source: Option<anyhow::Error>,
    },
    #[error("membership effect state is corrupt")]
    Corrupt,
    #[error("membership effect dependency failed")]
    Dependency {
        #[source]
        source: anyhow::Error,
    },
}

/// 纯状态或输入校验失败时 `source` 为空；有下层错误时保留为来源。
impl MembershipEffectExecutionError {
    pub fn deferred() -> Self {
        Self::Deferred { source: None }
    }

    pub fn deferred_from(source: impl Into<anyhow::Error>) -> Self {
        Self::Deferred {
            source: Some(source.into()),
        }
    }
}

/// 成员效果的第一步：维护成员资料。实现必须按事件幂等。
#[async_trait]
pub trait ApplyMembershipMemberFactsPort: Send + Sync {
    async fn apply_member_facts(
        &self,
        effect: &UnfinishedMemberEffect,
    ) -> Result<(), MembershipEffectExecutionError>;
}

/// 成员效果的第二步：更新安全状态。实现必须按事件幂等。
#[async_trait]
pub trait ApplyMembershipSecurityPort: Send + Sync {
    async fn apply_membership_security(
        &self,
        effect: &UnfinishedMemberEffect,
    ) -> Result<(), MembershipEffectExecutionError>;
}

/// 成员效果的最后一步：激活。实现必须按事件幂等。
#[async_trait]
pub trait ActivateMembershipEffectPort: Send + Sync {
    async fn activate_membership_effect(
        &self,
        effect: &UnfinishedMemberEffect,
    ) -> Result<(), MembershipEffectExecutionError>;
}

/// 普通成员范围已排除的设备只能收到的精确成员资料。
#[derive(Clone, PartialEq, Eq)]
pub enum RestrictedMembershipDelivery {
    Event(Box<MembershipEventV2>),
    Decision(Box<MembershipDecisionV2>),
}

impl std::fmt::Debug for RestrictedMembershipDelivery {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Event(_) => "RestrictedMembershipDelivery::Event([REDACTED])",
            Self::Decision(_) => "RestrictedMembershipDelivery::Decision([REDACTED])",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RestrictedMembershipDeliveryError {
    #[error("restricted membership delivery is deferred")]
    Deferred,
    #[error("restricted membership delivery was rejected")]
    Rejected,
}

#[async_trait]
pub trait RestrictedMembershipDeliveryPort: Send + Sync {
    async fn deliver_restricted_membership(
        &self,
        peer: &DeviceId,
        delivery: &RestrictedMembershipDelivery,
    ) -> Result<(), RestrictedMembershipDeliveryError>;
}
