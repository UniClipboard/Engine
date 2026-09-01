//! Profile V1/V2 到 V3 的唯一存储升级协调入口。
//!
//! 调用方只执行 [`ProfileStorageUpgrade::ensure_v3`]。跨进程互斥、source
//! identity 绑定、耐久 journal、一致性 snapshot、target generation、primary
//! payload 转换与重启复用都由本模块隐藏；后续切片会在同一 interface 内填入
//! 专用字段/搜索转换、验证、promotion 和清理。

mod derived_payloads;
mod journal;
mod persistence;
mod primary_payloads;
mod target;
mod validation;

#[cfg(test)]
mod field_codec_tests;

use std::sync::Arc;

use tokio::sync::Mutex;
use uc_core::ids::ProfileId;

use super::{
    ActiveRuntimeManifest, ActiveSpaceGenerationManifestStore, AdmissionKeyManager,
    ProfileContentKeyVault,
};
use crate::security::active_space_generation_manifest_store::V3ManifestPromotionOutcome;
use crate::space::InMemorySession;
use derived_payloads::DerivedPayloadConverter;
use journal::{UpgradeJournalV1, UpgradePhaseV1};
use persistence::{UpgradeLeaseResult, UpgradePersistence};
use primary_payloads::PrimaryPayloadConverter;
use target::TargetGenerationStager;
use validation::RuntimeGenerationValidator;

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
    primary_payloads: PrimaryPayloadConverter,
    derived_payloads: DerivedPayloadConverter,
    validator: RuntimeGenerationValidator,
    in_process: Mutex<()>,
}

