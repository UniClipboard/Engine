use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};
use uc_application::deps::{ProfileUpgradeBackupError, ProfileUpgradeVersions};
use uc_core::ports::SecureStoragePort;
use uuid::Uuid;
use zeroize::Zeroizing;

use super::inventory::SecretValue;
use super::security_stream::{ArchiveReader, ArchiveWriter};
use super::store::backup_error;
use crate::security::profile_backup_archive::tree::open_regular_file;
use crate::security::profile_backup_archive::{private_new_file, sync_directory};
use crate::security::{MasterKey, ProfileArchiveReceipt};

pub(super) const RECORD_KEY: &str = "profile_upgrade_backup_record_key:v1";
const MAX_RECORD_BYTES: u64 = 256 * 1024;

#[derive(Serialize, Deserialize)]
pub(super) struct FileBackupRecord {
    pub schema: u32,
    pub target_product: String,
    pub target_engine: String,
    pub receipt: ProfileArchiveReceipt,
    pub spool_receipt: Option<ProfileArchiveReceipt>,
}

#[derive(Serialize, Deserialize)]
pub(super) struct SecurityBackupRecord {
    pub files: FileBackupRecord,
    pub secrets: Vec<SecretValue>,
}

impl FileBackupRecord {
    pub fn target(&self) -> ProfileUpgradeVersions {
        ProfileUpgradeVersions {
            product: self.target_product.clone(),
            engine: self.target_engine.clone(),
        }
    }
}

pub(super) fn read_bounded(
    path: &Path,
) -> Result<Option<Zeroizing<Vec<u8>>>, ProfileUpgradeBackupError> {
    let file = match open_regular_file(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(backup_error(error)),
    };
    let mut bytes = Zeroizing::new(Vec::new());
    file.take(MAX_RECORD_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(backup_error)?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(backup_error(io::Error::other(
            "profile backup record exceeds limit",
        )));
    }
    Ok(Some(bytes))
}

fn load_key(storage: &dyn SecureStoragePort) -> Result<MasterKey, ProfileUpgradeBackupError> {
    let bytes = Zeroizing::new(
        storage
            .get(RECORD_KEY)
            .map_err(backup_error)?
            .ok_or_else(|| {
                backup_error(io::Error::other("profile backup record key is missing"))
            })?,
    );
    MasterKey::from_bytes(&bytes).map_err(backup_error)
}

pub(super) fn read_record(
    directory: &Path,
    storage: &dyn SecureStoragePort,
) -> Result<Option<SecurityBackupRecord>, ProfileUpgradeBackupError> {
    read_record_path(&directory.join("security-current"), storage)
}

pub(super) fn read_record_path(
    path: &Path,
    storage: &dyn SecureStoragePort,
) -> Result<Option<SecurityBackupRecord>, ProfileUpgradeBackupError> {
    let Some(ciphertext) = read_bounded(path)? else {
        return Ok(None);
    };
    let key = load_key(storage)?;
    let mut plaintext = Zeroizing::new(Vec::new());
    ArchiveReader::new(ciphertext.as_slice(), &key)
        .map_err(backup_error)?
        .take(MAX_RECORD_BYTES + 1)
        .read_to_end(&mut plaintext)
        .map_err(backup_error)?;
    if plaintext.len() as u64 > MAX_RECORD_BYTES {
        return Err(backup_error(io::Error::other(
            "profile backup record exceeds limit",
        )));
    }
    let record: SecurityBackupRecord = serde_json::from_slice(&plaintext).map_err(backup_error)?;
    if record.files.schema != 3 {
        return Err(backup_error(io::Error::other(
            "profile backup record is unsupported",
        )));
    }
    Ok(Some(record))
}

