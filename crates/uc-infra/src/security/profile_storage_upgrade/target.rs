use std::path::{Path, PathBuf};

use crate::config_migration::db_snapshot;
use crate::db::pool::DbPool;

use super::journal::UpgradeJournalV1;
use super::ProfileStorageUpgradeError;

const PROFILE_DATA_DIRECTORY: &str = "profile-data-generations";
const SPACE_CONTROL_DIRECTORY: &str = "space-control-generations";
const TARGET_PROFILE_DATABASE: &str = "profile.sqlite";
const TARGET_CONTROL_DATABASE: &str = "control.sqlite";
const GENERATION_PATH_DOMAIN: &[u8] = b"uniclipboard/profile-upgrade-generation-path/v1\0";

/// 升级器内部唯一拥有 source snapshot 与 target generation 物理布局的组件。
pub(super) struct TargetGenerationStager {
    root: PathBuf,
    source_pool: DbPool,
}

pub(super) struct StagedTarget {
    pub(super) source_snapshot_digest: [u8; 32],
    pub(super) source_database_revision: u64,
}

impl TargetGenerationStager {
    pub(super) fn new(root: PathBuf, source_pool: DbPool) -> Self {
        Self { root, source_pool }
    }

    pub(super) fn stage(
        &self,
        journal: &UpgradeJournalV1,
    ) -> Result<StagedTarget, ProfileStorageUpgradeError> {
        let paths = self.paths(journal);
        let source_database_revision = self.source_revision()?;
        let snapshot = db_snapshot::snapshot_to_bytes(&self.source_pool, &paths.scratch).map_err(
            |source| ProfileStorageUpgradeError::Storage {
                source: anyhow::Error::new(source)
                    .context("snapshot source profile database for V3 staging"),
            },
        )?;
        if self.source_revision()? != source_database_revision {
            return Err(ProfileStorageUpgradeError::SourceChanged);
        }
        let source_snapshot_digest = *blake3::hash(&snapshot).as_bytes();
        write_target(&paths.profile_database, &snapshot)?;
        write_target(&paths.control_database, &snapshot)?;
        self.verify_with_digest(journal, source_snapshot_digest)?;
        Ok(StagedTarget {
            source_snapshot_digest,
            source_database_revision,
        })
    }

    pub(super) fn verify(
        &self,
        journal: &UpgradeJournalV1,
    ) -> Result<(), ProfileStorageUpgradeError> {
        let digest = journal.source_snapshot_digest().ok_or_else(|| {
            ProfileStorageUpgradeError::Corrupt {
                source: anyhow::anyhow!("profile upgrade snapshot digest is missing"),
            }
        })?;
        let revision = journal.source_database_revision().ok_or_else(|| {
            ProfileStorageUpgradeError::Corrupt {
                source: anyhow::anyhow!("profile upgrade source revision is missing"),
            }
        })?;
        if self.source_revision()? != revision {
            return Err(ProfileStorageUpgradeError::SourceChanged);
        }
        self.verify_with_digest(journal, digest)
    }

    fn source_revision(&self) -> Result<u64, ProfileStorageUpgradeError> {
        self.source_pool.persistent_revision().map_err(|source| {
            ProfileStorageUpgradeError::Storage {
                source: source.context("read source profile database revision for V3 staging"),
            }
        })
    }

    fn verify_with_digest(
        &self,
        journal: &UpgradeJournalV1,
        expected: [u8; 32],
    ) -> Result<(), ProfileStorageUpgradeError> {
        let paths = self.paths(journal);
        for path in [&paths.profile_database, &paths.control_database] {
            let bytes = std::fs::read(path).map_err(storage_error)?;
            if *blake3::hash(&bytes).as_bytes() != expected {
                return Err(ProfileStorageUpgradeError::Corrupt {
                    source: anyhow::anyhow!("profile upgrade staged database digest mismatch"),
                });
            }
        }
        Ok(())
    }

    fn paths(&self, journal: &UpgradeJournalV1) -> TargetPaths {
        let profile_directory = self
            .root
            .join(PROFILE_DATA_DIRECTORY)
            .join(generation_token(journal.target_profile_data_generation()));
        let control_directory = self
            .root
            .join(SPACE_CONTROL_DIRECTORY)
            .join(generation_token(journal.target_space_control_generation()));
        TargetPaths {
            scratch: self
                .root
                .join("profile-storage-upgrade")
                .join("source.snapshot.tmp"),
            profile_database: profile_directory.join(TARGET_PROFILE_DATABASE),
            control_database: control_directory.join(TARGET_CONTROL_DATABASE),
        }
    }
}

struct TargetPaths {
    scratch: PathBuf,
    profile_database: PathBuf,
    control_database: PathBuf,
}

fn generation_token(generation: &[u8; 16]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(GENERATION_PATH_DOMAIN);
    hasher.update(generation);
    hasher.finalize().to_hex().to_string()
}

fn write_target(path: &Path, bytes: &[u8]) -> Result<(), ProfileStorageUpgradeError> {
    let parent = path
        .parent()
        .ok_or_else(|| ProfileStorageUpgradeError::Storage {
            source: anyhow::anyhow!("profile upgrade target parent is missing"),
        })?;
    std::fs::create_dir_all(parent).map_err(storage_error)?;
    if path.is_file() {
        let existing = std::fs::read(path).map_err(storage_error)?;
        if existing == bytes {
            return Ok(());
        }
    }
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        use std::io::Write as _;
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        replace_file_atomically(&temporary, path)?;
        sync_parent_directory(parent)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result.map_err(storage_error)
}

#[cfg(not(windows))]
fn replace_file_atomically(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::rename(source, destination)
}

#[cfg(windows)]
fn replace_file_atomically(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    let wide = |path: &Path| {
        let mut value: Vec<u16> = path.as_os_str().encode_wide().collect();
        value.push(0);
        value
    };
    let source = wide(source);
    let destination = wide(destination);
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(windows))]
fn sync_parent_directory(parent: &Path) -> std::io::Result<()> {
    std::fs::File::open(parent)?.sync_all()
}

#[cfg(windows)]
fn sync_parent_directory(_parent: &Path) -> std::io::Result<()> {
    Ok(())
}

fn storage_error(source: std::io::Error) -> ProfileStorageUpgradeError {
    ProfileStorageUpgradeError::Storage {
        source: anyhow::Error::new(source).context("persist profile upgrade target generation"),
    }
}
