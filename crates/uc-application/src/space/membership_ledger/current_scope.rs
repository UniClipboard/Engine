use uc_core::ids::DeviceId;

use super::MembershipLedgerError;

#[async_trait::async_trait]
pub trait CurrentSpaceMemberScopePort: Send + Sync {
    async fn snapshot(&self) -> Result<CurrentSpaceMemberScope, CurrentSpaceMemberScopeError>;
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
