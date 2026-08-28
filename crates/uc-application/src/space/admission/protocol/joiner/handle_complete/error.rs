#[derive(Debug, thiserror::Error)]
pub enum PrepareJoinerActivationError {
    #[error("joiner activation plan is invalid")]
    Invalid {
        #[source]
        source: anyhow::Error,
    },
    #[error("joiner activation plan is unavailable")]
    Unavailable {
        #[source]
        source: anyhow::Error,
    },
}

impl PrepareJoinerActivationError {
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
