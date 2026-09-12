//! 停写资料目录的加密归档能力；不负责程序安装、密钥清单或活动资料切换。
//! 调用方必须先停止所有写入者；本模块不打开 SQLite，也不执行迁移。

mod stream;
mod tree;

use std::fs::{self, File, OpenOptions};
use std::io::{self, Seek};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use uc_core::ports::{SecureStorageError, SecureStoragePort};
use uuid::Uuid;
use zeroize::Zeroizing;

use super::MasterKey;
use stream::{ArchiveReader, ArchiveWriter};
use tree::{read_tree, write_tree};

/// 由产品提供的精确来源记录；归档层不把这些字段当作签名验证结果。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileBackupSource {
    pub product_version: String,
    pub engine_version: String,
    pub platform: String,
    pub architecture: String,
    pub installation_channel: String,
    pub artifact_digest: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileArchiveReceipt {
    pub archive_id: [u8; 16],
    pub source: ProfileBackupSource,
    pub archive_digest: [u8; 32],
}

#[derive(Debug, thiserror::Error)]
pub enum ProfileBackupArchiveError {
    #[error("profile backup storage failed")]
    Storage {
        #[source]
        source: io::Error,
    },
    #[error("profile backup secure storage failed")]
    SecureStorage {
        #[source]
        source: SecureStorageError,
    },
    #[error("profile backup protection failed")]
    Protection {
        #[source]
        source: anyhow::Error,
    },
    #[error("profile backup key is missing")]
    KeyMissing,
    #[error("profile backup source changed")]
    SourceChanged,
    #[error("profile backup does not match the requested archive")]
    StateChanged,
    #[error("profile backup directories overlap")]
    OverlappingDirectories,
}

impl From<io::Error> for ProfileBackupArchiveError {
    fn from(source: io::Error) -> Self {
        Self::Storage { source }
    }
}

impl From<SecureStorageError> for ProfileBackupArchiveError {
    fn from(source: SecureStorageError) -> Self {
        Self::SecureStorage { source }
    }
}

/// 只提供单个受管目录的归档与还原，不代表完整用户资料或可用版本回退点。
/// 备份根必须位于来源目录之外，并由宿主限定为当前用户私有目录。
pub struct ProfileBackupArchive {
    directory: PathBuf,
    secure_storage: Arc<dyn SecureStoragePort>,
}

impl ProfileBackupArchive {
    pub fn new(directory: PathBuf, secure_storage: Arc<dyn SecureStoragePort>) -> Self {
        Self {
            directory,
            secure_storage,
        }
    }

    /// 成功前进行来源复读和完整归档认证；失败只留下随机命名的加密未完成文件。
    /// 本能力不自动清理既有归档，也不创建明文数据库快照。
    pub fn capture(
        &self,
        source_root: &Path,
        source: ProfileBackupSource,
    ) -> Result<ProfileArchiveReceipt, ProfileBackupArchiveError> {
        tree::require_directory(source_root)?;
        let source_root = fs::canonicalize(source_root)?;
        // 先检查最近的既有父目录，避免在来源树内创建备份目录后才拒绝。
        tree::require_disjoint_destination(&source_root, &self.directory)?;
        tree::create_private_directory(&self.directory)?;
        let directory = fs::canonicalize(&self.directory)?;
        if directory.starts_with(&source_root) || source_root.starts_with(&directory) {
            return Err(ProfileBackupArchiveError::OverlappingDirectories);
        }
        let id = Uuid::new_v4();
        let key = MasterKey::generate().map_err(protection_error)?;
        if self.secure_storage.get(&key_name(id))?.is_some() {
            return Err(ProfileBackupArchiveError::StateChanged);
        }
        self.secure_storage.set(&key_name(id), key.as_bytes())?;
        let pending = directory.join(format!("{id}.partial"));
        let output = private_new_file(&pending)?;
        let writer = ArchiveWriter::new(output, &key)?;
        let (writer, first_digest) = write_tree(writer, &source_root, &source)?;
        writer.finish()?.sync_all()?;
        // 从持久安全存储重新取钥验证，不能只凭仍在内存中的生成密钥发布备份。
        if self.load_key(id)?.as_bytes() != key.as_bytes() {
            return Err(ProfileBackupArchiveError::StateChanged);
        }
        let (_, second_digest) = write_tree(io::sink(), &source_root, &source)?;
        if first_digest != second_digest {
            return Err(ProfileBackupArchiveError::SourceChanged);
        }
        let (verified_source, verified_digest) =
            read_tree(ArchiveReader::new(File::open(&pending)?, &key)?, None)?;
        if verified_source != source || verified_digest != first_digest {
            return Err(ProfileBackupArchiveError::StateChanged);
        }
        // 硬链接发布具有“不覆盖既有目标”的语义；未完成文件不作为已验证归档返回。
        fs::hard_link(&pending, self.path(id))?;
        sync_directory(&directory)?;
        fs::remove_file(&pending)?;
        sync_directory(&directory)?;
        Ok(ProfileArchiveReceipt {
            archive_id: *id.as_bytes(),
            source,
            archive_digest: verified_digest,
        })
    }

