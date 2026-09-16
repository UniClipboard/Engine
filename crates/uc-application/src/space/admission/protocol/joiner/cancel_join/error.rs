use crate::error::anyhow_error_constructor;

#[derive(Debug, thiserror::Error)]
pub enum JoinerCancellationStateError {
    #[error("joiner cancellation state is locked")]
    Locked {
        #[source]
        source: anyhow::Error,
    },
    #[error("joiner cancellation state changed")]
    StateChanged {
        #[source]
        source: anyhow::Error,
    },
    #[error("joiner cancellation state requires recovery")]
    RecoveryRequired {
        #[source]
        source: anyhow::Error,
    },
    #[error("joiner cancellation state is unavailable")]
    Unavailable {
        #[source]
        source: anyhow::Error,
    },
}

impl JoinerCancellationStateError {
    anyhow_error_constructor!(pub locked, Locked);
    anyhow_error_constructor!(pub state_changed, StateChanged);
    anyhow_error_constructor!(pub recovery_required, RecoveryRequired);
    anyhow_error_constructor!(pub unavailable, Unavailable);
}

#[derive(Debug, thiserror::Error)]
pub enum JoinerCancellationMaterialError {
    #[error("joiner cancellation material is unavailable")]
    Unavailable {
        #[source]
        source: anyhow::Error,
    },
}

impl JoinerCancellationMaterialError {
    anyhow_error_constructor!(pub unavailable, Unavailable);
}
