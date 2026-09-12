#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SetupStatus {
    Disabled,
    Ready,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObservabilityHealth {
    pub remote: SetupStatus,
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
