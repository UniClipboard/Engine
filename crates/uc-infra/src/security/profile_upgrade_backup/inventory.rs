use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uc_application::deps::ProfileUpgradeBackupError;
use uc_core::app_dirs::AppPaths;
use uc_core::ports::SecureStoragePort;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::config_migration::secret_keys::migratable_secret_keys;
use crate::migration_state::{decode_legacy_migration_run_id, DEFAULT_MIGRATION_STATE_FILE};
use crate::network::iroh::IDENTITY_STORE_KEY;
use crate::security::admission_key_manager::PROFILE_ADMISSION_KEY_NAME;
use crate::security::key_migration_adapter::DefaultKeyMigrationAdapter;
use crate::security::profile_content_key_vault::PROFILE_CONTENT_VAULT_KEY_NAME;
use crate::security::profile_lifecycle::PROFILE_LIFECYCLE_MARKER_NAME;

use super::record::read_bounded;
use super::store::backup_error;

pub(super) const BACKUP_DIRECTORY: &str = "profile-upgrade-backups";

#[derive(PartialEq, Eq, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub(super) struct SecretValue {
    name: String,
    value: Option<Vec<u8>>,
}

pub(super) fn read_secrets(
    paths: &AppPaths,
    profile: &str,
    storage: &dyn SecureStoragePort,
) -> Result<Vec<SecretValue>, ProfileUpgradeBackupError> {
    let mut names: Vec<String> = migratable_secret_keys(profile)
        .into_iter()
        .map(|key| key.key)
        .collect();
    names.extend(
        [
            PROFILE_ADMISSION_KEY_NAME,
            PROFILE_CONTENT_VAULT_KEY_NAME,
            PROFILE_LIFECYCLE_MARKER_NAME,
            IDENTITY_STORE_KEY,
        ]
        .into_iter()
        .map(str::to_owned),
    );
    if let Some(bytes) = read_bounded(&paths.vault_dir.join(DEFAULT_MIGRATION_STATE_FILE))? {
        if !bytes.iter().all(u8::is_ascii_whitespace) {
            if let Some(run) = decode_legacy_migration_run_id(&bytes).map_err(backup_error)? {
                names.push(DefaultKeyMigrationAdapter::keyring_name(&run));
            }
        }
    }
    names
        .into_iter()
        .map(|name| {
            let value = storage.get(&name).map_err(backup_error)?;
            if value.as_ref().is_some_and(|value| value.len() > 16 * 1024) {
                return Err(backup_error(io::Error::other(
                    "profile backup secret exceeds limit",
                )));
            }
            Ok(SecretValue { name, value })
        })
        .collect()
}

pub(super) fn excluded_paths(paths: &AppPaths, root: &Path) -> Vec<PathBuf> {
    // 仅排除明确的非业务资料；未知普通文件保留，未知特殊文件由归档层拒绝。
    let mut excluded: Vec<_> = [
        BACKUP_DIRECTORY,
        ".uniclipd.lock",
        ".daemon-token",
        ".daemon-pid",
        "daemon.conn",
        "daemon-startup.conn",
        "daemon-run.json",
        "daemon-last-exit.json",
        ".uniclipd-handover.json",
        "EBWebView",
    ]
    .into_iter()
    .map(|name| root.join(name))
    .collect();
    for path in [&paths.logs_dir, &paths.cache_dir, &paths.spool_dir] {
        if let Ok(relative) = path.strip_prefix(&paths.app_data_root_dir) {
            if !relative.as_os_str().is_empty() {
                excluded.push(root.join(relative));
            }
        }
    }
    excluded
}

pub(super) fn has_profile(paths: &AppPaths) -> Result<bool, ProfileUpgradeBackupError> {
    // 空安装目录及宿主预建的运行文件不是待升级用户资料。
    for path in [
        &paths.db_path,
        &paths.vault_dir,
        &paths.settings_path,
        &paths.file_cache_dir,
        &paths.spool_dir,
    ] {
        match fs::symlink_metadata(path) {
            Ok(_) => return Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(backup_error(error)),
        }
    }
    match fs::read_dir(&paths.app_data_root_dir) {
        Ok(entries) => {
            for entry in entries {
                let name = entry.map_err(backup_error)?.file_name();
                if name.to_str().is_some_and(|name| {
                    name.starts_with("iroh-")
                        || matches!(
                            name,
                            "profile-data-generations"
                                | "space-control-generations"
                                | "space-generations"
                        )
                }) {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(backup_error(error)),
    }
}
