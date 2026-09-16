use uc_application::deps::ProfileUpgradeBackupPort;

use crate::error_codes::{DELETE_UPGRADE_BACKUP_FAILED_CODE, LIST_UPGRADE_BACKUPS_FAILED_CODE};
use crate::{
    DeleteUpgradeBackupInput, EngineError, EngineErrorCategory, OperationResult,
    UpgradeBackupSummary,
};

pub async fn execute_list_upgrade_backups(
    backups: &dyn ProfileUpgradeBackupPort,
) -> Result<OperationResult, EngineError> {
    backups
        .list_backups()
        .await
        .map(|entries| {
            OperationResult::UpgradeBackups(
                entries
                    .into_iter()
                    .map(|entry| UpgradeBackupSummary {
                        id: entry.id,
                        created_at_ms: entry.created_at_ms,
                        source_product: entry.source_product,
                        source_engine: entry.source_engine,
                        target_product: entry.target_product,
                        target_engine: entry.target_engine,
                        size_bytes: entry.size_bytes,
                    })
                    .collect(),
            )
        })
        .map_err(|_| {
            EngineError::new(
                LIST_UPGRADE_BACKUPS_FAILED_CODE,
                EngineErrorCategory::Internal,
                false,
            )
        })
}

pub async fn execute_delete_upgrade_backup(
    backups: &dyn ProfileUpgradeBackupPort,
    input: DeleteUpgradeBackupInput,
) -> Result<OperationResult, EngineError> {
    if uuid::Uuid::parse_str(&input.id).is_err() {
        return Err(EngineError::new(
            DELETE_UPGRADE_BACKUP_FAILED_CODE,
            EngineErrorCategory::InvalidInput,
            false,
        ));
    }
    backups.delete_backup(&input.id).await.map_err(|_| {
        EngineError::new(
            DELETE_UPGRADE_BACKUP_FAILED_CODE,
            EngineErrorCategory::Internal,
            false,
        )
    })?;
    Ok(OperationResult::UpgradeBackupDeleted { id: input.id })
}
