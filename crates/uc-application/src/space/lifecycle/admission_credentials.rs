use async_trait::async_trait;
use uc_core::crypto::domain::Passphrase;

#[derive(Debug, thiserror::Error)]
pub enum SpaceAdmissionCredentialPreparationError {
    #[error("space admission credentials are locked")]
    Locked {
        #[source]
        source: anyhow::Error,
    },
    #[error("space admission credentials require recovery")]
    RecoveryRequired {
        #[source]
        source: anyhow::Error,
    },
    #[error("space admission credentials are unavailable")]
    Unavailable {
        #[source]
        source: anyhow::Error,
    },
}

#[async_trait]
pub trait PrepareSpaceAdmissionCredentialsPort: Send + Sync {
    /// Ensures the current Space has one durable OPAQUE registration for the
    /// passphrase that was already accepted by the Space access capability.
    async fn ensure_for_unlocked_space(
        &self,
        passphrase: &Passphrase,
    ) -> Result<(), SpaceAdmissionCredentialPreparationError>;
}
