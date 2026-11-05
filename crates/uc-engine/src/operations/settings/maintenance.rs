//! Engine 自有存储与升级维护：占用统计、缓存清理、升级状态与升级备份。

use uc_application::deps::ProfileUpgradeBackupPort;
use uc_application::facade::{AppFacade, StorageFacadeError, StorageStatsView, UpgradeStatus};
use uc_observability_contract::uc_error;

use crate::error_codes::*;
use crate::{
    DeleteUpgradeBackupInput, EngineError, EngineErrorCategory, OperationResult,
    StorageStatsSummary, UpgradeBackupSummary, UpgradeStatusSummary,
};

pub async fn execute_query_storage_stats(
    facade: &AppFacade,
) -> Result<OperationResult, EngineError> {
    facade
        .storage_stats()
        .await
        .map(storage_stats_result)
        .map_err(map_storage_error)
}

pub async fn execute_clear_storage_cache(
    facade: &AppFacade,
) -> Result<OperationResult, EngineError> {
    let result = facade
        .clear_storage_cache()
        .await
        .map_err(map_storage_error)?;
    Ok(OperationResult::StorageCacheCleared {
        freed_bytes: result.freed_bytes,
    })
}

pub(crate) fn storage_stats_result(view: StorageStatsView) -> OperationResult {
    OperationResult::StorageStats(StorageStatsSummary {
        total_bytes: view.total_bytes,
        database_bytes: view.database_bytes,
        vault_bytes: view.vault_bytes,
        cache_bytes: view.cache_bytes,
        logs_bytes: view.logs_bytes,
    })
}

pub(crate) fn map_storage_error(error: StorageFacadeError) -> EngineError {
    let code = match error {
        StorageFacadeError::Stats(_) => QUERY_STORAGE_STATS_FAILED_CODE,
        StorageFacadeError::ClearCache(_) => CLEAR_STORAGE_CACHE_FAILED_CODE,
    };
    uc_error!(code = code, "engine storage operation failed");
    EngineError::new(code, EngineErrorCategory::Internal, false)
}

pub(crate) async fn execute_query_upgrade_status(
    facade: &AppFacade,
    app_version: &str,
) -> Result<OperationResult, EngineError> {
    let status = facade
        .upgrade_status(app_version)
        .await
        // discarded-source[contract-boundary]: the public error carries a stable code only, the owner records the failure classification
        .map_err(|_| internal_error(QUERY_UPGRADE_STATUS_FAILED_CODE))?;
    Ok(OperationResult::UpgradeStatus(map_status(
        status,
        app_version,
    )))
}

pub(crate) async fn execute_acknowledge_upgrade(
    facade: &AppFacade,
    app_version: &str,
) -> Result<OperationResult, EngineError> {
    facade
        .acknowledge_upgrade(app_version)
        .await
        // discarded-source[contract-boundary]: the public error carries a stable code only, the owner records the failure classification
        .map_err(|_| internal_error(ACKNOWLEDGE_UPGRADE_FAILED_CODE))?;
    Ok(OperationResult::UpgradeAcknowledged {
        version: app_version.to_string(),
    })
}

fn map_status(status: UpgradeStatus, app_version: &str) -> UpgradeStatusSummary {
    match status {
        UpgradeStatus::FreshInstall => UpgradeStatusSummary::FreshInstall {
            current: app_version.to_string(),
        },
        UpgradeStatus::NoChange => UpgradeStatusSummary::NoChange {
            current: app_version.to_string(),
        },
        UpgradeStatus::Upgraded { from, to } => UpgradeStatusSummary::Upgraded {
            from: from.map(|version| version.to_string()),
            to: to.to_string(),
        },
        UpgradeStatus::Downgraded { from, to } => UpgradeStatusSummary::Downgraded {
            from: from.to_string(),
            to: to.to_string(),
        },
    }
}

fn internal_error(code: u32) -> EngineError {
    EngineError::new(code, EngineErrorCategory::Internal, false)
}

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
        // discarded-source[contract-boundary]: the public error carries a stable code only, the owner records the failure classification
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
    // discarded-source[contract-boundary]: the public error carries a stable code only, the owner records the failure classification
    backups.delete_backup(&input.id).await.map_err(|_| {
        EngineError::new(
            DELETE_UPGRADE_BACKUP_FAILED_CODE,
            EngineErrorCategory::Internal,
            false,
        )
    })?;
    Ok(OperationResult::UpgradeBackupDeleted { id: input.id })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upgrade_status_mapping_uses_the_engine_version_for_terminal_states() {
        assert_eq!(
            map_status(UpgradeStatus::FreshInstall, "1.2.3"),
            UpgradeStatusSummary::FreshInstall {
                current: "1.2.3".into(),
            }
        );
        assert_eq!(
            map_status(UpgradeStatus::NoChange, "1.2.3"),
            UpgradeStatusSummary::NoChange {
                current: "1.2.3".into(),
            }
        );
    }
}
