//! Profile V1/V2 到 V3 的唯一存储升级协调入口。
//!
//! 调用方只执行 [`ProfileStorageUpgrade::ensure_v3`]。本文件保留互斥、升级顺序
//! 与持久恢复判断；启动准备由 `bootstrap` 负责，已验证旧资料的清理由 `cleanup`
//! 负责，进度摘要与重建由 `progress` 负责。内部步骤不向宿主开放。

mod bootstrap;
mod cleanup;
mod derived_payloads;
mod diagnostics;
mod journal;
mod persistence;
mod primary_payloads;
mod progress;
mod target;
mod validation;

#[cfg(test)]
mod field_codec_tests;

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Mutex;

use super::{ActiveRuntimeManifest, ActiveSpaceGenerationManifestStore};
use crate::security::active_space_generation_manifest_store::V3ManifestPromotionOutcome;
use bootstrap::RuntimeUpgradeBootstrap;
use derived_payloads::DerivedPayloadConverter;
use diagnostics::UpgradeDiagnostics;
use journal::{UpgradeJournalV1, UpgradePhaseV1};
use persistence::{UpgradeLeaseResult, UpgradePersistence};
use primary_payloads::PrimaryPayloadConverter;
use progress::UpgradeProgress;
pub use progress::{
    StorageUpgradeFailure, StorageUpgradeObserver, StorageUpgradeProgressOutcome,
    StorageUpgradeSnapshot, StorageUpgradeStep, StorageUpgradeStepProgress, StorageUpgradeUnit,
};
use target::TargetGenerationStager;
use validation::RuntimeGenerationValidator;

/// 一次完整存储升级检查的稳定结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileStorageUpgradeOutcome {
    /// 当前 profile 已使用完整 V3 runtime layout。
    UpToDate,
    /// 本次调用完成了 V3 promotion。
    Upgraded,
    /// 空 profile 已准备好首个 V3 data/control generation，等待首次 Space 激活。
    FreshReady {
        profile_data_generation: [u8; 16],
        space_control_generation: [u8; 16],
    },
    /// A legacy profile was converted and is ready to expose under its existing Space.
    LegacyReady {
        profile_data_generation: [u8; 16],
        space_control_generation: [u8; 16],
        space_id: uc_core::ids::SpaceId,
    },
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

struct UpgradeComponents {
    target: TargetGenerationStager,
    primary_payloads: Option<PrimaryPayloadConverter>,
    derived_payloads: Option<DerivedPayloadConverter>,
    legacy_space_id: Option<uc_core::ids::SpaceId>,
}

enum UpgradeMode {
    Prepared(UpgradeComponents),
    Runtime(RuntimeUpgradeBootstrap),
}

/// 唯一拥有 profile storage upgrade 协调与恢复的 Infra 深模块。
pub struct ProfileStorageUpgrade {
    profile_root: PathBuf,
    legacy_database: PathBuf,
    legacy_blob_root: PathBuf,
    persistence: UpgradePersistence,
    manifests: Arc<ActiveSpaceGenerationManifestStore>,
    mode: UpgradeMode,
    validator: RuntimeGenerationValidator,
    in_process: Mutex<()>,
    max_steps_per_call: Option<usize>,
    progress: Option<Arc<dyn StorageUpgradeObserver>>,
}

impl ProfileStorageUpgrade {
    /// 只接入进度摘要，不改变升级和恢复的所有权。
    pub fn with_progress(mut self, observer: Arc<dyn StorageUpgradeObserver>) -> Self {
        self.progress = Some(observer);
        self
    }

