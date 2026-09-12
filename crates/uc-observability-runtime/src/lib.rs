//! 宿主进程唯一拥有的运行诊断装配。

#[cfg(target_os = "android")]
mod android;
mod config;
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
pub use local_capture::{
    CaptureEndReason, DetailedCaptureRequest, LocalCaptureMode, LocalCaptureStatus,
    LocalDiagnosticError, LocalDiagnosticExportReport, LocalDiagnosticStatus, SourceCapability,
    SourceCollection, SourceCoverage, StopCaptureResult,
};
mod remote_health;
mod runtime;
mod status;
mod subscriber;
mod telemetry;

#[cfg(target_os = "android")]
pub use android::*;
pub use config::*;
pub use local_file::managed_log_files;
pub use runtime::*;
pub use status::*;
pub use subscriber::HostLogLayer;

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
