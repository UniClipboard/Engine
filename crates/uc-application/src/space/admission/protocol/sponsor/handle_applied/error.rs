#[derive(Debug, thiserror::Error)]
pub enum PrepareSponsorCompleteError {
    #[error("sponsor Complete material is invalid")]
    Invalid {
        #[source]
        source: anyhow::Error,
    },
    #[error("sponsor Complete material is unavailable")]
    Unavailable {
        #[source]
        source: anyhow::Error,
    },
}

impl PrepareSponsorCompleteError {
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
