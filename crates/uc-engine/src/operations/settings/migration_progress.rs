//! Shared switch-space migration progress query.

use crate::error_codes::*;

use tracing::error;
use uc_application::facade::space_setup::{MigrationPhaseKind, QueryMigrationProgressError};
use uc_application::facade::AppFacade;

use crate::{
    EngineError, EngineErrorCategory, MigrationPhaseSummary, MigrationProgressSummary,
    OperationResult,
};

pub async fn execute_query_migration_progress(
    facade: &AppFacade,
) -> Result<OperationResult, EngineError> {
    let progress = facade
        .query_migration_progress()
        .await
        .map_err(|error| match error {
            QueryMigrationProgressError::StorageFailed(_)
            | QueryMigrationProgressError::Internal(_) => {
                error!(error = %error, "query migration progress failed");
                EngineError::new(
                    QUERY_MIGRATION_PROGRESS_FAILED_CODE,
                    EngineErrorCategory::Internal,
                    false,
                )
            }
        })?;

    Ok(OperationResult::MigrationProgress(
        MigrationProgressSummary {
            phase: progress.phase.map(|phase| match phase {
                MigrationPhaseKind::Prepared => MigrationPhaseSummary::Prepared,
                MigrationPhaseKind::HandshakeDone => MigrationPhaseSummary::HandshakeDone,
                MigrationPhaseKind::Swapped => MigrationPhaseSummary::Swapped,
            }),
            backup_record_count: progress.backup_record_count,
        },
    ))
}
