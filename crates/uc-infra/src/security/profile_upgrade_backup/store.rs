use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use blake3::hash;
use uc_application::deps::{
    ProfileUpgradeBackupEntry, ProfileUpgradeBackupError, ProfileUpgradeBackupPort,
    ProfileUpgradeSource, ProfileUpgradeVersions,
};
use uc_core::app_dirs::AppPaths;
use uc_core::ports::SecureStoragePort;

use crate::app_version_state::{read_version_before_upgrade, DEFAULT_FILE_NAME};
use crate::fs::file_lock::try_lock_exclusive;
use crate::fs::VaultLayout;
use crate::security::profile_backup_archive::{
    sync_directory,
    tree::{create_private_directory, require_disjoint_destination, resolve_source_root},
};
use crate::security::{
    ProfileArchiveReceipt, ProfileBackupArchive, ProfileBackupArchiveError, ProfileBackupSource,
};

use super::diagnostics::{record_backup_failure, with_backup_action};
use super::inventory::{excluded_paths, has_profile};
use super::record::{
    publish_file_record, read_file_record, read_file_record_path, read_record, read_record_path,
    FileBackupRecord, RECORD_KEY,
};

const MAX_RETAINED_BACKUPS: usize = 5;

/// 宿主保证资料独占后使用；不打开数据库，不执行任何旧资料迁移。
#[derive(Clone)]
pub struct ProfileUpgradeBackupStore {
    pub(super) paths: AppPaths,
    pub(super) profile: String,
    pub(super) secure_storage: Arc<dyn SecureStoragePort>,
    directory: PathBuf,
}

impl ProfileUpgradeBackupStore {
    pub(super) fn record_failure(action: &'static str, error: &ProfileUpgradeBackupError) {
        record_backup_failure(action, error);
    }

    fn record_result<T>(
        action: &'static str,
        result: Result<T, ProfileUpgradeBackupError>,
    ) -> Result<T, ProfileUpgradeBackupError> {
        if let Err(error) = &result {
            record_backup_failure(action, error);
        }
        result
    }

    pub(super) fn record_action<T>(
        action: &'static str,
        result: Result<T, ProfileUpgradeBackupError>,
    ) -> Result<T, ProfileUpgradeBackupError> {
        result.map_err(|error| with_backup_action(action, error))
    }

    pub fn new(
        paths: AppPaths,
        profile: String,
        secure_storage: Arc<dyn SecureStoragePort>,
        directory: PathBuf,
    ) -> Self {
        // 同一宿主备份根可能服务多个便携安装；不能仅凭目标版本复用另一份资料的副本。
        // 宿主路径约定在重启和资料删除后保持不变，编号不依赖 userdata 中的新写入。
        let source_directory_id = hash(paths.app_data_root_dir.as_os_str().as_encoded_bytes());
        Self {
            paths,
            profile,
            secure_storage,
            directory: directory.join(source_directory_id.to_hex().as_str()),
        }
    }

    pub(super) fn directory(&self) -> PathBuf {
        self.directory.clone()
    }

    pub(super) fn source(&self) -> Result<ProfileBackupSource, ProfileUpgradeBackupError> {
        Ok(ProfileBackupSource {
            product_version: read_version_before_upgrade(
                &self.paths.app_data_root_dir.join(DEFAULT_FILE_NAME),
            )
            .map_err(backup_error)?,
            engine_version: read_version_before_upgrade(
                &VaultLayout::new(&self.paths.app_data_root_dir).engine_upgrade_cursor_path(),
            )
            .map_err(backup_error)?,
            platform: std::env::consts::OS.to_owned(),
            architecture: std::env::consts::ARCH.to_owned(),
            installation_channel: None,
            artifact_digest: None,
        })
    }
}

#[async_trait]
impl ProfileUpgradeBackupPort for ProfileUpgradeBackupStore {
    async fn list_backups(
        &self,
    ) -> Result<Vec<ProfileUpgradeBackupEntry>, ProfileUpgradeBackupError> {
        let store = self.clone();
        let result = tokio::task::spawn_blocking(move || {
            let _lease = store.lease()?;
            store.list_locked()
        })
        .await
        .map_err(backup_error)
        .and_then(|listed| listed);
        Self::record_result("list_backups", result)
    }

