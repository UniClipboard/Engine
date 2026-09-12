use std::fs;
use std::io::{self, Seek};
use std::path::{Path, PathBuf};

use uuid::Uuid;

use super::error::invalid_archive;
use super::tree::read_tree;
use super::{
    private_new_file, sync_directory, tree, ProfileArchiveReceipt, ProfileBackupArchiveError,
    ProfileBackupSource,
};

/// 只提供单个受管目录的归档与还原，不代表完整用户资料或可用版本回退点。
/// 备份根必须位于来源目录之外，并由宿主限定为当前用户私有目录。
pub struct ProfileBackupArchive {
    directory: PathBuf,
}

impl ProfileBackupArchive {
    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    /// 成功前复读来源及完整归档；失败保留未完成副本，不改变原文件保护状态。
    /// 只用于当前用户私有的本机恢复副本，不是可分享导出。
    pub fn capture(
        &self,
        source_root: &Path,
        source: ProfileBackupSource,
    ) -> Result<ProfileArchiveReceipt, ProfileBackupArchiveError> {
        self.capture_selected(source_root, source, &[])
    }

    /// 仅供资料清单适配器使用；排除项必须是来源根下的完整子树或文件。
    pub(in super::super) fn capture_selected(
        &self,
        source_root: &Path,
        source: ProfileBackupSource,
        excluded: &[PathBuf],
    ) -> Result<ProfileArchiveReceipt, ProfileBackupArchiveError> {
        let source_root = tree::resolve_source_root(source_root)?;
        for path in excluded {
            if path == &source_root || !path.starts_with(&source_root) {
                return Err(ProfileBackupArchiveError::OverlappingDirectories);
            }
        }
        tree::require_disjoint_destination(&source_root, &self.directory)?;
        tree::create_private_directory(&self.directory)?;
        let directory = fs::canonicalize(&self.directory)?;
        if directory.starts_with(&source_root) || source_root.starts_with(&directory) {
            return Err(ProfileBackupArchiveError::OverlappingDirectories);
        }
        let id = Uuid::new_v4();
        let pending = directory.join(format!("{id}.partial"));
        let output = private_new_file(&pending)?;
        let (writer, first_digest) =
            tree::write_selected_tree(output, &source_root, &source, excluded)?;
        writer.sync_all()?;
        let (_, second_digest) =
            tree::write_selected_tree(io::sink(), &source_root, &source, excluded)?;
        if first_digest != second_digest {
            return Err(ProfileBackupArchiveError::SourceChanged);
        }
        let (verified_source, verified_digest) =
            read_tree(tree::open_regular_file(&pending)?, None)?;
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
        let (source, digest) = read_tree(tree::open_regular_file(&self.path(id))?, None)?;
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
        // 完整校验先于任何资料写出，包括尾部和追加数据检查。
        let mut input = tree::open_regular_file(&self.path(id))?;
        let (source, digest) = read_tree(&mut input, None)?;
        check_receipt(receipt, &source, digest)?;
        input.rewind()?;
        let parent = destination.parent().ok_or_else(invalid_archive)?;
        tree::require_directory(parent)?;
        tree::require_disjoint_destination(&fs::canonicalize(&self.directory)?, destination)?;
        tree::create_private_directory_new(destination)?;
        // 中断后留下的目录不是成功结果；重试必须另选新的隔离目录。
        let (source, digest) = read_tree(input, Some(destination))?;
        check_receipt(receipt, &source, digest)?;
        sync_directory(destination)?;
        sync_directory(parent)?;
        Ok(())
    }

    pub(super) fn path(&self, id: Uuid) -> PathBuf {
        self.directory.join(format!("{id}.archive"))
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
