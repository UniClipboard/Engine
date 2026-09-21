#[derive(Debug, thiserror::Error)]
pub enum PrepareJoinerActivationError {
    #[error("joiner activation plan is invalid")]
    Invalid {
        reason: uc_core::membership::SpaceAdmissionRejectionReason,
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
            reason: uc_core::membership::SpaceAdmissionRejectionReason::HistoryConflict,
            source: source.into(),
        }
    }

    pub fn invalid_for<E: Into<anyhow::Error>>(
        reason: uc_core::membership::SpaceAdmissionRejectionReason,
        source: E,
    ) -> Self {
        Self::Invalid {
            reason,
            source: source.into(),
        }
    }

    pub fn unavailable<E: Into<anyhow::Error>>(source: E) -> Self {
        Self::Unavailable {
            source: source.into(),
        }
    }
}
