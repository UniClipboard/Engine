//! 配置导入的离线采用事务：保留目标状态，核验副本，再安装来源资料。

use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use uc_observability_contract::uc_warn;
use uuid::Uuid;

use uc_application::deps::{ProfileLifecycleRepositoryPort, ProfileLifecycleState};
use uc_core::app_dirs::{AppPaths, HostConfigurationFiles};
use uc_core::ports::SecureStoragePort;
use uc_infra_local::fs::atomic_publish::rename_no_replace_io;
use uc_infra_local::fs::durability::{replace_file, sync_directory};

use crate::security::{
    ActiveSpaceGenerationManifestStore, AdmissionKeyManager, ProfileArchiveReceipt,
    ProfileBackupArchive, ProfileBackupSource, ProfileKeyRecoveryStore, ProfileLifecycleRepository,
    ProfileRuntimeLayout, ProfileUpgradeBackupStore,
};

use super::pending::{
    adoption_journal_present, inspect_pending_import, read_import_record, PendingImportState,
};
use super::staging::{
    complete_pending_import, copy_member, copy_staged_file, install_pending_import,
    PendingImportError, CONTROL_DB_MEMBER, DB_MEMBER, PENDING_IMPORT_MARKER, STAGING_DIR_NAME,
};

pub(crate) const ADOPTION_JOURNAL: &str = ".config-import-adoption-v1";

// 只有这些 Engine 状态参与采用；归档和宿主配置声明都不能扩大退役范围。
const COMPONENTS: [&str; 14] = [
    "vault",
    "iroh-identity",
    "iroh-blobs",
    "profile-storage-upgrade",
    "profile-data-generations",
    "space-control-generations",
    "space-generations",
    "uniclipboard.db",
    "uniclipboard.db-wal",
    "uniclipboard.db-shm",
    "first-sync-state.json",
    "settings.json",
    "file-cache",
    ".engine-upgrade-cursor.json",
];

#[derive(Serialize, Deserialize)]
struct AdoptionJournal {
    format_version: u16,
    transaction: [u8; 16],
    marker_digest: [u8; 32],
    present: Vec<bool>,
    source: ProfileBackupSource,
    receipt: Option<ProfileArchiveReceipt>,
    installed: bool,
}

