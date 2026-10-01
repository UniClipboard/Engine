use std::fs;
use std::io;

use async_trait::async_trait;
use uc_application::deps::{
    ProfileFactoryResetCapabilityError, ProfileUpgradeBackupError, ProfileUpgradeVersions,
    RetireUpgradeBackupSecurityRecordsPort,
};
use uc_observability_contract::uc_warn;

use super::inventory::{excluded_paths, read_secrets};
use super::record::{
    publish_record, read_file_record, read_record, retire_security_records, FileBackupRecord,
    SecurityBackupRecord, RECORD_KEY,
};
use super::store::{backup_error, ProfileUpgradeBackupStore};
use crate::security::profile_backup_archive::tree::{resolve_source_root, write_selected_tree};

#[async_trait]
impl RetireUpgradeBackupSecurityRecordsPort for ProfileUpgradeBackupStore {
    async fn retire_upgrade_backup_security_records(
        &self,
    ) -> Result<(), ProfileFactoryResetCapabilityError> {
        let store = self.clone();
        let result = tokio::task::spawn_blocking(move || store.retire_security_records_locked())
            .await
            .map_err(backup_error)
            .and_then(|retired| retired);
        if let Err(error) = &result {
            Self::record_failure("retire_security_records", error);
        }
        result.map_err(ProfileFactoryResetCapabilityError::from_source)
    }
}

impl ProfileUpgradeBackupStore {
    /// 出厂重置作废本 profile 的安全记录与记录密钥；文件副本保持可验证、可恢复。
    fn retire_security_records_locked(&self) -> Result<(), ProfileUpgradeBackupError> {
        // 从未做过备份时目录不存在；不为了退役而创建它。
        match fs::symlink_metadata(self.directory()) {
            Ok(_) => {
                let _lease = Self::record_action("acquire_lease", self.lease())?;
                Self::record_action(
                    "retire_security_records",
                    retire_security_records(&self.directory()),
                )?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(backup_error(error)),
        }
        Self::record_action(
            "delete_security_record_key",
            self.secure_storage.delete(RECORD_KEY).map_err(backup_error),
        )
    }

    pub(super) fn preserve_secrets(
        &self,
        target: &ProfileUpgradeVersions,
    ) -> Result<(), ProfileUpgradeBackupError> {
        let _lease = Self::record_action("acquire_lease", self.lease())?;
        Self::record_action("verify_prepared_profile", self.verify(target))?;
        let files = Self::record_action(
            "read_prepared_record",
            read_file_record(&self.directory()).and_then(|record| {
                record
                    .ok_or_else(|| backup_error(io::Error::other("file backup record is missing")))
            }),
        )?;
        // 记录密钥缺失时，旧安全记录已不可能再解密，只是文件副本的派生物：退役后按“尚无记录”
        // 走原有发布路径。只放宽“密钥确定不存在”；存储读取失败、记录损坏或无法解密仍然失败。
        if Self::record_action(
            "read_security_record_key",
            self.secure_storage.get(RECORD_KEY).map_err(backup_error),
        )?
        .is_none()
            && Self::record_action(
                "retire_unusable_security_records",
                retire_security_records(&self.directory()),
            )?
        {
            uc_warn!(
                cause = "record_key_missing",
                "升级备份安全记录的保护密钥缺失，已作废旧记录并重新建立"
            );
        }
        if let Some(existing) = Self::record_action(
            "read_security_record",
            read_record(&self.directory(), self.secure_storage.as_ref()),
        )? {
            if existing.files.receipt == files.receipt
                && existing.files.spool_receipt == files.spool_receipt
                && existing.files.target() == files.target()
            {
                Self::record_action("prune_backups", self.prune_locked())?;
                return Ok(());
            }
        }
        // 只给未变化的文件现场补充安全材料；不能将新资料的密钥配给旧文件。
        Self::record_action("verify_source_files", self.verify_source_files(&files))?;
        let secrets = Self::record_action(
            "read_security_materials",
            read_secrets(&self.paths, &self.profile, self.secure_storage.as_ref()),
        )?;
        Self::record_action("reverify_source_files", self.verify_source_files(&files))?;
        if secrets
            != Self::record_action(
                "confirm_security_materials",
                read_secrets(&self.paths, &self.profile, self.secure_storage.as_ref()),
            )?
        {
            return Self::record_action(
                "confirm_security_materials",
                Err(backup_error(io::Error::other(
                    "profile backup secrets changed",
                ))),
            );
        }
        Self::record_action(
            "publish_security_record",
            publish_record(
                &self.directory(),
                self.secure_storage.as_ref(),
                &SecurityBackupRecord { files, secrets },
            ),
        )?;
        Self::record_action("prune_backups", self.prune_locked())
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
