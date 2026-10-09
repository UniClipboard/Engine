use crate::runtime_lifecycle::LifecycleError;
use async_trait::async_trait;

use super::{
    ProfileFactoryResetCapabilityError, ProfileGeneration, ProfileLifecycle,
    ProfileLifecycleRepositoryError,
};

pub trait ProfileLifecycleRepositoryPort: Send + Sync {
    fn load(&self) -> Result<Option<ProfileLifecycle>, ProfileLifecycleRepositoryError>;

    fn compare_and_swap(
        &self,
        expected: Option<&ProfileLifecycle>,
        next: &ProfileLifecycle,
    ) -> Result<(), ProfileLifecycleRepositoryError>;
}

#[async_trait]
pub trait StopProfileRuntimePort: Send + Sync {
    async fn stop_profile_runtime(&self) -> Result<(), LifecycleError>;
}

#[async_trait]
pub trait WipeProfileKeysPort: Send + Sync {
    async fn wipe_and_verify_profile_keys(
        &self,
        profile_generation: ProfileGeneration,
    ) -> Result<(), ProfileFactoryResetCapabilityError>;
}

/// 作废本 profile 升级备份的安全记录（密文与其密钥），文件备份保持有效。
///
/// 安全记录是用被清除的密钥保护的派生副本，重置后不应继续留在备份目录。
/// 实现必须可重复执行：目录或记录不存在也算完成。
#[async_trait]
pub trait RetireUpgradeBackupSecurityRecordsPort: Send + Sync {
    async fn retire_upgrade_backup_security_records(
        &self,
    ) -> Result<(), ProfileFactoryResetCapabilityError>;
}

#[async_trait]
pub trait ClearProfileStatePort: Send + Sync {
    async fn clear_and_verify_profile_state(
        &self,
    ) -> Result<(), ProfileFactoryResetCapabilityError>;
}