    async fn delete_backup(&self, id: &str) -> Result<(), ProfileUpgradeBackupError> {
        let store = self.clone();
        let id = id.to_owned();
        let result = tokio::task::spawn_blocking(move || {
            let _lease = store.lease()?;
            store.delete_locked(&id)
        })
        .await
        .map_err(backup_error)
        .and_then(|deleted| deleted);
        Self::record_result("delete_backup", result)
    }

    fn read_source(&self) -> Result<ProfileUpgradeSource, ProfileUpgradeBackupError> {
        Self::record_result(
            "read_source",
            (|| {
                if !has_profile(&self.paths)? {
                    return Ok(ProfileUpgradeSource {
                        has_data: false,
                        source_product: None,
                        source_engine: None,
                    });
                }
                let source = self.source()?;
                Ok(ProfileUpgradeSource {
                    has_data: true,
                    source_product: source.product_version,
                    source_engine: source.engine_version,
                })
            })(),
        )
    }

    async fn capture_verified(
        &self,
        target: &ProfileUpgradeVersions,
    ) -> Result<(), ProfileUpgradeBackupError> {
        let store = self.clone();
        let target = target.clone();
        let result = match tokio::task::spawn_blocking(move || store.capture(&target)).await {
            Ok(result) => result,
            Err(error) => Err(backup_error(error)),
        };
        Self::record_result("capture_profile", result)
    }

    async fn preserve_security_materials(
        &self,
        target: &ProfileUpgradeVersions,
    ) -> Result<(), ProfileUpgradeBackupError> {
        let store = self.clone();
        let target = target.clone();
        let result =
            match tokio::task::spawn_blocking(move || store.preserve_secrets(&target)).await {
                Ok(result) => result,
                Err(error) => Err(backup_error(error)),
            };
        Self::record_result("preserve_security_materials", result)
    }
}

impl ProfileUpgradeBackupStore {
    pub(super) fn lease(&self) -> Result<File, ProfileUpgradeBackupError> {
        for path in [
            &self.paths.app_data_root_dir,
            &self.paths.cache_dir,
            &self.paths.logs_dir,
        ] {
            if path.exists() {
                let root = fs::canonicalize(path).map_err(backup_error)?;
                require_disjoint_destination(&root, &self.directory()).map_err(backup_error)?;
            } else if self.directory().starts_with(path) || path.starts_with(self.directory()) {
                return Err(backup_error(io::Error::other(
                    "backup directory overlaps application data",
                )));
            }
        }
        create_private_directory(&self.directory()).map_err(backup_error)?;
        let mut options = OpenOptions::new();
        options.create(true).truncate(false).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let lease = options
            .open(self.directory().join(".lease"))
            .map_err(backup_error)?;
        try_lock_exclusive(&lease).map_err(|error| {
            backup_error(match error {
                TryLockError::WouldBlock => io::Error::from(io::ErrorKind::WouldBlock),
                TryLockError::Error(source) => source,
            })
        })?;
        Ok(lease)
    }

