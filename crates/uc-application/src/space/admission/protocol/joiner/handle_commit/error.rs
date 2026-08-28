#[derive(Debug, thiserror::Error)]
pub enum PrepareJoinerAppliedError {
    #[error("joiner Applied material is invalid")]
    Invalid {
        #[source]
        source: anyhow::Error,
    },
    #[error("joiner Applied material is unavailable")]
    Unavailable {
        #[source]
        source: anyhow::Error,
    },
}

impl PrepareJoinerAppliedError {
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
