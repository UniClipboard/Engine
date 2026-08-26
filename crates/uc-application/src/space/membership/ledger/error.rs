#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MembershipLedgerError {
    #[error("space is locked")]
    Locked,
    #[error("membership ledger changed")]
    Conflict,
    #[error("membership ledger is corrupt")]
    Corrupt,
    #[error("membership ledger is unavailable")]
    Unavailable,
    #[error("membership recovery is required")]
    RecoveryRequired,
}
