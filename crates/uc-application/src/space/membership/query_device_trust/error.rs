use crate::space::membership::MembershipLedgerError;

#[derive(Debug, thiserror::Error)]
pub enum QueryDeviceTrustError {
    #[error("space is locked")]
    Locked,
    #[error("device trust recovery is required")]
    RecoveryRequired,
    #[error("device trust state is unavailable")]
    Unavailable,
    #[error("device trust dependency failed")]
    Dependency {
        #[source]
        source: anyhow::Error,
    },
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
