use async_trait::async_trait;

#[derive(Debug, thiserror::Error)]
pub enum LegacyMigrationRecoveryError {
    #[error("legacy migration requires manual recovery")]
    RecoveryRequired,
    #[error("legacy migration recovery failed: {0}")]
    Internal(String),
}

#[async_trait]
pub trait LegacyMigrationRecoveryPort: Send + Sync {
    async fn recover(&self) -> Result<(), LegacyMigrationRecoveryError>;
}
