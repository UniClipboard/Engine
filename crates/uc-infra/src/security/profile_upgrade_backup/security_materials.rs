use std::fs;
use std::io;
use uc_application::deps::{ProfileUpgradeBackupError, ProfileUpgradeVersions};

use super::inventory::{excluded_paths, read_secrets};
use super::record::{
    publish_record, read_file_record, read_record, FileBackupRecord, SecurityBackupRecord,
};
use super::store::{backup_error, ProfileUpgradeBackupStore};
use crate::security::profile_backup_archive::tree::{resolve_source_root, write_selected_tree};

impl ProfileUpgradeBackupStore {
    pub(super) fn preserve_secrets(
        &self,
        target: &ProfileUpgradeVersions,
    ) -> Result<(), ProfileUpgradeBackupError> {
        let _lease = self.lease()?;
        self.verify(target)?;
        let files = read_file_record(&self.directory())?
            .ok_or_else(|| backup_error(io::Error::other("file backup record is missing")))?;
        if let Some(existing) = read_record(&self.directory(), self.secure_storage.as_ref())? {
            if existing.files.receipt == files.receipt
                && existing.files.spool_receipt == files.spool_receipt
                && existing.files.target() == files.target()
            {
                return Ok(());
            }
        }
        // 只给未变化的文件现场补充安全材料；不能将新资料的密钥配给旧文件。
        self.verify_source_files(&files)?;
        let secrets = read_secrets(&self.paths, &self.profile, self.secure_storage.as_ref())?;
        self.verify_source_files(&files)?;
        if secrets != read_secrets(&self.paths, &self.profile, self.secure_storage.as_ref())? {
            return Err(backup_error(io::Error::other(
                "profile backup secrets changed",
            )));
        }
        publish_record(
            &self.directory(),
            self.secure_storage.as_ref(),
            &SecurityBackupRecord { files, secrets },
        )
    }

    fn verify_source_files(
        &self,
        record: &FileBackupRecord,
    ) -> Result<(), ProfileUpgradeBackupError> {
        let root = resolve_source_root(&self.paths.app_data_root_dir).map_err(backup_error)?;
        let source = self.source()?;
        let (_, digest) = write_selected_tree(
            io::sink(),
            &root,
            &source,
            &excluded_paths(&self.paths, &root),
        )
        .map_err(backup_error)?;
        if source != record.receipt.source || digest != record.receipt.archive_digest {
            return Err(backup_error(io::Error::other(
                "profile backup source changed",
            )));
        }
        match (
            &record.spool_receipt,
            fs::symlink_metadata(&self.paths.spool_dir),
        ) {
            (Some(receipt), Ok(_)) => {
                let (_, digest) =
                    write_selected_tree(io::sink(), &self.paths.spool_dir, &source, &[])
                        .map_err(backup_error)?;
                if digest != receipt.archive_digest {
                    return Err(backup_error(io::Error::other(
                        "profile backup spool changed",
                    )));
                }
            }
            (None, Err(error)) if error.kind() == io::ErrorKind::NotFound => {}
            (_, Err(error)) => return Err(backup_error(error)),
            _ => {
                return Err(backup_error(io::Error::other(
                    "profile backup spool changed",
                )))
            }
        }
        Ok(())
    }
}
