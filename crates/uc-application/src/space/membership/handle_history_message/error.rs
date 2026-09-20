#[derive(Debug, thiserror::Error)]
pub enum HandleMembershipHistoryMessageError {
    #[error("space pairing is still in progress")]
    PairingInProgress,
    #[error("space is locked")]
    Locked,
    #[error("membership history recovery is required")]
    RecoveryRequired,
    #[error("membership history message is rejected")]
    Rejected,
    #[error("membership history handling is unavailable")]
    Unavailable,
}
