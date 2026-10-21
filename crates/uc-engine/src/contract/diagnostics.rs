use std::fmt;

use serde::{Deserialize, Serialize};

use crate::HostFileHandle;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticsStatusSummary {
    /// 拒绝资料仍在原位；宿主必须展示原因和重新导出指引。
    #[serde(default)]
    pub rejected_config_import: Option<RejectedConfigImportSummary>,
    /// 宿主经 `EngineConfig::with_host_log_profile` 声明的本进程实际日志档位；未声明为 `None`。
    pub effective_log_profile: Option<String>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ExportDiagnosticLogsInput {
    pub since_hours: Option<u32>,
    pub destination: HostFileHandle,
}

impl fmt::Debug for ExportDiagnosticLogsInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExportDiagnosticLogsInput")
            .field("since_hours", &self.since_hours)
            .field("destination", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticLogsExportSummary {
    pub included_files: Vec<String>,
    pub since_unix_ms: i64,
}

impl fmt::Debug for DiagnosticLogsExportSummary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DiagnosticLogsExportSummary")
            .field("included_file_count", &self.included_files.len())
            .field("since_unix_ms", &self.since_unix_ms)
            .finish()
    }
}

uc_observability_contract::wire_enum! {
    serde
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    pub enum ConfigImportRejectionReason {
        UnsupportedFormat => "unsupported_format",
        MalformedMarker => "malformed_marker",
        MissingRuntimeMembers => "missing_runtime_members",
    }
}

/// 描述未完成的导入；不包含路径、身份、密钥或归档内容。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RejectedConfigImportSummary {
    pub reason: ConfigImportRejectionReason,
    pub message: String,
}