pub(super) fn publish_record(
    directory: &Path,
    storage: &dyn SecureStoragePort,
    record: &SecurityBackupRecord,
) -> Result<(), ProfileUpgradeBackupError> {
    // 已有安全材料记录缺钥时不能生成替代值；原样文件记录不依赖此密钥。
    if storage.get(RECORD_KEY).map_err(backup_error)?.is_none() {
        if fs::symlink_metadata(directory.join("security-current")).is_ok() {
            return Err(backup_error(io::Error::other(
                "profile backup record key is missing",
            )));
        }
        let key = MasterKey::generate().map_err(backup_error)?;
        storage
            .set(RECORD_KEY, key.as_bytes())
            .map_err(backup_error)?;
    }
    let key = load_key(storage)?;
    let plaintext = Zeroizing::new(serde_json::to_vec(record).map_err(backup_error)?);
    if plaintext.len() as u64 > MAX_RECORD_BYTES - 1024 {
        return Err(backup_error(io::Error::other(
            "profile backup record exceeds limit",
        )));
    }
    let path = directory.join(format!("{}.record", Uuid::new_v4()));
    let file = private_new_file(&path).map_err(backup_error)?;
    let mut writer = ArchiveWriter::new(file, &key).map_err(backup_error)?;
    writer.write_all(&plaintext).map_err(backup_error)?;
    writer
        .finish()
        .map_err(backup_error)?
        .sync_all()
        .map_err(backup_error)?;
    let pending = directory.join(format!("{}.pointer", Uuid::new_v4()));
    fs::hard_link(&path, &pending).map_err(backup_error)?;
    fs::rename(&pending, directory.join("security-current")).map_err(backup_error)?;
    sync_directory(directory).map_err(backup_error)?;
    let reopened = read_record(directory, storage)?
        .ok_or_else(|| backup_error(io::Error::other("profile backup record is missing")))?;
    if reopened.files.receipt != record.files.receipt
        || reopened.files.spool_receipt != record.files.spool_receipt
        || reopened.secrets != record.secrets
        || reopened.files.target() != record.files.target()
    {
        return Err(backup_error(io::Error::other(
            "profile backup record changed",
        )));
    }
    Ok(())
}

pub(super) fn read_file_record(
    directory: &Path,
) -> Result<Option<FileBackupRecord>, ProfileUpgradeBackupError> {
    read_file_record_path(&directory.join("current"))
}

pub(super) fn read_file_record_path(
    path: &Path,
) -> Result<Option<FileBackupRecord>, ProfileUpgradeBackupError> {
    let Some(bytes) = read_bounded(path)? else {
        return Ok(None);
    };
    let record: FileBackupRecord = serde_json::from_slice(&bytes).map_err(backup_error)?;
    if record.schema != 3 {
        return Err(backup_error(io::Error::other("invalid file backup record")));
    }
    Ok(Some(record))
}

pub(super) fn publish_file_record(
    directory: &Path,
    record: &FileBackupRecord,
) -> Result<(), ProfileUpgradeBackupError> {
    let bytes = serde_json::to_vec(record).map_err(backup_error)?;
    let path = directory.join(format!(
        "{}.files",
        Uuid::from_bytes(record.receipt.archive_id)
    ));
    let mut file = private_new_file(&path).map_err(backup_error)?;
    file.write_all(&bytes).map_err(backup_error)?;
    file.sync_all().map_err(backup_error)?;
    let pending = directory.join(format!("{}.pointer", Uuid::new_v4()));
    fs::hard_link(&path, &pending).map_err(backup_error)?;
    fs::rename(pending, directory.join("current")).map_err(backup_error)?;
    sync_directory(directory).map_err(backup_error)?;
    let reopened = read_file_record(directory)?
        .ok_or_else(|| backup_error(io::Error::other("file backup record is missing")))?;
    if reopened.receipt != record.receipt
        || reopened.spool_receipt != record.spool_receipt
        || reopened.target() != record.target()
    {
        return Err(backup_error(io::Error::other("file backup record changed")));
    }
    Ok(())
}