impl ProfileStorageUpgrade {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        profile_root: std::path::PathBuf,
        source_pool: crate::db::pool::DbPool,
        source_blob_root: std::path::PathBuf,
        profile_id: ProfileId,
        source_session: Arc<InMemorySession>,
        vault: Arc<ProfileContentKeyVault>,
        keys: Arc<AdmissionKeyManager>,
        manifests: Arc<ActiveSpaceGenerationManifestStore>,
    ) -> Self {
        let primary_payloads = PrimaryPayloadConverter::new(
            source_blob_root,
            Arc::clone(&source_session),
            Arc::clone(&vault),
        );
        let derived_payloads = DerivedPayloadConverter::new(profile_id, source_session, vault);
        Self {
            persistence: UpgradePersistence::new(profile_root.clone(), keys),
            manifests,
            target: TargetGenerationStager::new(profile_root, source_pool),
            primary_payloads,
            derived_payloads,
            validator: RuntimeGenerationValidator::new(),
            in_process: Mutex::new(()),
        }
    }

    /// 确保当前 profile 使用完整 V3 存储布局。
    ///
    /// 每次调用最多推进一个 phase。`StoresSeparated` 后会在独立原子目录中转换
    /// inline/UCBL；所有阶段都保持 source 只读，完整专用字段转换前不会提升
    /// manifest。
    pub async fn ensure_v3(
        &self,
    ) -> Result<ProfileStorageUpgradeOutcome, ProfileStorageUpgradeError> {
        let _in_process = self.in_process.lock().await;
        let _lease = match self.persistence.try_acquire_lease()? {
            UpgradeLeaseResult::Acquired(lease) => lease,
            UpgradeLeaseResult::Busy => return Ok(ProfileStorageUpgradeOutcome::Busy),
        };

        let runtime_manifest = match self.manifests.load_runtime_sync() {
            Ok(source) => source,
            Err(source) => {
                return Err(ProfileStorageUpgradeError::Manifest {
                    source: anyhow::Error::new(source)
                        .context("inspect active manifest for profile storage upgrade"),
                });
            }
        };
        let persisted_journal = self.persistence.load_journal().await?;
        if let Some(ActiveRuntimeManifest::V3(target)) = runtime_manifest.as_ref() {
            let Some(mut journal) = persisted_journal else {
                return Ok(ProfileStorageUpgradeOutcome::UpToDate);
            };
            if !journal.matches_target(target) {
                return Err(ProfileStorageUpgradeError::SourceChanged);
            }
            return match journal.phase() {
                UpgradePhaseV1::Verified => {
                    self.derived_payloads.verify(&journal, &self.target).await?;
                    self.validator.verify(&journal, &self.target)?;
                    journal.mark_promoted()?;
                    self.persistence.save_journal(&journal).await?;
                    Ok(ProfileStorageUpgradeOutcome::Upgraded)
                }
                UpgradePhaseV1::Promoted => {
                    journal.mark_cleanup_pending()?;
                    self.persistence.save_journal(&journal).await?;
                    Ok(ProfileStorageUpgradeOutcome::Pending)
                }
                UpgradePhaseV1::CleanupPending => Ok(ProfileStorageUpgradeOutcome::UpToDate),
                _ => Err(ProfileStorageUpgradeError::Corrupt {
                    source: anyhow::anyhow!(
                        "active V3 manifest precedes the verified upgrade boundary"
                    ),
                }),
            };
        }
        let source = match runtime_manifest {
            Some(ActiveRuntimeManifest::V2(source)) => Some(source),
            Some(ActiveRuntimeManifest::V3(_)) => {
                return Err(ProfileStorageUpgradeError::Corrupt {
                    source: anyhow::anyhow!("profile upgrade runtime version changed while held"),
                });
            }
            None => None,
        };
        let mut journal = match persisted_journal {
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
            UpgradePhaseV1::StoresSeparated => {
                let converted = self
                    .primary_payloads
                    .convert(&journal, &self.target)
                    .await?;
                journal.mark_primary_payloads_converted(
                    converted.profile_database_digest,
                    converted.blob_tree_digest,
                    converted.inline_count,
                    converted.blob_count,
                )?;
                self.persistence.save_journal(&journal).await?;
            }
            UpgradePhaseV1::PrimaryPayloadsConverted => {
                self.primary_payloads.verify(&journal, &self.target).await?;
                let converted = self
                    .derived_payloads
                    .convert(&journal, &self.target)
                    .await?;
                journal.mark_payloads_converted(
                    converted.profile_database_digest,
                    converted.blob_tree_digest,
                    converted.derived_count,
                    converted.search_document_count,
                )?;
                self.persistence.save_journal(&journal).await?;
            }
            UpgradePhaseV1::PayloadsConverted => {
                self.derived_payloads.verify(&journal, &self.target).await?;
                let verified = self.validator.validate(&journal, &self.target)?;
                journal.mark_verified(
                    verified.profile_schema_digest,
                    verified.control_schema_digest,
                )?;
                self.persistence.save_journal(&journal).await?;
            }
            UpgradePhaseV1::Verified => {
                self.derived_payloads.verify(&journal, &self.target).await?;
                self.validator.verify(&journal, &self.target)?;
                let Some(source) = source.as_ref() else {
                    return Ok(ProfileStorageUpgradeOutcome::Pending);
                };
                let target = journal.target_manifest(source)?;
                let promotion = self
                    .manifests
                    .promote_v3_from_v2(source, &target)
                    .await
                    .map_err(|source| ProfileStorageUpgradeError::Manifest {
                        source: anyhow::Error::new(source)
                            .context("promote verified profile runtime manifest"),
                    })?;
                match promotion {
                    V3ManifestPromotionOutcome::Promoted
                    | V3ManifestPromotionOutcome::AlreadyActive => {}
                    V3ManifestPromotionOutcome::SourceChanged => {
                        return Err(ProfileStorageUpgradeError::SourceChanged);
                    }
                }
                journal.mark_promoted()?;
                self.persistence.save_journal(&journal).await?;
                return Ok(ProfileStorageUpgradeOutcome::Upgraded);
            }
            UpgradePhaseV1::Promoted | UpgradePhaseV1::CleanupPending => {
                return Err(ProfileStorageUpgradeError::SourceChanged);
            }
        }

        Ok(ProfileStorageUpgradeOutcome::Pending)
    }
}
