//! 清理已验证升级留下的旧资料和临时副本；执行时机仍由完整升级流程决定。

use super::journal::UpgradeJournalV1;
use super::target::{legacy_space_generation_directory, TargetGenerationStager};
use super::{ProfileStorageUpgrade, ProfileStorageUpgradeError};
use std::path::{Path, PathBuf};

impl ProfileStorageUpgrade {
    pub(super) fn cleanup(
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
