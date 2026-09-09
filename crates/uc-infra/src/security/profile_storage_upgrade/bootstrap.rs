//! 构造升级负责人，并在主流程取得租约后准备旧资料。

use super::derived_payloads::DerivedPayloadConverter;
use super::persistence::UpgradePersistence;
use super::primary_payloads::PrimaryPayloadConverter;
use super::progress::UpgradeProgress;
use super::target::{legacy_space_generation_directory, TargetGenerationStager};
use super::validation::RuntimeGenerationValidator;
use super::{ProfileStorageUpgrade, ProfileStorageUpgradeError, UpgradeComponents, UpgradeMode};
use crate::security::{
    ActiveRuntimeManifest, ActiveSpaceGenerationManifestStore, AdmissionKeyManager,
    DefaultCurrentProfile, ProfileContentKeyVault,
};
use crate::space::{InMemorySession, KeyMaterialStore, RuntimeSpaceAccessAdapter};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;
use uc_application::deps::{CurrentSpaceIdentityPort as _, InitialSpaceActivationPort as _};
use uc_core::ids::ProfileId;
use uc_core::membership::RevocationRepositoryPort as _;
use uc_core::ports::space::SpaceAccessStore as _;
use uc_core::ports::SecureStoragePort;

pub(super) struct RuntimeUpgradeBootstrap {
    profile_id: ProfileId,
    secure_storage: Arc<dyn SecureStoragePort>,
    vault_path: PathBuf,
    vault: Arc<ProfileContentKeyVault>,
    keys: Arc<AdmissionKeyManager>,
    current_space: Arc<crate::space::CurrentSpaceResolver>,
}

#[derive(serde::Deserialize)]
struct LegacySetupStatus {
    has_completed: bool,
    #[serde(default)]
    space_id: Option<String>,
}

impl LegacySetupStatus {
    fn completed_space_id(self) -> Option<uc_core::ids::SpaceId> {
        self.has_completed.then(|| {
            self.space_id
                .filter(|space_id| !space_id.is_empty())
                .map(uc_core::ids::SpaceId::from_string)
                .unwrap_or_else(uc_core::ids::SpaceId::new)
        })
    }
}

