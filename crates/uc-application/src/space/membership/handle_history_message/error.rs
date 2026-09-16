#[derive(Debug, thiserror::Error)]
pub enum HandleMembershipHistoryMessageError {
    #[error("space is locked")]
    Locked,
    #[error("membership history recovery is required")]
    RecoveryRequired,
    #[error("membership history message is rejected")]
    Rejected,
    #[error("membership history handling is unavailable")]
    Unavailable,
}
