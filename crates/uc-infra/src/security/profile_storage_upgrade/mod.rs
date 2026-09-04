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

use std::path::{Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use uc_core::ids::ProfileId;
use uc_core::ports::space::SpaceAccessStore as _;
use uc_core::ports::SecureStoragePort;

use super::{
    ActiveRuntimeManifest, ActiveSpaceGenerationManifestStore, AdmissionKeyManager,
    ProfileContentKeyVault,
};
use crate::security::active_space_generation_manifest_store::V3ManifestPromotionOutcome;
use crate::space::{InMemorySession, KeyMaterialStore, RuntimeSpaceAccessAdapter};
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
    /// 空 profile 已准备好首个 V3 data/control generation，等待首次 Space 激活。
    FreshReady {
        profile_data_generation: [u8; 16],
        space_control_generation: [u8; 16],
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
}

struct RuntimeUpgradeBootstrap {
    profile_id: ProfileId,
    secure_storage: Arc<dyn SecureStoragePort>,
    vault_path: PathBuf,
    vault: Arc<ProfileContentKeyVault>,
    keys: Arc<AdmissionKeyManager>,
}

enum UpgradeMode {
    Prepared(UpgradeComponents),
    Runtime(RuntimeUpgradeBootstrap),
}

impl RuntimeUpgradeBootstrap {
    async fn prepare(
        &self,
        profile_root: &Path,
        legacy_database: &Path,
        legacy_blob_root: &Path,
        manifests: &ActiveSpaceGenerationManifestStore,
    ) -> Result<UpgradeComponents, ProfileStorageUpgradeError> {
        let active = manifests.load_runtime_sync().map_err(|source| {
            ProfileStorageUpgradeError::Manifest {
                source: anyhow::Error::new(source)
                    .context("inspect active manifest after acquiring the upgrade lease"),
            }
        })?;
        if matches!(active, Some(ActiveRuntimeManifest::V3(_))) {
            return Ok(UpgradeComponents {
                target: TargetGenerationStager::cleanup_only(
                    profile_root.to_path_buf(),
                    Arc::clone(&self.keys),
                ),
                primary_payloads: None,
                derived_payloads: None,
            });
        }

        let (source_database, source_blob_root, source_space_id) = match active.as_ref() {
            Some(ActiveRuntimeManifest::V2(source)) => {
                let source_root = legacy_space_generation_directory(
                    &profile_root.join("space-generations"),
                    &source.space_id,
                    &source.database_generation,
                );
                let database = source_root.join("target.sqlite");
                if !database.is_file() {
                    return Err(ProfileStorageUpgradeError::Storage {
                        source: anyhow::anyhow!(
                            "profile storage upgrade source database is missing"
                        ),
                    });
                }
                (
                    database,
                    source_root.join("blobs"),
                    Some(source.space_id.clone()),
                )
            }
            Some(ActiveRuntimeManifest::V3(_)) => {
                return Err(ProfileStorageUpgradeError::Corrupt {
                    source: anyhow::anyhow!("profile storage version changed during bootstrap"),
                });
            }
            None => (
                legacy_database.to_path_buf(),
                legacy_blob_root.to_path_buf(),
                None,
            ),
        };
        if let Some(parent) = source_database.parent() {
            std::fs::create_dir_all(parent).map_err(|source| {
                ProfileStorageUpgradeError::Storage {
                    source: anyhow::Error::new(source)
                        .context("prepare profile storage upgrade source directory"),
                }
            })?;
        }
        let database =
            source_database
                .to_str()
                .ok_or_else(|| ProfileStorageUpgradeError::Storage {
                    source: anyhow::anyhow!("profile storage upgrade source path is invalid"),
                })?;
        let source_pool = crate::db::pool::init_db_pool(database).map_err(|source| {
            ProfileStorageUpgradeError::Storage {
                source: source.context("open profile storage upgrade source database"),
            }
        })?;
        let source_session = Arc::new(InMemorySession::new());
        if let Some(source_space_id) = source_space_id {
            let current_profile: Arc<
                dyn uc_core::ports::security::current_profile::CurrentProfilePort,
            > = Arc::new(super::DefaultCurrentProfile::for_profile(
                self.profile_id.clone(),
            ));
            let keyslot_store: Arc<dyn crate::fs::key_slot_store::KeySlotStore> = Arc::new(
                crate::fs::key_slot_store::JsonKeySlotStore::new(self.vault_path.clone()),
            );
            let key_material = Arc::new(KeyMaterialStore::new(
                Arc::clone(&self.secure_storage),
                keyslot_store,
            ));
            let executor = Arc::new(crate::db::executor::DieselSqliteExecutor::new(
                source_pool.clone(),
            ));
            let security_repository =
                Arc::new(crate::db::repositories::DieselSpaceSecurityStore::new(
                    executor,
                    source_session.as_ref().clone(),
                ));
            let access = RuntimeSpaceAccessAdapter::new(
                key_material,
                current_profile,
                Arc::clone(&source_session),
                security_repository.clone(),
                security_repository,
                Arc::clone(&self.vault),
            );
            let resumed = access
                .try_resume_session(&uc_core::ids::SpaceId::from_string(source_space_id))
                .await
                .map_err(|source| ProfileStorageUpgradeError::Security {
                    source: anyhow::Error::new(source)
                        .context("resume source security session for profile storage upgrade"),
                })?;
            if resumed.is_none() {
                return Err(ProfileStorageUpgradeError::Security {
                    source: anyhow::anyhow!(
                        "profile storage upgrade source security session is locked"
                    ),
                });
            }
        }

        Ok(UpgradeComponents {
            target: TargetGenerationStager::new(
                profile_root.to_path_buf(),
                source_pool,
                Arc::clone(&self.keys),
            ),
            primary_payloads: Some(PrimaryPayloadConverter::new(
                source_blob_root,
                Arc::clone(&source_session),
                Arc::clone(&self.vault),
            )),
            derived_payloads: Some(DerivedPayloadConverter::new(
                self.profile_id.clone(),
                source_session,
                Arc::clone(&self.vault),
            )),
        })
    }
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
}

impl ProfileStorageUpgrade {
    /// 为 Engine 启动构造唯一升级负责人。
    ///
    /// V2 source 定位、最小安全 repository、静默 MasterKey/session 恢复与
    /// cleanup-only 选择都留在模块内部；调用方不能组装旧 reader。
    #[allow(clippy::too_many_arguments)]
    pub fn for_runtime(
        profile_root: PathBuf,
        legacy_database: PathBuf,
        legacy_blob_root: PathBuf,
        profile_id: ProfileId,
        secure_storage: Arc<dyn SecureStoragePort>,
        vault_path: PathBuf,
        vault: Arc<ProfileContentKeyVault>,
        keys: Arc<AdmissionKeyManager>,
        manifests: Arc<ActiveSpaceGenerationManifestStore>,
    ) -> Self {
        Self {
            legacy_database,
            legacy_blob_root,
            persistence: UpgradePersistence::new(profile_root.clone(), Arc::clone(&keys)),
            manifests,
            mode: UpgradeMode::Runtime(RuntimeUpgradeBootstrap {
                profile_id,
                secure_storage,
                vault_path,
                vault,
                keys,
            }),
            profile_root,
            validator: RuntimeGenerationValidator::new(),
            in_process: Mutex::new(()),
            max_steps_per_call: None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        profile_root: PathBuf,
        source_pool: crate::db::pool::DbPool,
        source_blob_root: std::path::PathBuf,
        profile_id: ProfileId,
        source_session: Arc<InMemorySession>,
        vault: Arc<ProfileContentKeyVault>,
        keys: Arc<AdmissionKeyManager>,
        manifests: Arc<ActiveSpaceGenerationManifestStore>,
    ) -> Self {
        Self::with_source(
            profile_root,
            source_pool,
            source_blob_root,
            profile_id,
            source_session,
            vault,
            keys,
            manifests,
            None,
        )
    }

    /// 集成测试故障注入入口：每次只推进一个耐久 phase，以模拟进程退出。
    #[doc(hidden)]
    #[allow(clippy::too_many_arguments)]
    pub fn new_stepwise_for_testing(
        profile_root: PathBuf,
        source_pool: crate::db::pool::DbPool,
        source_blob_root: PathBuf,
        profile_id: ProfileId,
        source_session: Arc<InMemorySession>,
        vault: Arc<ProfileContentKeyVault>,
        keys: Arc<AdmissionKeyManager>,
        manifests: Arc<ActiveSpaceGenerationManifestStore>,
    ) -> Self {
        Self::with_source(
            profile_root,
            source_pool,
            source_blob_root,
            profile_id,
            source_session,
            vault,
            keys,
            manifests,
            Some(1),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn with_source(
        profile_root: PathBuf,
        source_pool: crate::db::pool::DbPool,
        source_blob_root: PathBuf,
        profile_id: ProfileId,
        source_session: Arc<InMemorySession>,
        vault: Arc<ProfileContentKeyVault>,
        keys: Arc<AdmissionKeyManager>,
        manifests: Arc<ActiveSpaceGenerationManifestStore>,
        max_steps_per_call: Option<usize>,
    ) -> Self {
        let primary_payloads = PrimaryPayloadConverter::new(
            source_blob_root.clone(),
            Arc::clone(&source_session),
            Arc::clone(&vault),
        );
        let derived_payloads = DerivedPayloadConverter::new(profile_id, source_session, vault);
        Self {
            legacy_database: profile_root.join("uniclipboard.db"),
            legacy_blob_root: source_blob_root,
            persistence: UpgradePersistence::new(profile_root.clone(), Arc::clone(&keys)),
            manifests,
            mode: UpgradeMode::Prepared(UpgradeComponents {
                target: TargetGenerationStager::new(profile_root.clone(), source_pool, keys),
                primary_payloads: Some(primary_payloads),
                derived_payloads: Some(derived_payloads),
            }),
            profile_root,
            validator: RuntimeGenerationValidator::new(),
            in_process: Mutex::new(()),
            max_steps_per_call,
        }
    }

    /// 已活动 V3 的恢复只验证 target 并清理旧 source，不重新打开旧业务库。
    pub fn new_cleanup_only(
        profile_root: PathBuf,
        legacy_database: PathBuf,
        legacy_blob_root: PathBuf,
        keys: Arc<AdmissionKeyManager>,
        manifests: Arc<ActiveSpaceGenerationManifestStore>,
    ) -> Self {
        Self {
            legacy_database,
            legacy_blob_root,
            persistence: UpgradePersistence::new(profile_root.clone(), Arc::clone(&keys)),
            manifests,
            mode: UpgradeMode::Prepared(UpgradeComponents {
                target: TargetGenerationStager::cleanup_only(profile_root.clone(), keys),
                primary_payloads: None,
                derived_payloads: None,
            }),
            profile_root,
            validator: RuntimeGenerationValidator::new(),
            in_process: Mutex::new(()),
            max_steps_per_call: None,
        }
    }

    /// 确保当前 profile 使用完整 V3 存储布局。
    ///
    /// production 构造器会在一次调用内推进到 V3 promotion 或 Fresh-ready；
    /// 只有测试故障注入构造器会在单个耐久 phase 后返回 `Pending`。
    pub async fn ensure_v3(
        &self,
    ) -> Result<ProfileStorageUpgradeOutcome, ProfileStorageUpgradeError> {
        let _in_process = self.in_process.lock().await;
        let _lease = match self.persistence.try_acquire_lease()? {
            UpgradeLeaseResult::Acquired(lease) => lease,
            UpgradeLeaseResult::Busy => return Ok(ProfileStorageUpgradeOutcome::Busy),
        };

        match &self.mode {
            UpgradeMode::Prepared(components) => self.ensure_with(components).await,
            UpgradeMode::Runtime(bootstrap) => {
                let components = bootstrap
                    .prepare(
                        &self.profile_root,
                        &self.legacy_database,
                        &self.legacy_blob_root,
                        &self.manifests,
                    )
                    .await?;
                self.ensure_with(&components).await
            }
        }
    }

    async fn ensure_with(
        &self,
        components: &UpgradeComponents,
    ) -> Result<ProfileStorageUpgradeOutcome, ProfileStorageUpgradeError> {
        let mut steps = 0_usize;
        loop {
            let outcome = self.advance_once(components).await?;
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
    ) -> Result<ProfileStorageUpgradeOutcome, ProfileStorageUpgradeError> {
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
                if journal.matches_activated_fresh_profile(target) {
                    self.cleanup(&journal, &components.target)?;
                    self.persistence.clear_journal().await?;
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
            }
            UpgradePhaseV1::StoresSeparated => {
                let converted = self
                    .primary_payloads(components)?
                    .convert(&journal, &components.target)
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
                self.primary_payloads(components)?
                    .verify(&journal, &components.target)
                    .await?;
                let converted = self
                    .derived_payloads(components)?
                    .convert(&journal, &components.target)
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
                self.derived_payloads(components)?
                    .verify(&journal, &components.target)
                    .await?;
                let verified = self.validator.validate(&journal, &components.target)?;
                journal.mark_verified(
                    verified.profile_schema_digest,
                    verified.control_schema_digest,
                )?;
                self.persistence.save_journal(&journal).await?;
            }
            UpgradePhaseV1::Verified => {
                if source.is_none() {
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

    fn cleanup(
        &self,
        journal: &UpgradeJournalV1,
        target: &TargetGenerationStager,
    ) -> Result<(), ProfileStorageUpgradeError> {
        let paths = target.paths(journal);
        if let (Some(space_id), Some(database_generation)) = (
            journal.source_space_id(),
            journal.source_database_generation(),
        ) {
            let source = legacy_space_generation_directory(
                &self.profile_root.join("space-generations"),
                space_id,
                database_generation,
            );
            if paths.payload_output.starts_with(&source)
                || paths.control_database.starts_with(&source)
            {
                return Err(ProfileStorageUpgradeError::Corrupt {
                    source: anyhow::anyhow!("profile upgrade cleanup overlaps the active target"),
                });
            }
            remove_directory_if_present(&source)?;
            sync_parent_if_present(source.parent())?;
        } else {
            remove_sqlite_if_present(&self.legacy_database)?;
            remove_directory_if_present(&self.legacy_blob_root)?;
            sync_parent_if_present(self.legacy_database.parent())?;
            sync_parent_if_present(self.legacy_blob_root.parent())?;
        }

        remove_file_if_present(&paths.profile_database)?;
        remove_directory_if_present(&paths.primary_output)?;
        remove_file_if_present(&paths.scratch)?;
        remove_temporary_outputs(paths.payload_output.parent().ok_or_else(|| {
            ProfileStorageUpgradeError::Storage {
                source: anyhow::anyhow!("profile upgrade target parent is missing"),
            }
        })?)?;
        sync_parent_if_present(paths.profile_database.parent())?;
        Ok(())
    }
}

fn cleanup_source_unavailable() -> ProfileStorageUpgradeError {
    ProfileStorageUpgradeError::Corrupt {
        source: anyhow::anyhow!("profile upgrade source is unavailable after promotion"),
    }
}

fn legacy_space_generation_directory(
    generation_root: &Path,
    space_id: &str,
    generation: &[u8; 16],
) -> PathBuf {
    let mut hasher = Sha256::new();
    hasher.update(b"uniclipboard/space-generation-directory/v1\0");
    hasher.update(space_id.as_bytes());
    hasher.update(generation);
    let digest: [u8; 32] = hasher.finalize().into();
    generation_root.join(legacy_generation_directory_name(&digest))
}

fn legacy_generation_directory_name(digest: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(32);
    for byte in &digest[..16] {
        value.push(HEX[(byte >> 4) as usize] as char);
        value.push(HEX[(byte & 0x0f) as usize] as char);
    }
    value
}

fn remove_sqlite_if_present(path: &Path) -> Result<(), ProfileStorageUpgradeError> {
    remove_file_if_present(path)?;
    for suffix in ["-wal", "-shm"] {
        let sidecar = PathBuf::from(format!("{}{}", path.to_string_lossy(), suffix));
        remove_file_if_present(&sidecar)?;
    }
    Ok(())
}

fn remove_file_if_present(path: &Path) -> Result<(), ProfileStorageUpgradeError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(ProfileStorageUpgradeError::Storage {
            source: anyhow::Error::new(source).context("remove obsolete profile upgrade file"),
        }),
    }
}

fn remove_directory_if_present(path: &Path) -> Result<(), ProfileStorageUpgradeError> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(ProfileStorageUpgradeError::Storage {
            source: anyhow::Error::new(source).context("remove obsolete profile upgrade directory"),
        }),
    }
}

fn remove_temporary_outputs(parent: &Path) -> Result<(), ProfileStorageUpgradeError> {
    let entries =
        std::fs::read_dir(parent).map_err(|source| ProfileStorageUpgradeError::Storage {
            source: anyhow::Error::new(source).context("inspect profile upgrade staging directory"),
        })?;
    for entry in entries {
        let entry = entry.map_err(|source| ProfileStorageUpgradeError::Storage {
            source: anyhow::Error::new(source).context("inspect profile upgrade staging entry"),
        })?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(".v3-primary-") || name.starts_with(".v3-payloads-") {
            remove_directory_if_present(&entry.path())?;
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn sync_parent_if_present(parent: Option<&Path>) -> Result<(), ProfileStorageUpgradeError> {
    let Some(parent) = parent.filter(|parent| parent.is_dir()) else {
        return Ok(());
    };
    std::fs::File::open(parent)
        .and_then(|file| file.sync_all())
        .map_err(|source| ProfileStorageUpgradeError::Storage {
            source: anyhow::Error::new(source).context("sync profile upgrade cleanup directory"),
        })
}

#[cfg(windows)]
fn sync_parent_if_present(_parent: Option<&Path>) -> Result<(), ProfileStorageUpgradeError> {
    Ok(())
}
