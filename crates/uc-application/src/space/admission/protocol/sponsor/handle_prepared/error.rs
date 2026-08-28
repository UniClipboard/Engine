#[derive(Debug, thiserror::Error)]
pub enum PrepareSponsorCommitError {
    #[error("sponsor commit material is invalid")]
    Invalid {
        #[source]
        source: anyhow::Error,
    },
    #[error("sponsor commit material is unavailable")]
    Unavailable {
        #[source]
        source: anyhow::Error,
    },
}

impl PrepareSponsorCommitError {
    pub fn invalid<E: Into<anyhow::Error>>(source: E) -> Self {
        Self::Invalid {
            source: source.into(),
        }
    }

    pub fn unavailable<E: Into<anyhow::Error>>(source: E) -> Self {
        Self::Unavailable {
            source: source.into(),
        }
    }
}
