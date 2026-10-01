use crate::error_codes::*;

use std::path::Path;
use std::time::Duration;

use uc_application::facade::{AppFacade, DiagnosticsFacadeError};
use uc_core::ids::RepresentationId;
use uc_observability_contract::{error_source::io_error_kind, uc_warn};

use crate::runtime::host_file::{copy_path_to_host, HostFileCopyError};
use crate::{
    DebugModeUpdateSummary, DiagnosticLogsExportSummary, DiagnosticsStatusSummary, EngineError,
    EngineErrorCategory, ExportDiagnosticLogsInput, HostCapabilityErrorCategory, HostFileAccess,
    OperationResult, UpdateDebugModeInput,
};

pub(crate) async fn execute_query_diagnostics(
    facade: &AppFacade,
) -> Result<OperationResult, EngineError> {
    let status = facade
        .diagnostics_status()
        .await
        // discarded-source[contract-boundary]: the public error carries a stable code only, the owner records the failure classification
        .map_err(|_| internal_error(QUERY_DIAGNOSTICS_FAILED_CODE))?;
    Ok(OperationResult::DiagnosticsStatus(
        DiagnosticsStatusSummary {
            debug_mode: status.debug_mode,
            effective_log_profile: status.effective_log_profile,
            restart_required: status.restart_required,
        },
    ))
}

pub(crate) async fn execute_update_debug_mode(
    facade: &AppFacade,
    input: UpdateDebugModeInput,
) -> Result<OperationResult, EngineError> {
    let result = facade
        .update_debug_mode(input.enabled)
        .await
        // discarded-source[contract-boundary]: the public error carries a stable code only, the owner records the failure classification
        .map_err(|_| internal_error(UPDATE_DEBUG_MODE_FAILED_CODE))?;
    Ok(OperationResult::DebugModeUpdated(DebugModeUpdateSummary {
        debug_mode: result.debug_mode,
        restart_required: result.restart_required,
    }))
}

pub(crate) async fn execute_export_diagnostic_logs(
    facade: &AppFacade,
    files: &dyn HostFileAccess,
    temporary_root: &Path,
    input: ExportDiagnosticLogsInput,
) -> Result<OperationResult, EngineError> {
    if !matches!(
        crate::observability::ProcessObservabilityRuntime::flush_local_logs(Duration::from_secs(1)),
        crate::observability::ObservabilitySignalResult::Completed
    ) {
        return Err(internal_error(EXPORT_DIAGNOSTIC_LOGS_FAILED_CODE));
    }
    let export_dir = create_export_dir(&temporary_root)?;

    let exported = facade
        .export_diagnostic_logs(input.since_hours, export_dir.clone())
        .await;
    let result = match exported {
        Ok(exported) => copy_path_to_host(files, &input.destination, Path::new(&exported.path))
            .await
            .map_err(|error| {
                error.record();
                map_copy_error(error)
            })
            .map(|()| {
                OperationResult::DiagnosticLogsExported(DiagnosticLogsExportSummary {
                    included_files: exported.included_files,
                    since_unix_ms: exported.since.timestamp_millis(),
                })
            }),
        Err(error) => Err(map_diagnostics_error(error)),
    };

    if let Err(error) = std::fs::remove_dir_all(&export_dir) {
        uc_warn!(
            error_kind = "temp_dir_remove",
            io_error_kind = io_error_kind(&error),
            "failed to remove diagnostic export temporary directory"
        );
    }
    result
}

fn create_export_dir(temporary_root: &Path) -> Result<std::path::PathBuf, EngineError> {
    let export_dir = temporary_root.join(format!("diagnostic-export-{}", RepresentationId::new()));
    std::fs::create_dir_all(&export_dir).map_err(|error| {
        uc_warn!(
            error_kind = "temp_dir_create",
            io_error_kind = io_error_kind(&error),
            "failed to create diagnostic export temporary directory"
        );
        internal_error(EXPORT_DIAGNOSTIC_LOGS_FAILED_CODE)
    })?;
    Ok(export_dir)
}

fn map_diagnostics_error(error: DiagnosticsFacadeError) -> EngineError {
    match error {
        DiagnosticsFacadeError::DownloadsUnavailable
        | DiagnosticsFacadeError::LoadSettings(_)
        | DiagnosticsFacadeError::SaveSettings(_)
        | DiagnosticsFacadeError::Export(_) => internal_error(EXPORT_DIAGNOSTIC_LOGS_FAILED_CODE),
    }
}

fn map_copy_error(error: HostFileCopyError) -> EngineError {
    match error {
        HostFileCopyError::LocalRead(_)
        | HostFileCopyError::LocalWrite(_)
        | HostFileCopyError::HostChunkInvalid => internal_error(EXPORT_DIAGNOSTIC_LOGS_FAILED_CODE),
        HostFileCopyError::Host(error) => match error.category() {
            HostCapabilityErrorCategory::InvalidHandle => EngineError::new(
                EXPORT_DIAGNOSTIC_LOGS_INVALID_TARGET_CODE,
                EngineErrorCategory::InvalidInput,
                false,
            ),
            HostCapabilityErrorCategory::PermissionDenied => EngineError::new(
                EXPORT_DIAGNOSTIC_LOGS_UNAUTHORIZED_CODE,
                EngineErrorCategory::Unauthorized,
                false,
            ),
            HostCapabilityErrorCategory::Unavailable => EngineError::new(
                EXPORT_DIAGNOSTIC_LOGS_UNAVAILABLE_CODE,
                EngineErrorCategory::Unavailable,
                true,
            ),
            HostCapabilityErrorCategory::Io => EngineError::new(
                EXPORT_DIAGNOSTIC_LOGS_FAILED_CODE,
                EngineErrorCategory::Internal,
                true,
            ),
        },
    }
}

fn internal_error(code: u32) -> EngineError {
    EngineError::new(code, EngineErrorCategory::Internal, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_export_dir_creation_is_recorded_without_the_path_and_keeps_its_code() {
        let logs = uc_testkit::log_capture::CapturedLogs::default();
        let _guard = logs.install();
        let root = tempfile::tempdir().unwrap();
        let blocker = root.path().join("PRIVATE_BLOCKER");
        std::fs::write(&blocker, b"file").unwrap();

        let error = create_export_dir(&blocker).unwrap_err();

        assert_eq!(error.code(), EXPORT_DIAGNOSTIC_LOGS_FAILED_CODE);
        assert_eq!(
            logs.count("error_kind=\"temp_dir_create\""),
            1,
            "{}",
            logs.output()
        );
        assert!(logs.output().contains("io_error_kind"), "{}", logs.output());
        assert!(!logs.output().contains("PRIVATE"), "{}", logs.output());
    }
}