impl RuntimeUpgradeBootstrap {
    pub(super) async fn prepare(
        &self,
        profile_root: &Path,
        legacy_database: &Path,
        legacy_blob_root: &Path,
        manifests: &ActiveSpaceGenerationManifestStore,
        progress: &UpgradeProgress,
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
                legacy_space_id: None,
            });
        }

        let (source_database, source_blob_root, source_space_id) = match active.as_ref() {
            Some(ActiveRuntimeManifest::V2(source)) => {
                progress.required(true);
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
            None => {
                let legacy_space_id = self.resolve_legacy_space_id().await?;
                progress.required(legacy_space_id.is_some());
                (
                    legacy_database.to_path_buf(),
                    legacy_blob_root.to_path_buf(),
                    legacy_space_id.map(|space_id| space_id.as_ref().to_owned()),
                )
            }
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
        let source_session = Arc::new(InMemorySession::for_maintenance());
        if let Some(source_space_id) = source_space_id.as_ref() {
            let current_profile: Arc<
                dyn uc_core::ports::security::current_profile::CurrentProfilePort,
            > = Arc::new(DefaultCurrentProfile::for_profile(self.profile_id.clone()));
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
                security_repository.clone(),
                Arc::clone(&self.vault),
            );
            let resumed = access
                .try_resume_session(&uc_core::ids::SpaceId::from_string(source_space_id.clone()))
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
            // 存储布局缺少 manifest 不代表旧版没有可用的群组密钥。
            let needs_legacy_material = active.is_none()
                && security_repository
                    .load_space_material(&uc_core::ids::SpaceId::from_string(
                        source_space_id.clone(),
                    ))
                    .await
                    .map_err(|source| ProfileStorageUpgradeError::Security {
                        source: anyhow::Error::new(source)
                            .context("inspect restored legacy profile security material"),
                    })?
                    .is_none();
            if needs_legacy_material {
                let material = source_session
                    .create_profile_storage_upgrade_material(&uc_core::ids::SpaceId::from_string(
                        source_space_id.clone(),
                    ))
                    .map_err(|source| ProfileStorageUpgradeError::Security {
                        source: anyhow::Error::new(source)
                            .context("prepare V3 content protection for legacy profile upgrade"),
                    })?;
                source_session
                    .install_space_material(&material)
                    .map_err(|source| ProfileStorageUpgradeError::Security {
                        source: anyhow::Error::new(source)
                            .context("activate V3 content protection for legacy profile upgrade"),
                    })?;
                self.vault
                    .install_verified_space_material(&material)
                    .await
                    .map_err(|source| ProfileStorageUpgradeError::Security {
                        source: anyhow::Error::new(source)
                            .context("persist V3 content protection for legacy profile upgrade"),
                    })?;
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
            legacy_space_id: if active.is_none() {
                source_space_id.map(uc_core::ids::SpaceId::from_string)
            } else {
                None
            },
        })
    }

    async fn resolve_legacy_space_id(
        &self,
    ) -> Result<Option<uc_core::ids::SpaceId>, ProfileStorageUpgradeError> {
        if let Some(space_id) = self
            .current_space
            .current_space_id()
            .await
            .map_err(|source| ProfileStorageUpgradeError::Security {
                source: anyhow::Error::new(source)
                    .context("load protected legacy Space identity for profile storage upgrade"),
            })?
        {
            return Ok(Some(space_id));
        }

        let status_path = self.vault_path.join(".setup_status");
        let bytes = match std::fs::read(&status_path) {
            Ok(bytes) => bytes,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(ProfileStorageUpgradeError::Storage {
                    source: anyhow::Error::new(source)
                        .context("read legacy setup status for profile storage upgrade"),
                });
            }
        };
        let status: LegacySetupStatus = serde_json::from_slice(&bytes).map_err(|source| {
            ProfileStorageUpgradeError::Corrupt {
                source: anyhow::Error::new(source)
                    .context("decode legacy setup status for profile storage upgrade"),
            }
        })?;
        let Some(space_id) = status.completed_space_id() else {
            return Ok(None);
        };
        self.current_space
            .activate_initial_space(&space_id)
            .await
            .map_err(|source| ProfileStorageUpgradeError::Security {
                source: anyhow::Error::new(source)
                    .context("protect legacy Space identity for profile storage upgrade"),
            })?;
        Ok(Some(space_id))
    }
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
        current_space: Arc<crate::space::CurrentSpaceResolver>,
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
                current_space,
            }),
            profile_root,
            validator: RuntimeGenerationValidator::new(),
            in_process: Mutex::new(()),
            max_steps_per_call: None,
            progress: None,
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
                legacy_space_id: None,
            }),
            profile_root,
            validator: RuntimeGenerationValidator::new(),
            in_process: Mutex::new(()),
            max_steps_per_call,
            progress: None,
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
                legacy_space_id: None,
            }),
            profile_root,
            validator: RuntimeGenerationValidator::new(),
            in_process: Mutex::new(()),
            max_steps_per_call: None,
            progress: None,
        }
    }
}

#[cfg(test)]
mod legacy_setup_status_tests {
    use super::LegacySetupStatus;

    #[test]
    fn completed_pre_space_id_status_gets_a_recoverable_identity() {
        let space_id = LegacySetupStatus {
            has_completed: true,
            space_id: None,
        }
        .completed_space_id()
        .expect("completed legacy setup should remain recoverable");

        assert!(!space_id.as_ref().is_empty());
    }

    #[test]
    fn incomplete_status_does_not_become_an_existing_profile() {
        assert!(LegacySetupStatus {
            has_completed: false,
            space_id: Some("stale-space".to_owned()),
        }
        .completed_space_id()
        .is_none());
    }
}