/// 调用方保证资料独占。所有退役原件和经过核验的归档均保留，不在成功后自动删除。
pub(crate) fn adopt_pending_import(
    paths: &AppPaths,
    files: &HostConfigurationFiles,
    storage: Arc<dyn SecureStoragePort>,
    recovery: &ProfileKeyRecoveryStore,
    backups: &ProfileUpgradeBackupStore,
) -> Result<(), PendingImportError> {
    let journal_path = paths.app_data_root_dir.join(ADOPTION_JOURNAL);
    let marker_path = paths.app_data_root_dir.join(PENDING_IMPORT_MARKER);
    if !adoption_journal_present(&paths.app_data_root_dir).map_err(failure)? {
        match inspect_pending_import(&paths.app_data_root_dir).map_err(failure)? {
            PendingImportState::Absent => return Ok(()),
            PendingImportState::Rejected(_) => {
                uc_warn!("incompatible_staged_import_retained");
                return Ok(());
            }
            PendingImportState::Applicable => {}
        }
    }
    let marker = read_import_record(&marker_path).map_err(PendingImportError::read_marker_from)?;
    let mut components: Vec<PathBuf> = COMPONENTS
        .iter()
        .map(|name| paths.app_data_root_dir.join(name))
        .collect();
    components.push(backups.directory());
    components.push(paths.spool_dir.clone());
    let mut journal: AdoptionJournal = match read_import_record(&journal_path).map_err(failure)? {
        Some(bytes) => serde_json::from_slice(&bytes).map_err(failure)?,
        None => {
            let marker = marker
                .as_ref()
                .ok_or_else(PendingImportError::read_marker)?;
            // 只在事务尚未开始时读取目标生命周期，重试不打开可能只复制了一部分的 vault。
            let lifecycle = ProfileLifecycleRepository::new(Arc::clone(&storage))
                .load()
                .map_err(failure)?;
            if lifecycle.is_some_and(|state| state.state() != ProfileLifecycleState::Ready) {
                return Err(failure(io::Error::other(
                    "profile reset must finish before import",
                )));
            }
            let journal = AdoptionJournal {
                format_version: 1,
                transaction: *Uuid::new_v4().as_bytes(),
                marker_digest: *blake3::hash(marker).as_bytes(),
                present: components
                    .iter()
                    .map(|path| {
                        if path == &paths.settings_path
                            && !paths
                                .app_data_root_dir
                                .join(STAGING_DIR_NAME)
                                .join("settings.json")
                                .exists()
                        {
                            return Ok(false);
                        }
                        exists(path)
                    })
                    .collect::<Result<_, _>>()?,
                source: backups.source().map_err(failure)?,
                receipt: None,
                installed: false,
            };
            write_record(&journal_path, &journal)?;
            journal
        }
    };
    if journal.format_version != 1 || journal.present.len() != components.len() {
        return Err(failure(io::Error::other("invalid import adoption journal")));
    }
    if let Some(marker) = marker.as_ref() {
        if blake3::hash(marker).as_bytes() != &journal.marker_digest {
            return Err(failure(io::Error::other(
                "pending import changed during adoption",
            )));
        }
    } else if !journal.installed {
        return Err(failure(io::Error::other(
            "pending import marker disappeared",
        )));
    }
    let parent = paths
        .app_data_root_dir
        .parent()
        .ok_or_else(|| failure(io::Error::other("import recovery parent is missing")))?;
    let root_hash = blake3::hash(paths.app_data_root_dir.as_os_str().as_encoded_bytes());
    let recovery_root = parent
        .join(".uc-config-import-recovery")
        .join(root_hash.to_hex().as_str());
    private_directory(&parent.join(".uc-config-import-recovery"))?;
    private_directory(&recovery_root)?;
    let transaction_root = recovery_root.join(Uuid::from_bytes(journal.transaction).to_string());
    private_directory(&transaction_root)?;
    let originals = transaction_root.join("originals");
    private_directory(&originals)?;
    let archive_root = transaction_root.join("archive");
    private_directory(&archive_root)?;
    recovery.suspend();
    if journal.receipt.is_none() {
        for (index, source) in components.iter().enumerate() {
            if !journal.present[index] {
                continue;
            }
            let name = if index < COMPONENTS.len() {
                COMPONENTS[index]
            } else if index == COMPONENTS.len() {
                "upgrade-backups"
            } else {
                "spool"
            };
            let destination = originals.join(name);
            if exists(&destination)? {
                if exists(source)? {
                    return Err(failure(io::Error::other(
                        "import retirement has ambiguous state",
                    )));
                }
            } else {
                rename_no_replace_io(source, &destination).map_err(failure)?;
                sync_directory(&originals).map_err(failure)?;
                if let Some(parent) = source.parent() {
                    sync_directory(parent).map_err(failure)?;
                }
            }
        }
        let archive = ProfileBackupArchive::new(archive_root.clone());
        let receipt = archive
            .capture(&originals, journal.source.clone())
            .map_err(failure)?;
        archive.verify(&receipt).map_err(failure)?;
        write_record(&transaction_root.join("receipt.json"), &receipt)?;
        journal.receipt = Some(receipt);
        write_record(&journal_path, &journal)?;
    }
    if !journal.installed {
        let receipt = journal
            .receipt
            .as_ref()
            .ok_or_else(|| failure(io::Error::other("import backup receipt is missing")))?;
        ProfileBackupArchive::new(archive_root.clone())
            .verify(receipt)
            .map_err(failure)?;
        install_pending_import(
            &paths.app_data_root_dir,
            &paths.db_path,
            &paths.vault_dir,
            &paths.settings_path,
            &paths.iroh_identity_dir(),
            files,
            storage.as_ref(),
        )?;
        recovery.suspend();
        install_runtime(paths, Arc::clone(&storage))?;
        journal.installed = true;
        write_record(&journal_path, &journal)?;
    }
    // 先解除 marker 再清理暂存。中断留下的暂存不再是活动事务。
    complete_pending_import(&paths.app_data_root_dir)?;
    fs::remove_file(&journal_path).map_err(failure)?;
    sync_directory(&paths.app_data_root_dir).map_err(failure)?;
    Ok(())
}

