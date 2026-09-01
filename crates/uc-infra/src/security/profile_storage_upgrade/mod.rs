//! Profile V1/V2 到 V3 的唯一存储升级协调入口。
//!
//! 调用方只执行 [`ProfileStorageUpgrade::ensure_v3`]。跨进程互斥、source
//! identity 绑定、耐久 journal 与重启复用都由本模块隐藏；后续切片会在同一
//! interface 内填入 snapshot、转换、验证、promotion 和清理。

mod journal;
mod persistence;

use std::sync::Arc;

use tokio::sync::Mutex;

use super::{
    ActiveSpaceGenerationManifestStore, ActiveSpaceGenerationManifestStoreError,
    AdmissionKeyManager,
};
use journal::UpgradeJournalV1;
use persistence::{UpgradeLeaseResult, UpgradePersistence};

/// 一次完整存储升级检查的稳定结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileStorageUpgradeOutcome {
    /// 当前 profile 已使用完整 V3 runtime layout。
    UpToDate,
    /// 本次调用完成了 V3 promotion。
    Upgraded,
    /// 已耐久保存恢复状态，后续切片仍需继续转换。
    Pending,
    /// 另一个进程或宿主实例正在负责该 profile 的升级。
    Busy,
}

/// Profile 存储升级的稳定失败分类。
#[derive(Debug, thiserror::Error)]
pub enum ProfileStorageUpgradeError {
    #[error("profile storage upgrade persistence is unavailable")]
    Storage {
        #[source]
        source: anyhow::Error,
    },
    #[error("profile storage upgrade security state is unavailable")]
    Security {
        #[source]
        source: anyhow::Error,
    },
    #[error("profile storage upgrade journal is corrupt")]
    Corrupt {
        #[source]
        source: anyhow::Error,
    },
    #[error("profile storage upgrade source changed during recovery")]
    SourceChanged,
    #[error("active runtime manifest cannot be inspected for storage upgrade")]
    Manifest {
        #[source]
        source: anyhow::Error,
    },
}

/// 唯一拥有 profile storage upgrade 协调与恢复的 Infra 深模块。
pub struct ProfileStorageUpgrade {
    persistence: UpgradePersistence,
    manifests: Arc<ActiveSpaceGenerationManifestStore>,
    in_process: Mutex<()>,
}

impl ProfileStorageUpgrade {
    pub fn new(
        profile_root: std::path::PathBuf,
        keys: Arc<AdmissionKeyManager>,
        manifests: Arc<ActiveSpaceGenerationManifestStore>,
    ) -> Self {
        Self {
            persistence: UpgradePersistence::new(profile_root, keys),
            manifests,
            in_process: Mutex::new(()),
        }
    }

    /// 确保当前 profile 使用完整 V3 存储布局。
    ///
    /// 本切片只建立协调与恢复基础：V2 或空 profile 会创建/复用加密 journal
    /// 并返回 `Pending`；不会修改 source、写 V3 payload 或提升 manifest。
    pub async fn ensure_v3(
        &self,
    ) -> Result<ProfileStorageUpgradeOutcome, ProfileStorageUpgradeError> {
        let _in_process = self.in_process.lock().await;
        let _lease = match self.persistence.try_acquire_lease()? {
            UpgradeLeaseResult::Acquired(lease) => lease,
            UpgradeLeaseResult::Busy => return Ok(ProfileStorageUpgradeOutcome::Busy),
        };

        let source = match self.manifests.load_sync() {
            Ok(source) => source,
            Err(ActiveSpaceGenerationManifestStoreError::UnsupportedVersion) => {
                return Ok(ProfileStorageUpgradeOutcome::UpToDate);
            }
            Err(source) => {
                return Err(ProfileStorageUpgradeError::Manifest {
                    source: anyhow::Error::new(source)
                        .context("inspect active manifest for profile storage upgrade"),
                });
            }
        };
        match self.persistence.load_journal().await? {
            Some(journal) => {
                if !journal.matches_source(source.as_ref()) {
                    return Err(ProfileStorageUpgradeError::SourceChanged);
                }
            }
            None => {
                self.persistence
                    .save_new_journal(&UpgradeJournalV1::detected(source.as_ref()))
                    .await?;
            }
        }

        Ok(ProfileStorageUpgradeOutcome::Pending)
    }
}
