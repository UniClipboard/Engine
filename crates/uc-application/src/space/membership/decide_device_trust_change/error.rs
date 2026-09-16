#[derive(Debug, thiserror::Error)]
pub enum DecideDeviceTrustChangeError {
    #[error("space is locked")]
    Locked,
    #[error("device trust recovery is required")]
    RecoveryRequired,
    #[error("device trust decision is unavailable")]
    Unavailable,
    #[error("device trust state changed")]
    StateChanged,
    #[error("device trust decision was committed but follow-up is pending")]
    CommittedButPending,
}
