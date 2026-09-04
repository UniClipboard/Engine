#[derive(Debug, thiserror::Error)]
pub enum CancelInvitationError {
    #[error("no in-flight invitation to cancel")]
    NotIssued,

    #[error("pairing invitation service is unavailable")]
    Unavailable {
        #[source]
        source: anyhow::Error,
    },

    #[error("failed to cancel pairing invitation")]
    Internal {
        #[source]
        source: anyhow::Error,
    },
}

impl CancelInvitationError {
    pub(crate) fn unavailable(source: impl Into<anyhow::Error>) -> Self {
        Self::Unavailable {
            source: source.into(),
        }
    }

    pub(crate) fn internal(source: impl Into<anyhow::Error>) -> Self {
        Self::Internal {
            source: source.into(),
        }
    }
}