    pub fn verify(&self, receipt: &ProfileArchiveReceipt) -> Result<(), ProfileBackupArchiveError> {
        let id = Uuid::from_bytes(receipt.archive_id);
        let key = self.load_key(id)?;
        let (source, digest) = read_tree(
            ArchiveReader::new(tree::open_regular_file(&self.path(id))?, &key)?,
            None,
        )?;
        check_receipt(receipt, &source, digest)
    }

    /// 仅还原到尚不存在的隔离目录；绝不覆盖或切换当前资料。
    /// 活动资料、系统安全存储和程序切换由后续完整维护流程负责。
    pub fn restore_to_new_directory(
        &self,
        receipt: &ProfileArchiveReceipt,
        destination: &Path,
    ) -> Result<(), ProfileBackupArchiveError> {
        let id = Uuid::from_bytes(receipt.archive_id);
        let key = self.load_key(id)?;
        // 完整认证先于任何资料写出，包括尾部结束认证和追加数据检查。
        let mut input = tree::open_regular_file(&self.path(id))?;
        let (source, digest) = read_tree(ArchiveReader::new(&mut input, &key)?, None)?;
        check_receipt(receipt, &source, digest)?;
        input.rewind()?;
        let parent = destination.parent().ok_or_else(stream::invalid_archive)?;
        tree::require_directory(parent)?;
        tree::require_disjoint_destination(&fs::canonicalize(&self.directory)?, destination)?;
        tree::create_private_directory_new(destination)?;
        // 中断后留下的目录不是成功结果；重试必须另选新的隔离目录。
        let (source, digest) = read_tree(ArchiveReader::new(input, &key)?, Some(destination))?;
        check_receipt(receipt, &source, digest)?;
        sync_directory(destination)?;
        sync_directory(parent)?;
        Ok(())
    }

    fn path(&self, id: Uuid) -> PathBuf {
        self.directory.join(format!("{id}.archive"))
    }

    fn load_key(&self, id: Uuid) -> Result<MasterKey, ProfileBackupArchiveError> {
        let bytes = Zeroizing::new(
            self.secure_storage
                .get(&key_name(id))?
                .ok_or(ProfileBackupArchiveError::KeyMissing)?,
        );
        MasterKey::from_bytes(&bytes).map_err(protection_error)
    }
}

fn check_receipt(
    receipt: &ProfileArchiveReceipt,
    source: &ProfileBackupSource,
    digest: [u8; 32],
) -> Result<(), ProfileBackupArchiveError> {
    if &receipt.source != source || receipt.archive_digest != digest {
        return Err(ProfileBackupArchiveError::StateChanged);
    }
    Ok(())
}

fn key_name(id: Uuid) -> String {
    format!("profile_backup_archive_key:v1:{id}")
}

fn protection_error(source: impl Into<anyhow::Error>) -> ProfileBackupArchiveError {
    ProfileBackupArchiveError::Protection {
        source: source.into(),
    }
}

fn private_new_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests;