    /// 确保当前 profile 使用完整 V3 存储布局。
    ///
    /// production 构造器会在一次调用内推进到 V3 promotion 或 Fresh-ready；
    /// 只有测试故障注入构造器会在单个耐久 phase 后返回 `Pending`。
    pub async fn ensure_v3(
        &self,
    ) -> Result<ProfileStorageUpgradeOutcome, ProfileStorageUpgradeError> {
        let mut diagnostic = UpgradeDiagnostics::new();
        let progress = UpgradeProgress::new(self.progress.clone());
        progress.begin(StorageUpgradeStep::Checking, None, None);
        let result = self.ensure_locked(&mut diagnostic, &progress).await;
        if let Err(error) = &result {
            diagnostic.record_failure(error);
        }
        progress.finish(&result);
        result
    }

    async fn ensure_locked(
        &self,
        diagnostic: &mut UpgradeDiagnostics,
        progress: &UpgradeProgress,
    ) -> Result<ProfileStorageUpgradeOutcome, ProfileStorageUpgradeError> {
        let _in_process = self.in_process.lock().await;
        let _lease = match self.persistence.try_acquire_lease()? {
            UpgradeLeaseResult::Acquired(lease) => lease,
            UpgradeLeaseResult::Busy => return Ok(ProfileStorageUpgradeOutcome::Busy),
        };

        match &self.mode {
            UpgradeMode::Prepared(components) => {
                self.ensure_with(components, diagnostic, progress).await
            }
            UpgradeMode::Runtime(bootstrap) => {
                diagnostic.action = "prepare_source";
                let components = bootstrap
                    .prepare(
                        &self.profile_root,
                        &self.legacy_database,
                        &self.legacy_blob_root,
                        &self.manifests,
                        progress,
                    )
                    .await?;
                self.ensure_with(&components, diagnostic, progress).await
            }
        }
    }

    async fn ensure_with(
        &self,
        components: &UpgradeComponents,
        diagnostic: &mut UpgradeDiagnostics,
        progress: &UpgradeProgress,
    ) -> Result<ProfileStorageUpgradeOutcome, ProfileStorageUpgradeError> {
        let mut steps = 0_usize;
        loop {
            let outcome = self
                .advance_once(components, steps == 0, diagnostic, progress)
                .await?;
            steps += 1;
            if outcome != ProfileStorageUpgradeOutcome::Pending
                || self
                    .max_steps_per_call
                    .is_some_and(|maximum| steps >= maximum)
            {
                return Ok(outcome);
            }
        }
    }

