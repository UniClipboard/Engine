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

impl SpaceMemberPauseReason {
    /// 资料核对与内容访问分开：仍在已验证成员范围内的异常关系允许重新证明。
    pub(crate) fn permits_history_verification(self) -> bool {
        matches!(
            self,
            Self::RelationshipUnconfirmed
                | Self::PendingLocalDecision
                | Self::UpgradeRequired
                | Self::Invalid
        )
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CurrentSpaceMemberScopeError {
    #[error("there is no current space")]
    NoCurrentSpace,
    #[error("space is locked")]
    Locked,
    #[error("membership recovery is required")]
    RecoveryRequired,
    #[error("membership state is unavailable")]
    Unavailable,
}

impl From<MembershipLedgerError> for CurrentSpaceMemberScopeError {
    fn from(error: MembershipLedgerError) -> Self {
        match error {
            MembershipLedgerError::Locked => Self::Locked,
            MembershipLedgerError::Corrupt | MembershipLedgerError::RecoveryRequired => {
                Self::RecoveryRequired
            }
            MembershipLedgerError::Conflict | MembershipLedgerError::Unavailable => {
                Self::Unavailable
            }
        }
    }
}
