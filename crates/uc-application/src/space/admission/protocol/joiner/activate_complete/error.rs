use crate::error::anyhow_error_constructor;

#[derive(Debug, thiserror::Error)]
pub enum JoinerActivationStateError {
    #[error("joiner activation state is locked")]
    Locked {
        #[source]
        source: anyhow::Error,
    },
    #[error("joiner activation state changed")]
    StateChanged {
        #[source]
        source: anyhow::Error,
    },
    #[error("joiner activation state requires recovery")]
    RecoveryRequired {
        #[source]
        source: anyhow::Error,
    },
    #[error("joiner activation state is unavailable")]
    Unavailable {
        #[source]
        source: anyhow::Error,
    },
}

impl JoinerActivationStateError {
    anyhow_error_constructor!(locked, Locked);
    anyhow_error_constructor!(state_changed, StateChanged);
    anyhow_error_constructor!(recovery_required, RecoveryRequired);
    anyhow_error_constructor!(unavailable, Unavailable);
}

#[derive(Debug, thiserror::Error)]
pub enum ExecuteJoinerActivationError {
    #[error("joiner activation plan is invalid")]
    Invalid {
        #[source]
        source: anyhow::Error,
    },
    #[error("joiner activation is temporarily unavailable")]
    Unavailable {
        #[source]
        source: anyhow::Error,
    },
}

impl ExecuteJoinerActivationError {
    anyhow_error_constructor!(invalid, Invalid);
    anyhow_error_constructor!(unavailable, Unavailable);
}
