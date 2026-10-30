//! 宿主进程唯一拥有的运行诊断装配。

#[cfg(target_os = "android")]
mod android;
mod build_source;
mod config;
mod engine_activity;
pub use engine_activity::{
    confirm_engine_closed, update_engine_closer, EngineLogActivity, RetainedEngineCloser,
    RetainedEngineResult, RetainedEngineState, RetainedEngineToken,
};
mod file_statistics;
mod host_diagnostics;
pub use host_diagnostics::{
    HostDiagnosticAction, HostDiagnosticEvent, HostDiagnosticFailure, HostDiagnosticOutcome,
    HostDiagnosticReceipt, HostDiagnosticRecordStatus, HostDiagnosticSource, HostLifecycleState,
    HostNetworkKind,
};
mod filter;
mod local_capture;
mod local_file;
pub use file_statistics::{FileSourceCounts, LocalDiagnosticSource};
mod local_log_processor;
mod local_recording;
mod module_log;
mod module_log_fields;
mod network_log;
mod process_census;
mod process_release;
pub use local_capture::{
    CaptureEndReason, DetailedCaptureRequest, LocalCaptureMode, LocalCaptureStatus,
    LocalDiagnosticError, LocalDiagnosticExportReport, LocalDiagnosticStatus, SourceCapability,
    SourceCollection, SourceCoverage, StopCaptureResult,
};
pub use module_log::ModuleLogCounts;
pub use network_log::{
    NetworkCaptureMode, NetworkCaptureStatus, NetworkLibraryStatus, NetworkLogBudget,
    NetworkLogConfig, NetworkLogCounters, NetworkLogError, NetworkLogExportReport, NetworkLogFile,
    NetworkLogState, NetworkLogStatus, NetworkProcessRole, NETWORK_LOG_SCHEMA_VERSION,
};
pub use process_release::{
    ProcessFailureKind, ProcessReleaseReport, ProcessReleaseResult, ProcessResumeResult,
};
mod remote_health;
mod runtime;
mod status;
mod subscriber;
mod subscriber_gate;
mod telemetry;

#[cfg(target_os = "android")]
pub use android::*;
pub use config::*;
pub use local_file::{managed_log_files, migrate_managed_log_files};
pub use runtime::*;
pub use status::*;
pub use subscriber::HostLogLayer;

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
