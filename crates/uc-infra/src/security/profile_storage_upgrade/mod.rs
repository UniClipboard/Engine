//! Profile V1/V2 到 V3 的唯一存储升级协调入口。
//!
//! 调用方只执行 [`ProfileStorageUpgrade::ensure_v3`]。跨进程互斥、source
//! identity 绑定、耐久 journal、一致性 snapshot、target generation 与重启
//! 复用都由本模块隐藏；后续切片会在同一 interface 内填入表拆分、转换、
//! 验证、promotion 和清理。

mod journal;
mod persistence;
mod target;

use std::sync::Arc;

use tokio::sync::Mutex;

use super::{
    ActiveSpaceGenerationManifestStore, ActiveSpaceGenerationManifestStoreError,
    AdmissionKeyManager,
};
use journal::{UpgradeJournalV1, UpgradePhaseV1};
use persistence::{UpgradeLeaseResult, UpgradePersistence};
use target::TargetGenerationStager;

/// 一次完整存储升级检查的稳定结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileStorageUpgradeOutcome {
    /// 当前 profile 已使用完整 V3 runtime layout。
    UpToDate,
    /// 本次调用完成了 V3 promotion。
    Upgraded,
    /// 已耐久推进一个恢复阶段，尚未完成 promotion。
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
    target: TargetGenerationStager,
    in_process: Mutex<()>,
}

impl ProfileStorageUpgrade {
    pub fn new(
        profile_root: std::path::PathBuf,
        source_pool: crate::db::pool::DbPool,
        keys: Arc<AdmissionKeyManager>,
        manifests: Arc<ActiveSpaceGenerationManifestStore>,
    ) -> Self {
        Self {
            persistence: UpgradePersistence::new(profile_root.clone(), keys),
            manifests,
            target: TargetGenerationStager::new(profile_root, source_pool),
            in_process: Mutex::new(()),
        }
    }

    /// 确保当前 profile 使用完整 V3 存储布局。
    ///
    /// V2 或空 profile 先创建加密 journal，下一次调用形成一致性 snapshot 并
    /// 耐久进入 `TargetStaged`。每次调用最多推进一个 phase；不会修改 source、
    /// 写 V3 payload 或提升 manifest。
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
        let mut journal = match self.persistence.load_journal().await? {
            Some(journal) => {
                if !journal.matches_source(source.as_ref()) {
                    return Err(ProfileStorageUpgradeError::SourceChanged);
                }
                journal
            }
            None => {
                self.persistence
                    .save_new_journal(&UpgradeJournalV1::detected(source.as_ref()))
                    .await?;
                return Ok(ProfileStorageUpgradeOutcome::Pending);
            }
        };
        match journal.phase() {
            UpgradePhaseV1::Detected => {
                let staged = self.target.stage(&journal)?;
                journal.mark_target_staged(
                    staged.source_snapshot_digest,
                    staged.source_database_revision,
                )?;
                self.persistence.save_journal(&journal).await?;
            }
            UpgradePhaseV1::TargetStaged => {
                let separated = self.target.separate(&journal)?;
                journal.mark_stores_separated(
                    separated.profile_database_digest,
                    separated.control_database_digest,
                )?;
                self.persistence.save_journal(&journal).await?;
            }
            UpgradePhaseV1::StoresSeparated => self.target.verify_separated(&journal)?,
            UpgradePhaseV1::PayloadsConverted
            | UpgradePhaseV1::Verified
            | UpgradePhaseV1::Promoted
            | UpgradePhaseV1::CleanupPending => {}
        }

        Ok(ProfileStorageUpgradeOutcome::Pending)
    }
}
