use crate::profile::factory_reset::ProfileLifecycleRepositoryError;

#[derive(Debug, thiserror::Error)]
#[error("profile upgrade backup failed")]
pub struct ProfileUpgradeBackupError {
    #[source]
    pub source: anyhow::Error,
}

#[derive(Debug, thiserror::Error)]
#[error("profile startup storage preparation failed")]
pub struct ProfileStartupStorageError {
    #[source]
    pub source: anyhow::Error,
}

#[derive(Debug, thiserror::Error)]
pub enum ProfileStartupError {
    #[error("profile startup backup failed")]
    Backup(#[from] ProfileUpgradeBackupError),
    #[error("profile startup storage preparation failed")]
    Storage(#[from] ProfileStartupStorageError),
    #[error("profile startup lifecycle preparation failed")]
    Lifecycle(#[from] ProfileLifecycleRepositoryError),
}
