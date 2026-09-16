use thiserror::Error;

#[derive(Debug, Error)]
pub enum UnlockSpaceError {
    #[error("setup has not been completed")]
    SetupNotCompleted,

    #[error("space is not initialised")]
    SpaceNotInitialized,

    #[error("wrong passphrase")]
    WrongPassphrase,

    #[error("space key material corrupted")]
    CorruptedKeyMaterial,

    #[error("internal error")]
    Internal {
        #[source]
        source: anyhow::Error,
    },
}

impl UnlockSpaceError {
    pub(crate) fn internal(source: impl Into<anyhow::Error>) -> Self {
        Self::Internal {
            source: source.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::UnlockSpaceError;

    #[test]
    fn internal_failure_preserves_its_source() {
        let error = UnlockSpaceError::internal(anyhow::anyhow!("dependency failed"));

        assert!(error.source().is_some());
    }
}