    fn capture(&self, target: &ProfileUpgradeVersions) -> Result<(), ProfileUpgradeBackupError> {
        let _lease = Self::record_action("acquire_lease", self.lease())?;
        // 取消后的旧任务可能刚完成发布；持锁重新读取，不覆盖它保留的原始资料。
        if let Some(prepared) =
            Self::record_action("read_prepared_record", read_file_record(&self.directory()))?
                .filter(|record| &record.target() == target)
        {
            Self::record_action("verify_prepared_profile", self.verify(target))?;
            if !Self::record_action(
                "check_prepared_freshness",
                self.prepared_backup_is_stale(&prepared),
            )? {
                return Ok(());
            }
            // 副本尚无安全记录且来源已变：升级写入还没开始，原副本保留在目录中，重新捕获当前资料。
        }
        let root = Self::record_action(
            "resolve_profile_root",
            resolve_source_root(&self.paths.app_data_root_dir).map_err(backup_error),
        )?;
        let directory = self.directory();
        let archive = ProfileBackupArchive::new(directory.clone());
        let source = Self::record_action("read_source_versions", self.source())?;
        let receipt = Self::record_action(
            "capture_profile_files",
            archive
                .capture_selected(&root, source.clone(), &excluded_paths(&self.paths, &root))
                .map_err(backup_error),
        )?;
        // 待处理内容可能是唯一副本，即使位于独立缓存根也必须随同保留。
        let spool_receipt = match fs::symlink_metadata(&self.paths.spool_dir) {
            Ok(_) => Some(Self::record_action(
                "capture_pending_files",
                archive
                    .capture(&self.paths.spool_dir, source.clone())
                    .map_err(backup_error),
            )?),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => {
                return Self::record_action("inspect_pending_files", Err(backup_error(error)))
            }
        };
        if Self::record_action("confirm_source_versions", self.source())? != source {
            return Self::record_action(
                "confirm_source_versions",
                Err(backup_error(ProfileBackupArchiveError::SourceChanged)),
            );
        }
        Self::record_action(
            "publish_file_record",
            publish_file_record(
                &directory,
                &FileBackupRecord {
                    schema: 3,
                    target_product: target.product.clone(),
                    target_engine: target.engine.clone(),
                    receipt,
                    spool_receipt,
                },
            ),
        )
    }

    /// 已准备副本是否应当丢弃重捕获。
    ///
    /// 安全记录一旦属于这份副本，后续升级写入可能已经开始，来源资料不再等于升级前状态，
    /// 绝不能再捕获；读不出安全记录时同样按“可能已发布”保守复用。只有确认没有属于它的
    /// 安全记录、且来源摘要不再匹配时才判定过期；其他校验失败照常上报。
    fn prepared_backup_is_stale(
        &self,
        prepared: &FileBackupRecord,
    ) -> Result<bool, ProfileUpgradeBackupError> {
        let published = match read_record(&self.directory(), self.secure_storage.as_ref()) {
            Ok(Some(security)) => {
                security.files.receipt == prepared.receipt
                    && security.files.spool_receipt == prepared.spool_receipt
                    && security.files.target() == prepared.target()
            }
            Ok(None) => false,
            Err(_) => true,
        };
        if published {
            return Ok(false);
        }
        match self.verify_source_files(prepared) {
            Ok(()) => Ok(false),
            Err(error)
                if error.source.chain().any(|source| {
                    matches!(
                        source.downcast_ref::<ProfileBackupArchiveError>(),
                        Some(ProfileBackupArchiveError::SourceChanged)
                    )
                }) =>
            {
                Ok(true)
            }
            Err(error) => Err(error),
        }
    }

    pub(super) fn prune_locked(&self) -> Result<(), ProfileUpgradeBackupError> {
        let backups = self.list_locked()?;
        for backup in backups.into_iter().skip(MAX_RETAINED_BACKUPS) {
            self.delete_locked(&backup.id)?;
        }
        Ok(())
    }

