use crate::space::membership::MembershipLedgerError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum QueryDeviceTrustError {
    #[error("space is locked")]
    Locked,
    #[error("device trust recovery is required")]
    RecoveryRequired,
    #[error("device trust state is unavailable")]
    Unavailable,
}

impl From<MembershipLedgerError> for QueryDeviceTrustError {
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