fn exists(path: &Path) -> Result<bool, PendingImportError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(failure(error)),
    }
}

fn private_directory(path: &Path) -> Result<(), PendingImportError> {
    let builder = DirBuilder::new();
    #[cfg(unix)]
    let builder = {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = builder;
        builder.mode(0o700);
        builder
    };
    match builder.create(path) {
        Ok(()) => {
            sync_directory(path).map_err(failure)?;
            if let Some(parent) = path.parent() {
                sync_directory(parent).map_err(failure)?;
            }
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            if !fs::symlink_metadata(path)
                .map_err(failure)?
                .file_type()
                .is_dir()
            {
                return Err(failure(io::Error::other(
                    "import recovery path is not a directory",
                )));
            }
        }
        Err(error) => return Err(failure(error)),
    }
    Ok(())
}

fn write_record(path: &Path, journal: &impl Serialize) -> Result<(), PendingImportError> {
    let bytes = serde_json::to_vec(journal).map_err(failure)?;
    let temporary = path.with_extension("tmp");
    match fs::remove_file(&temporary) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(failure(error)),
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary).map_err(failure)?;
    file.write_all(&bytes).map_err(failure)?;
    file.sync_all().map_err(failure)?;
    drop(file);
    replace_file(&temporary, path).map_err(failure)?;
    if let Some(parent) = path.parent() {
        sync_directory(parent).map_err(failure)?;
    }
    Ok(())
}

fn failure(source: impl Into<anyhow::Error>) -> PendingImportError {
    PendingImportError::copy_member_from(source)
}

// 源 manifest 通过既有加密存储认证；目录只由布局负责人从 opaque generation 派生。
fn install_runtime(
    paths: &AppPaths,
    storage: Arc<dyn SecureStoragePort>,
) -> Result<(), PendingImportError> {
    let lifecycle = ProfileLifecycleRepository::new(Arc::clone(&storage))
        .load()
        .map_err(failure)?
        .ok_or_else(|| failure(io::Error::other("imported lifecycle is missing")))?;
    if lifecycle.state() != ProfileLifecycleState::Ready {
        return Err(failure(io::Error::other("imported lifecycle is not ready")));
    }
    let keys = Arc::new(AdmissionKeyManager::new(
        storage,
        lifecycle.generation().into_bytes(),
    ));
    let manifest = ActiveSpaceGenerationManifestStore::new(paths.vault_dir.clone(), keys)
        .load_v3_sync()
        .map_err(failure)?
        .ok_or_else(|| failure(io::Error::other("imported runtime manifest is missing")))?;
    let layout = ProfileRuntimeLayout::v3(&paths.app_data_root_dir, &manifest);
    let staging = paths.app_data_root_dir.join(STAGING_DIR_NAME);
    copy_member(&staging, DB_MEMBER, layout.profile_database())?;
    copy_member(&staging, CONTROL_DB_MEMBER, layout.control_database())?;
    private_directory(layout.blob_root())?;
    let source_blobs = staging.join("runtime-blobs");
    if source_blobs.exists() {
        let mut pending = vec![source_blobs.clone()];
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(directory).map_err(failure)? {
                let entry = entry.map_err(failure)?;
                let path = entry.path();
                let kind = entry.file_type().map_err(failure)?;
                if kind.is_dir() {
                    pending.push(path);
                } else if kind.is_file() {
                    let relative = path.strip_prefix(&source_blobs).map_err(failure)?;
                    copy_staged_file(&path, &layout.blob_root().join(relative))?;
                } else {
                    return Err(failure(io::Error::other("invalid staged blob member")));
                }
            }
        }
    }
    Ok(())
}
