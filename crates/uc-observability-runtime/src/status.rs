#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SetupStatus {
    Disabled,
    Ready,
    Unavailable,
}

/// 远端导出器构建失败的阶段；只有固定分类，不携带 endpoint、header 或底层错误正文。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteSetupFailure {
    HttpClient,
    TraceExporter,
    LogExporter,
}

impl RemoteSetupFailure {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::HttpClient => "http_client",
            Self::TraceExporter => "trace_exporter",
            Self::LogExporter => "log_exporter",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObservabilityHealth {
    pub remote: SetupStatus,
    /// `remote` 为 `Unavailable` 时的构建失败阶段；其他状态为 `None`。
    pub remote_setup_failure: Option<RemoteSetupFailure>,
    pub local_file: SetupStatus,
    pub dropped_local_records: u64,
    pub dropped_remote_spans: u64,
    pub dropped_remote_logs: u64,
    pub failed_remote_span_batches: u64,
    pub failed_remote_log_batches: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalResult {
    Completed,
    Failed,
    TimedOut,
    AlreadyShutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlushSummary {
    pub traces: SignalResult,
    pub logs: SignalResult,
}

impl FlushSummary {
    pub(crate) fn failed() -> Self {
        Self {
            traces: SignalResult::Failed,
            logs: SignalResult::Failed,
        }
    }

    pub(crate) fn timed_out() -> Self {
        Self {
            traces: SignalResult::TimedOut,
            logs: SignalResult::TimedOut,
        }
    }

    pub(crate) fn already_shutdown() -> Self {
        Self {
            traces: SignalResult::AlreadyShutdown,
            logs: SignalResult::AlreadyShutdown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShutdownSummary {
    pub traces: SignalResult,
    pub logs: SignalResult,
}

impl ShutdownSummary {
    pub(crate) fn failed() -> Self {
        Self {
            traces: SignalResult::Failed,
            logs: SignalResult::Failed,
        }
    }

    pub(crate) fn timed_out() -> Self {
        Self {
            traces: SignalResult::TimedOut,
            logs: SignalResult::TimedOut,
        }
    }

    pub(crate) fn already_shutdown() -> Self {
        Self {
            traces: SignalResult::AlreadyShutdown,
            logs: SignalResult::AlreadyShutdown,
        }
    }
}
