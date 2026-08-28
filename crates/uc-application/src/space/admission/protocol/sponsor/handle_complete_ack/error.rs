#[derive(Debug, thiserror::Error)]
pub enum PrepareSponsorSettledError {
    #[error("sponsor Settled material is invalid")]
    Invalid {
        #[source]
        source: anyhow::Error,
    },
    #[error("sponsor Settled material is unavailable")]
    Unavailable {
        #[source]
        source: anyhow::Error,
    },
}

impl PrepareSponsorSettledError {
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
