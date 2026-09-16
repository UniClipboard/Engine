use async_trait::async_trait;

use super::{
    ProfileStartupStorageError, ProfileUpgradeBackupEntry, ProfileUpgradeBackupError,
    ProfileUpgradeSource, ProfileUpgradeVersions,
};

/// 已停写资料的备份能力；实现自行安排阻塞工作，取消等待不能修改来源资料。
#[async_trait]
pub trait ProfileUpgradeBackupPort: Send + Sync {
    async fn list_backups(
        &self,
    ) -> Result<Vec<ProfileUpgradeBackupEntry>, ProfileUpgradeBackupError>;
    async fn delete_backup(&self, id: &str) -> Result<(), ProfileUpgradeBackupError>;
    fn read_source(&self) -> Result<ProfileUpgradeSource, ProfileUpgradeBackupError>;
    fn read_prepared_target(
        &self,
    ) -> Result<Option<ProfileUpgradeVersions>, ProfileUpgradeBackupError>;
    async fn capture_verified(
        &self,
        target: &ProfileUpgradeVersions,
    ) -> Result<(), ProfileUpgradeBackupError>;
    async fn verify_prepared(
        &self,
        target: &ProfileUpgradeVersions,
    ) -> Result<(), ProfileUpgradeBackupError>;
    /// 文件副本完成后才访问安全存储；失败保留副本，不允许后续升级修改。
    async fn preserve_security_materials(
        &self,
        target: &ProfileUpgradeVersions,
    ) -> Result<(), ProfileUpgradeBackupError>;
}

pub trait ProfileStartupStoragePort: Send + Sync {
    fn adopt_legacy_layout(&self) -> Result<(), ProfileStartupStorageError>;
    fn apply_pending_import(&self) -> Result<(), ProfileStartupStorageError>;
}
