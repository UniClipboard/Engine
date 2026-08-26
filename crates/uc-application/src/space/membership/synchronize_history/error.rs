#[derive(Debug, thiserror::Error)]
pub enum SynchronizeMembershipHistoryError {
    #[error("current membership scope is unavailable")]
    CurrentScopeUnavailable,
    #[error("membership history recovery is required")]
    RecoveryRequired,
    #[error("membership history synchronization is unavailable")]
    Unavailable,
}
