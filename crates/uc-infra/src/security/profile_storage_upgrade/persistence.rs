use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use zeroize::Zeroizing;

use super::journal::UpgradeJournalV1;
use super::ProfileStorageUpgradeError;
use crate::security::{AdmissionKeyError, AdmissionKeyManager};

const UPGRADE_DIRECTORY: &str = "profile-storage-upgrade";
const UPGRADE_LEASE_FILE: &str = ".lease";
const UPGRADE_JOURNAL_FILE: &str = ".journal-v1";
const UPGRADE_JOURNAL_PURPOSE: &[u8] = b"profile-storage-upgrade-journal-v1";
const MAX_ENCRYPTED_JOURNAL_BYTES: u64 = 64 * 1024;

pub(super) struct UpgradePersistence {
    directory: PathBuf,
    lease_path: PathBuf,
    journal_path: PathBuf,
    keys: Arc<AdmissionKeyManager>,
}

pub(super) enum UpgradeLeaseResult {
    Acquired(UpgradeLease),
    Busy,
}

pub(super) struct UpgradeLease {
    _file: File,
}

impl UpgradePersistence {
    pub(super) fn new(profile_root: PathBuf, keys: Arc<AdmissionKeyManager>) -> Self {
        let directory = profile_root.join(UPGRADE_DIRECTORY);
        Self {
            lease_path: directory.join(UPGRADE_LEASE_FILE),
            journal_path: directory.join(UPGRADE_JOURNAL_FILE),
            directory,
            keys,
        }
    }

    pub(super) fn try_acquire_lease(
        &self,
    ) -> Result<UpgradeLeaseResult, ProfileStorageUpgradeError> {
        std::fs::create_dir_all(&self.directory).map_err(storage_error)?;
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&self.lease_path)
            .map_err(storage_error)?;
        match file.try_lock() {
            Ok(()) => Ok(UpgradeLeaseResult::Acquired(UpgradeLease { _file: file })),
            Err(TryLockError::WouldBlock) => Ok(UpgradeLeaseResult::Busy),
            Err(TryLockError::Error(source)) => Err(storage_error(source)),
        }
    }

    pub(super) async fn load_journal(
        &self,
    ) -> Result<Option<UpgradeJournalV1>, ProfileStorageUpgradeError> {
        let file = match tokio::fs::File::open(&self.journal_path).await {
            Ok(file) => file,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(storage_error(source)),
        };
        let mut ciphertext = Vec::new();
        file.take(MAX_ENCRYPTED_JOURNAL_BYTES + 1)
            .read_to_end(&mut ciphertext)
            .await
            .map_err(storage_error)?;
        if ciphertext.len() as u64 > MAX_ENCRYPTED_JOURNAL_BYTES {
            return Err(ProfileStorageUpgradeError::Corrupt {
                source: anyhow::anyhow!("encrypted profile storage upgrade journal is too large"),
            });
        }
        let plaintext = Zeroizing::new(
            self.keys
                .open_profile_payload(UPGRADE_JOURNAL_PURPOSE, &ciphertext)
                .map_err(security_error)?,
        );
        let journal: UpgradeJournalV1 = postcard::from_bytes(&plaintext).map_err(|source| {
            ProfileStorageUpgradeError::Corrupt {
                source: anyhow::Error::new(source)
                    .context("decode profile storage upgrade journal"),
            }
        })?;
        journal.validate()?;
        Ok(Some(journal))
    }

    pub(super) async fn save_new_journal(
        &self,
        journal: &UpgradeJournalV1,
    ) -> Result<(), ProfileStorageUpgradeError> {
        journal.validate()?;
        if tokio::fs::try_exists(&self.journal_path)
            .await
            .map_err(storage_error)?
        {
            return Err(ProfileStorageUpgradeError::Storage {
                source: anyhow::anyhow!("profile storage upgrade journal already exists"),
            });
        }
        let plaintext = Zeroizing::new(postcard::to_stdvec(journal).map_err(|source| {
            ProfileStorageUpgradeError::Corrupt {
                source: anyhow::Error::new(source)
                    .context("encode profile storage upgrade journal"),
            }
        })?);
        let ciphertext = self
            .keys
            .seal_profile_payload(UPGRADE_JOURNAL_PURPOSE, &plaintext)
            .map_err(security_error)?;
        write_atomically(&self.journal_path, &ciphertext).await
    }

    pub(super) async fn save_journal(
        &self,
        journal: &UpgradeJournalV1,
    ) -> Result<(), ProfileStorageUpgradeError> {
        journal.validate()?;
        let plaintext = Zeroizing::new(postcard::to_stdvec(journal).map_err(|source| {
            ProfileStorageUpgradeError::Corrupt {
                source: anyhow::Error::new(source)
                    .context("encode profile storage upgrade journal"),
            }
        })?);
        let ciphertext = self
            .keys
            .seal_profile_payload(UPGRADE_JOURNAL_PURPOSE, &plaintext)
            .map_err(security_error)?;
        write_atomically(&self.journal_path, &ciphertext).await
    }
}

async fn write_atomically(
    path: &Path,
    ciphertext: &[u8],
) -> Result<(), ProfileStorageUpgradeError> {
    let parent = path
        .parent()
        .ok_or_else(|| ProfileStorageUpgradeError::Storage {
            source: anyhow::anyhow!("profile storage upgrade journal parent is missing"),
        })?;
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(storage_error)?;
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = async {
        let mut file = tokio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .await
            .map_err(storage_error)?;
        file.write_all(ciphertext).await.map_err(storage_error)?;
        file.sync_all().await.map_err(storage_error)?;
        drop(file);
        replace_file_atomically(&temporary, path).map_err(storage_error)?;
        sync_parent_directory(parent).map_err(storage_error)
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&temporary).await;
    }
    result
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
        source: anyhow::Error::new(source).context("persist profile storage upgrade state"),
    }
}

fn security_error(source: AdmissionKeyError) -> ProfileStorageUpgradeError {
    ProfileStorageUpgradeError::Security {
        source: anyhow::Error::new(source).context("protect profile storage upgrade journal"),
    }
}
