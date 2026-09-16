use std::fs;
use std::io;
use std::path::Path;
use std::sync::Arc;

use uc_application::deps::{ProfileStartupStorageError, ProfileStartupStoragePort};
use uc_core::app_dirs::AppPaths;
use uc_core::ports::SecureStoragePort;

use crate::config_migration::staging::apply_pending_import;

/// 启动前的旧布局和待导入资料适配器；操作顺序由 Application 决定。
pub struct ProfileStartupStorage {
    paths: AppPaths,
    secure_storage: Arc<dyn SecureStoragePort>,
}

impl ProfileStartupStorage {
    pub fn new(paths: AppPaths, secure_storage: Arc<dyn SecureStoragePort>) -> Self {
        Self {
            paths,
            secure_storage,
        }
    }
}

impl ProfileStartupStoragePort for ProfileStartupStorage {
    fn adopt_legacy_layout(&self) -> Result<(), ProfileStartupStorageError> {
        adopt_v019_profile_directories(&self.paths.app_data_root_dir)
    }

    fn apply_pending_import(&self) -> Result<(), ProfileStartupStorageError> {
        apply_pending_import(
            &self.paths.app_data_root_dir,
            &self.paths.db_path,
            &self.paths.vault_dir,
            &self.paths.settings_path,
            &self.paths.app_data_root_dir.join("iroh-identity"),
            self.secure_storage.as_ref(),
        )
        .map_err(storage_error)
    }
}

fn adopt_v019_profile_directories(root: &Path) -> Result<(), ProfileStartupStorageError> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(storage_error(error)),
    };
    let mut removals = Vec::new();
    let mut moves = Vec::new();
    for (name, description) in [("iroh-identity", "identity"), ("iroh-blobs", "blob store")] {
        let current = root.join(name);
        let mut legacy_directories = entries.iter().map(|entry| entry.path()).filter(|path| {
            path.file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.starts_with(&format!("{name}_")))
        });
        let legacy = legacy_directories.next();
        if legacy_directories.next().is_some() {
            return Err(storage_error(anyhow::anyhow!(
                "multiple v0.19 {description} directories found"
            )));
        }
        let Some(legacy) = legacy else {
            continue;
        };
        if current.try_exists().map_err(storage_error)? {
            let mut contents = fs::read_dir(&legacy).map_err(storage_error)?;
            match contents.next().transpose().map_err(storage_error)? {
                None => removals.push(legacy),
                Some(_) => {
                    return Err(storage_error(anyhow::anyhow!(
                        "v0.19 {description} directory conflict"
                    )))
                }
            }
        } else {
            moves.push((legacy, current));
        }
    }
    for legacy in removals {
        fs::remove_dir(legacy).map_err(storage_error)?;
    }
    for (legacy, current) in moves {
        fs::rename(legacy, current).map_err(storage_error)?;
    }
    Ok(())
}

fn storage_error(source: impl Into<anyhow::Error>) -> ProfileStartupStorageError {
    ProfileStartupStorageError {
        source: source.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::adopt_v019_profile_directories;
    #[test]
    fn v019_profile_directories_are_adopted_before_engine_wiring() {
        let root = tempfile::tempdir().unwrap();
        let data_root = root.path();
        let legacy_identity = data_root.join("iroh-identity_mobile_primary");
        let legacy_blobs = data_root.join("iroh-blobs_mobile_primary");
        std::fs::create_dir_all(&legacy_identity).unwrap();
        std::fs::create_dir_all(&legacy_blobs).unwrap();
        std::fs::write(legacy_identity.join("identity.bin"), b"identity").unwrap();
        std::fs::write(legacy_blobs.join("blobs.db"), b"blobs").unwrap();

        adopt_v019_profile_directories(data_root).unwrap();

        assert_eq!(
            std::fs::read(data_root.join("iroh-identity/identity.bin")).unwrap(),
            b"identity"
        );
        assert_eq!(
            std::fs::read(data_root.join("iroh-blobs/blobs.db")).unwrap(),
            b"blobs"
        );
        assert!(!legacy_identity.exists());
        assert!(!legacy_blobs.exists());
    }

    #[test]
    fn absent_v019_profile_directories_leave_current_layout_untouched() {
        let root = tempfile::tempdir().unwrap();

        adopt_v019_profile_directories(root.path()).unwrap();

        assert!(!root.path().join("iroh-identity").exists());
        assert!(!root.path().join("iroh-blobs").exists());
    }

    #[test]
    fn missing_app_data_root_is_treated_as_a_fresh_installation() {
        let root = tempfile::tempdir().unwrap();
        let data_root = root.path().join("private");

        adopt_v019_profile_directories(&data_root).unwrap();

        assert!(!data_root.exists());
    }

    #[test]
    fn empty_v019_identity_directory_is_removed_when_current_identity_exists() {
        let root = tempfile::tempdir().unwrap();
        let current_identity = root.path().join("iroh-identity");
        let legacy_identity = root.path().join("iroh-identity_profile");
        std::fs::create_dir_all(&current_identity).unwrap();
        std::fs::create_dir_all(&legacy_identity).unwrap();
        std::fs::write(current_identity.join("identity.bin"), b"current identity").unwrap();

        adopt_v019_profile_directories(root.path()).unwrap();

        assert_eq!(
            std::fs::read(current_identity.join("identity.bin")).unwrap(),
            b"current identity"
        );
        assert!(!legacy_identity.exists());
    }

    #[test]
    fn nonempty_v019_and_current_blob_directories_stop_startup() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("iroh-identity_a")).unwrap();
        let legacy_blobs = root.path().join("iroh-blobs_a");
        std::fs::create_dir_all(&legacy_blobs).unwrap();
        std::fs::write(legacy_blobs.join("blobs.db"), b"legacy blobs").unwrap();
        std::fs::create_dir_all(root.path().join("iroh-blobs")).unwrap();

        let error = adopt_v019_profile_directories(root.path()).unwrap_err();

        assert!(error
            .source
            .to_string()
            .contains("blob store directory conflict"));
        assert!(root.path().join("iroh-identity_a").exists());
        assert!(!root.path().join("iroh-identity").exists());
    }

    #[test]
    fn multiple_empty_v019_directories_stop_startup() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("iroh-identity")).unwrap();
        std::fs::create_dir_all(root.path().join("iroh-identity_a")).unwrap();
        std::fs::create_dir_all(root.path().join("iroh-identity_b")).unwrap();

        let error = adopt_v019_profile_directories(root.path()).unwrap_err();

        assert!(error
            .source
            .to_string()
            .contains("multiple v0.19 identity directories found"));
        assert!(root.path().join("iroh-identity_a").exists());
        assert!(root.path().join("iroh-identity_b").exists());
    }
}