    fn list_locked(&self) -> Result<Vec<ProfileUpgradeBackupEntry>, ProfileUpgradeBackupError> {
        let directory = self.directory();
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(backup_error(error)),
        };
        let mut backups = Vec::new();
        for entry in entries {
            let entry = entry.map_err(backup_error)?;
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("files") {
                continue;
            }
            let Some(record) = read_file_record_path(&path)? else {
                continue;
            };
            let id = uuid::Uuid::from_bytes(record.receipt.archive_id).to_string();
            if path.file_stem().and_then(|value| value.to_str()) != Some(id.as_str()) {
                return Err(backup_error(io::Error::other(
                    "profile backup record identity changed",
                )));
            }
            let created_at_ms = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH)
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                .min(u128::from(u64::MAX)) as u64;
            let mut size_bytes =
                file_size(&path)? + file_size(&self.archive_path(&record.receipt))?;
            if let Some(receipt) = &record.spool_receipt {
                size_bytes = size_bytes.saturating_add(file_size(&self.archive_path(receipt))?);
            }
            backups.push(ProfileUpgradeBackupEntry {
                id,
                created_at_ms,
                source_product: record.receipt.source.product_version.clone(),
                source_engine: record.receipt.source.engine_version.clone(),
                target_product: record.target_product,
                target_engine: record.target_engine,
                size_bytes,
            });
        }
        backups.sort_by(|left, right| {
            right
                .created_at_ms
                .cmp(&left.created_at_ms)
                .then_with(|| right.id.cmp(&left.id))
        });
        Ok(backups)
    }

    fn delete_locked(&self, id: &str) -> Result<(), ProfileUpgradeBackupError> {
        let archive_id = uuid::Uuid::parse_str(id).map_err(backup_error)?;
        let directory = self.directory();
        let record_path = directory.join(format!("{archive_id}.files"));
        let record = read_file_record_path(&record_path)?
            .ok_or_else(|| backup_error(io::Error::from(io::ErrorKind::NotFound)))?;
        if record.receipt.archive_id != *archive_id.as_bytes() {
            return Err(backup_error(io::Error::other(
                "profile backup record identity changed",
            )));
        }

        let current_matches = read_file_record(&directory)?
            .is_some_and(|current| current.receipt.archive_id == record.receipt.archive_id);
        if current_matches {
            remove_if_exists(&directory.join("current"))?;
            remove_if_exists(&directory.join("security-current"))?;
        }

        for entry in fs::read_dir(&directory).map_err(backup_error)? {
            let path = entry.map_err(backup_error)?.path();
            if path.extension().and_then(|value| value.to_str()) != Some("record") {
                continue;
            }
            if read_record_path(&path, self.secure_storage.as_ref())?.is_some_and(|security| {
                security.files.receipt.archive_id == record.receipt.archive_id
            }) {
                remove_if_exists(&path)?;
            }
        }
        remove_if_exists(&self.archive_path(&record.receipt))?;
        if let Some(receipt) = &record.spool_receipt {
            remove_if_exists(&self.archive_path(receipt))?;
        }
        remove_if_exists(&record_path)?;
        if self.list_locked()?.is_empty() {
            self.secure_storage
                .delete(RECORD_KEY)
                .map_err(backup_error)?;
        }
        sync_directory(&directory).map_err(backup_error)
    }

    pub(super) fn archive_path(&self, receipt: &ProfileArchiveReceipt) -> PathBuf {
        self.directory().join(format!(
            "{}.archive",
            uuid::Uuid::from_bytes(receipt.archive_id)
        ))
    }

    pub(super) fn verify(
        &self,
        target: &ProfileUpgradeVersions,
    ) -> Result<(), ProfileUpgradeBackupError> {
        let record = Self::record_action(
            "read_prepared_record",
            read_file_record(&self.directory()).and_then(|record| {
                record.ok_or_else(|| {
                    backup_error(io::Error::other("profile upgrade backup is missing"))
                })
            }),
        )?;
        if &record.target() != target {
            return Self::record_action(
                "validate_backup_target",
                Err(backup_error(io::Error::other(
                    "profile upgrade backup target changed",
                ))),
            );
        }
        let archive = ProfileBackupArchive::new(self.directory());
        Self::record_action(
            "verify_profile_archive",
            archive.verify(&record.receipt).map_err(backup_error),
        )?;
        if let Some(receipt) = &record.spool_receipt {
            if receipt.source != record.receipt.source {
                return Self::record_action(
                    "validate_pending_archive_source",
                    Err(backup_error(io::Error::other(
                        "profile backup sources do not match",
                    ))),
                );
            }
            Self::record_action(
                "verify_pending_archive",
                archive.verify(receipt).map_err(backup_error),
            )?;
        }
        Ok(())
    }
}

pub(super) fn backup_error(source: impl Into<anyhow::Error>) -> ProfileUpgradeBackupError {
    ProfileUpgradeBackupError {
        source: source.into(),
    }
}

fn file_size(path: &Path) -> Result<u64, ProfileUpgradeBackupError> {
    fs::metadata(path)
        .map(|metadata| metadata.len())
        .map_err(backup_error)
}

/// 返回是否确实删除了文件；不存在视为已完成。
pub(super) fn remove_if_exists(path: &Path) -> Result<bool, ProfileUpgradeBackupError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(backup_error(error)),
    }
}