    async fn advance_once(
        &self,
        components: &UpgradeComponents,
        resuming: bool,
        diagnostic: &mut UpgradeDiagnostics,
        progress: &UpgradeProgress,
    ) -> Result<ProfileStorageUpgradeOutcome, ProfileStorageUpgradeError> {
        diagnostic.action = "inspect_manifest";
        let runtime_manifest = match self.manifests.load_runtime_sync() {
            Ok(source) => source,
            Err(source) => {
                return Err(ProfileStorageUpgradeError::Manifest {
                    source: anyhow::Error::new(source)
                        .context("inspect active manifest for profile storage upgrade"),
                });
            }
        };
        diagnostic.target_activated = Some(matches!(
            runtime_manifest,
            Some(ActiveRuntimeManifest::V3(_))
        ));
        diagnostic.action = "load_journal";
        let persisted_journal = self.persistence.load_journal().await?;
        if resuming {
            progress.required(
                !matches!(runtime_manifest, Some(ActiveRuntimeManifest::V3(_)))
                    && (matches!(runtime_manifest, Some(ActiveRuntimeManifest::V2(_)))
                        || components.legacy_space_id.is_some()
                        || persisted_journal.as_ref().is_some_and(|journal| {
                            journal.source_space_id().is_some()
                                || journal
                                    .converted_inline_count()
                                    .is_some_and(|count| count > 0)
                                || journal
                                    .converted_blob_count()
                                    .is_some_and(|count| count > 0)
                                || journal
                                    .converted_derived_count()
                                    .is_some_and(|count| count > 0)
                        })),
            );
            if let Some(journal) = &persisted_journal {
                progress.recovering();
                progress.restore_from_journal(journal);
            }
        }
        diagnostic.phase = persisted_journal.as_ref().map(UpgradeJournalV1::phase);
        if let Some(ActiveRuntimeManifest::V3(target)) = runtime_manifest.as_ref() {
            diagnostic.action = "recover_active_target";
            let Some(mut journal) = persisted_journal else {
                progress.complete(StorageUpgradeStep::Checking);
                return Ok(ProfileStorageUpgradeOutcome::UpToDate);
            };
            progress.begin(StorageUpgradeStep::Preparing, None, None);
            if !journal.matches_target(target) {
                if journal.matches_activated_fresh_profile(target) {
                    self.cleanup(&journal, &components.target)?;
                    self.persistence.clear_journal().await?;
                    progress.complete(StorageUpgradeStep::Preparing);
                    return Ok(ProfileStorageUpgradeOutcome::UpToDate);
                }
                return Err(ProfileStorageUpgradeError::SourceChanged);
            }
            return match journal.phase() {
                UpgradePhaseV1::Verified => {
                    self.validator.verify_promoted(
                        &journal,
                        &components.target,
                        journal.source_space_id().is_some(),
                    )?;
                    journal.mark_promoted()?;
                    self.persistence.save_journal(&journal).await?;
                    Ok(ProfileStorageUpgradeOutcome::Pending)
                }
                UpgradePhaseV1::Promoted => {
                    self.validator
                        .verify_promoted(&journal, &components.target, false)?;
                    journal.mark_cleanup_pending()?;
                    self.persistence.save_journal(&journal).await?;
                    Ok(ProfileStorageUpgradeOutcome::Pending)
                }
                UpgradePhaseV1::CleanupPending => {
                    self.validator
                        .verify_promoted(&journal, &components.target, false)?;
                    self.cleanup(&journal, &components.target)?;
                    self.persistence.clear_journal().await?;
                    progress.complete(StorageUpgradeStep::Preparing);
                    Ok(ProfileStorageUpgradeOutcome::UpToDate)
                }
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
                diagnostic.action = "verify_source";
                if !journal.matches_source(source.as_ref()) {
                    return Err(ProfileStorageUpgradeError::SourceChanged);
                }
                // 恢复入口只重建未提交且不再完整的准备副本；原始资料仍须通过身份与修订校验。
                if resuming
                    && !matches!(
                        journal.phase(),
                        UpgradePhaseV1::Promoted | UpgradePhaseV1::CleanupPending
                    )
                    && (components.target.source_changed(&journal)?
                        || (journal.phase() == UpgradePhaseV1::TargetStaged
                            && !components.target.staged_snapshot_matches(&journal)?))
                {
                    diagnostic.action = "save_restart_plan";
                    self.persistence
                        .save_journal(&journal.restart(source.as_ref()))
                        .await?;
                    progress.restart();
                    return Ok(ProfileStorageUpgradeOutcome::Pending);
                }
                journal
            }
            None => {
                diagnostic.action = "create_upgrade_plan";
                self.persistence
                    .save_new_journal(&UpgradeJournalV1::detected(source.as_ref()))
                    .await?;
                return Ok(ProfileStorageUpgradeOutcome::Pending);
            }
        };
        diagnostic.begin_step(journal.phase());
        match journal.phase() {
            UpgradePhaseV1::Detected => {
                let staged = components.target.stage(&journal)?;
                journal.mark_target_staged(
                    staged.source_snapshot_digest,
                    staged.source_database_revision,
                )?;
                self.persistence.save_journal(&journal).await?;
            }
            UpgradePhaseV1::TargetStaged => {
                let separated = components.target.separate(&journal, source.as_ref())?;
                journal.mark_stores_separated(
                    separated.profile_database_digest,
                    separated.control_database_digest,
                )?;
                self.persistence.save_journal(&journal).await?;
                progress.complete(StorageUpgradeStep::Checking);
            }
            UpgradePhaseV1::StoresSeparated => {
                let converted = self
                    .primary_payloads(components)?
                    .convert(&journal, &components.target, progress)
                    .await?;
                journal.mark_primary_payloads_converted(
                    converted.profile_database_digest,
                    converted.blob_tree_digest,
                    converted.inline_count,
                    converted.blob_count,
                )?;
                journal.record_primary_warnings(converted.warning_count);
                self.persistence.save_journal(&journal).await?;
                progress.complete(StorageUpgradeStep::Contents);
                progress.complete(StorageUpgradeStep::LargeContents);
            }
            UpgradePhaseV1::PrimaryPayloadsConverted => {
                progress.begin(StorageUpgradeStep::Verifying, None, None);
                self.primary_payloads(components)?
                    .verify(&journal, &components.target)
                    .await?;
                let converted = self
                    .derived_payloads(components)?
                    .convert(&journal, &components.target, progress)
                    .await?;
                journal.mark_payloads_converted(
                    converted.profile_database_digest,
                    converted.blob_tree_digest,
                    converted.derived_count,
                    converted.search_document_count,
                )?;
                journal.record_derived_warnings(converted.warning_count);
                self.persistence.save_journal(&journal).await?;
                progress.complete(StorageUpgradeStep::RelatedRecords);
            }
            UpgradePhaseV1::PayloadsConverted => {
                progress.begin(StorageUpgradeStep::Verifying, None, None);
                self.derived_payloads(components)?
                    .verify(&journal, &components.target)
                    .await?;
                let verified = self.validator.validate(&journal, &components.target)?;
                journal.mark_verified(
                    verified.profile_schema_digest,
                    verified.control_schema_digest,
                )?;
                self.persistence.save_journal(&journal).await?;
                progress.complete(StorageUpgradeStep::Verifying);
            }
            UpgradePhaseV1::Verified => {
                progress.begin(StorageUpgradeStep::Preparing, None, None);
                if source.is_none() {
                    progress.complete(StorageUpgradeStep::Preparing);
                    if let Some(space_id) = components.legacy_space_id.clone() {
                        return Ok(ProfileStorageUpgradeOutcome::LegacyReady {
                            profile_data_generation: *journal.target_profile_data_generation(),
                            space_control_generation: *journal.target_space_control_generation(),
                            space_id,
                        });
                    }
                    return Ok(ProfileStorageUpgradeOutcome::FreshReady {
                        profile_data_generation: *journal.target_profile_data_generation(),
                        space_control_generation: *journal.target_space_control_generation(),
                    });
                }
                self.derived_payloads(components)?
                    .verify(&journal, &components.target)
                    .await?;
                self.validator.verify(&journal, &components.target)?;
                let source = source.as_ref().expect("source checked above");
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
                progress.complete(StorageUpgradeStep::Preparing);
                return Ok(ProfileStorageUpgradeOutcome::Upgraded);
            }
            UpgradePhaseV1::Promoted | UpgradePhaseV1::CleanupPending => {
                return Err(ProfileStorageUpgradeError::SourceChanged);
            }
        }

        Ok(ProfileStorageUpgradeOutcome::Pending)
    }

    fn primary_payloads<'a>(
        &self,
        components: &'a UpgradeComponents,
    ) -> Result<&'a PrimaryPayloadConverter, ProfileStorageUpgradeError> {
        components
            .primary_payloads
            .as_ref()
            .ok_or_else(cleanup_source_unavailable)
    }

    fn derived_payloads<'a>(
        &self,
        components: &'a UpgradeComponents,
    ) -> Result<&'a DerivedPayloadConverter, ProfileStorageUpgradeError> {
        components
            .derived_payloads
            .as_ref()
            .ok_or_else(cleanup_source_unavailable)
    }
}

fn cleanup_source_unavailable() -> ProfileStorageUpgradeError {
    ProfileStorageUpgradeError::Corrupt {
        source: anyhow::anyhow!("profile upgrade source is unavailable after promotion"),
    }
}
