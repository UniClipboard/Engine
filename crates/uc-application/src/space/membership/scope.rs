use uc_core::ids::DeviceId;

use super::MembershipLedgerError;

#[async_trait::async_trait]
pub trait CurrentSpaceMemberScopePort: Send + Sync {
    async fn snapshot(&self) -> Result<CurrentSpaceMemberScope, CurrentSpaceMemberScopeError>;

    /// 通知仅使旧读取失效，接收者必须重新读取权威范围。
    fn subscribe_changes(&self) -> tokio::sync::watch::Receiver<()> {
        tokio::sync::watch::channel(()).1
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpaceMemberPauseReason {
    LocalMemberInactive,
    PendingLocalDecision,
    Diverged,
    Invalid,
    UpgradeRequired,
    RelationshipUnconfirmed,
    EffectPending,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PausedSpaceMember {
    pub device_id: DeviceId,
    pub reason: SpaceMemberPauseReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentSpaceMemberScope {
    pub revision: u64,
    pub local_member_active: bool,
    pub usable_peer_device_ids: Vec<DeviceId>,
    pub paused_peer_devices: Vec<PausedSpaceMember>,
}

#[derive(Debug, thiserror::Error)]
pub enum CurrentSpaceMemberScopeError {
    #[error("there is no current space")]
    NoCurrentSpace,
    #[error("space is locked")]
    Locked,
    #[error("membership recovery is required")]
    RecoveryRequired {
        #[source]
        source: Option<anyhow::Error>,
    },
    #[error("membership state is unavailable")]
    Unavailable,
}

/// 纯状态或输入校验失败时 `source` 为空；有下层错误时保留为来源。
impl CurrentSpaceMemberScopeError {
    pub fn recovery_required() -> Self {
        Self::RecoveryRequired { source: None }
    }

    pub fn recovery_required_from(source: impl Into<anyhow::Error>) -> Self {
        Self::RecoveryRequired {
            source: Some(source.into()),
        }
    }
}

#[cfg(test)]
impl CurrentSpaceMemberScopeError {
    /// 测试替身复用同一结果：错误不可克隆，按分类重建一个不带来源的同类错误。
    pub(crate) fn same_kind(&self) -> Self {
        match self {
            Self::NoCurrentSpace => Self::NoCurrentSpace,
            Self::Locked => Self::Locked,
            Self::RecoveryRequired { .. } => Self::recovery_required(),
            Self::Unavailable => Self::Unavailable,
        }
    }
}

impl From<MembershipLedgerError> for CurrentSpaceMemberScopeError {
    fn from(error: MembershipLedgerError) -> Self {
        match error {
            MembershipLedgerError::Locked => Self::Locked,
            error @ (MembershipLedgerError::Corrupt { .. }
            | MembershipLedgerError::RecoveryRequired) => Self::recovery_required_from(error),
            MembershipLedgerError::Conflict | MembershipLedgerError::Unavailable { .. } => {
                Self::Unavailable
            }
        }
    }
}
