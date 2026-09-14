#[derive(Debug, thiserror::Error)]
pub enum ChangeEncryptionPassphraseError {
    #[error("passphrase confirmation does not match")]
    PassphraseMismatch,
    #[error("space is locked")]
    Locked,
    #[error("passphrase change is only available when this device is the only member")]
    MultipleDevices,
    #[error("membership recovery is required")]
    MembershipRecoveryRequired,
    #[error("membership state is unavailable")]
    MembershipUnavailable,
    #[error("pairing invitations could not be retired")]
    Invitation {
        #[source]
        source: anyhow::Error,
    },
    #[error("passphrase change requires recovery")]
    RecoveryRequired {
        #[source]
        source: anyhow::Error,
    },
    #[error("passphrase change is unavailable")]
    Unavailable {
        #[source]
        source: anyhow::Error,
    },
}

impl ChangeEncryptionPassphraseError {
    pub(super) fn invitation(source: impl Into<anyhow::Error>) -> Self {
        Self::Invitation {
            source: source.into(),
        }
    }

    pub(super) fn recovery(source: impl Into<anyhow::Error>) -> Self {
        Self::RecoveryRequired {
            source: source.into(),
        }
    }

    pub(super) fn unavailable(source: impl Into<anyhow::Error>) -> Self {
        Self::Unavailable {
            source: source.into(),
        }
    }
}
