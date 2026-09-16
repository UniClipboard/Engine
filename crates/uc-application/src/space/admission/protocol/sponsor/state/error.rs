#[derive(Debug, thiserror::Error)]
pub enum SponsorAdmissionStateError {
    #[error("sponsor admission state is locked")]
    Locked {
        #[source]
        source: anyhow::Error,
    },
    #[error("sponsor admission state changed")]
    StateChanged {
        #[source]
        source: anyhow::Error,
    },
    #[error("sponsor admission state requires recovery")]
    RecoveryRequired {
        #[source]
        source: anyhow::Error,
    },
    #[error("sponsor admission state is unavailable")]
    Unavailable {
        #[source]
        source: anyhow::Error,
    },
}

impl SponsorAdmissionStateError {
    pub fn locked<E: Into<anyhow::Error>>(source: E) -> Self {
        Self::Locked {
            source: source.into(),
        }
    }

    pub fn state_changed<E: Into<anyhow::Error>>(source: E) -> Self {
        Self::StateChanged {
            source: source.into(),
        }
    }

    pub fn recovery_required<E: Into<anyhow::Error>>(source: E) -> Self {
        Self::RecoveryRequired {
            source: source.into(),
        }
    }

    pub fn unavailable<E: Into<anyhow::Error>>(source: E) -> Self {
        Self::Unavailable {
            source: source.into(),
        }
    }
}
