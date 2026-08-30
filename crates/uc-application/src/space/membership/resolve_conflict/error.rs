#[derive(Debug, thiserror::Error)]
pub enum ResolveMembershipConflictError {
    #[error("space is locked")]
    Locked {
        #[source]
        source: anyhow::Error,
    },
    #[error("the selected membership branch is invalid")]
    InvalidChoice,
    #[error("the target membership branch is unavailable")]
    TargetUnavailable {
        #[source]
        source: anyhow::Error,
    },
    #[error("membership conflict recovery is required")]
    RecoveryRequired {
        #[source]
        source: anyhow::Error,
    },
    #[error("the membership conflict choice was committed but follow-up is pending")]
    CommittedButPending {
        #[source]
        source: anyhow::Error,
    },
}
