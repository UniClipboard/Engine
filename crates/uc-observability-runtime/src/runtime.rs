use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use tracing_subscriber::filter::dynamic_filter_fn;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Layer;

use crate::config::ObservabilityConfig;
use crate::filter::local_sink_enabled;
use crate::local_file::LocalFileRuntime;
use crate::status::{
    FlushSummary, ObservabilityHealth, SetupStatus, ShutdownSummary, SignalResult,
};
use crate::subscriber::{local_health_layer, system_layer};
use crate::telemetry::TelemetryRuntime;
use crate::{
    DetailedCaptureRequest, HostDiagnosticEvent, HostDiagnosticReceipt, HostDiagnosticRecordStatus,
    HostDiagnosticSource, HostLogLayer, LocalCaptureStatus, LocalDiagnosticError,
    LocalDiagnosticExportReport, LocalDiagnosticStatus, SourceCapability, SourceCollection,
    StopCaptureResult,
};

static INSTALL_GUARD: Mutex<()> = Mutex::new(());
static INSTALLED: OnceLock<Arc<RuntimeState>> = OnceLock::new();

pub struct ProcessObservabilityRuntime;

impl ProcessObservabilityRuntime {
    pub fn install(config: ObservabilityConfig) -> Result<InstallOutcome, InstallError> {
        Self::install_with_host_layers(config, Vec::new())
    }

    /// 在同一个进程 subscriber 中保留宿主日志输出。宿主层仅可在首次安装时提供，
    /// 不接收 Engine 自有事件，不能复制核心诊断或绕过其隐私过滤。
    pub fn install_with_host_layers(
        config: ObservabilityConfig,
        host_layers: Vec<HostLogLayer>,
    ) -> Result<InstallOutcome, InstallError> {
        let _install_guard = INSTALL_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(installed) = INSTALLED.get() {
            if installed.config == config && host_layers.is_empty() {
                return Ok(InstallOutcome::Reused(ProcessObservabilityHandle {
                    state: Arc::clone(installed),
                }));
            }
            return Err(InstallError::AlreadyInstalled);
        }

        let (state, subscriber) = build_runtime(config, host_layers);
        tracing::subscriber::set_global_default(subscriber)
            .map_err(|_| InstallError::SubscriberAlreadyInstalled)?;
        let _ = tracing_log::LogTracer::init();
        INSTALLED
            .set(Arc::clone(&state))
            .map_err(|_| InstallError::AlreadyInstalled)?;
        state.telemetry.recording.checkpoint(
            "diagnostics.run.started",
            serde_json::json!({ "capture_mode": "standard" }),
        );
        Ok(InstallOutcome::Installed(ProcessObservabilityHandle {
            state,
        }))
    }

    pub fn flush_local_logs(deadline: Duration) -> SignalResult {
        let Some(state) = INSTALLED.get() else {
            return SignalResult::Completed;
        };
        if state.shutdown.is_started() {
            return SignalResult::AlreadyShutdown;
        }
        if state
            .local_file
            .as_ref()
            .is_none_or(|local_file| local_file.flush(deadline))
        {
            SignalResult::Completed
        } else {
            SignalResult::Failed
        }
    }
}

pub enum InstallOutcome {
    Installed(ProcessObservabilityHandle),
    Reused(ProcessObservabilityHandle),
}

impl InstallOutcome {
    pub fn handle(&self) -> ProcessObservabilityHandle {
        match self {
            Self::Installed(handle) | Self::Reused(handle) => handle.clone(),
        }
    }
}

impl fmt::Debug for InstallOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Installed(_) => formatter.write_str("Installed(REDACTED)"),
            Self::Reused(_) => formatter.write_str("Reused(REDACTED)"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InstallError {
    #[error("observability runtime is already installed with a different configuration")]
    AlreadyInstalled,
    #[error("the process already has a tracing subscriber")]
    SubscriberAlreadyInstalled,
}

#[derive(Clone)]
pub struct ProcessObservabilityHandle {
    state: Arc<RuntimeState>,
}

impl fmt::Debug for ProcessObservabilityHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProcessObservabilityHandle(REDACTED)")
    }
}

impl ProcessObservabilityHandle {
    pub fn register_host_diagnostic_source(
        &self,
        source: HostDiagnosticSource,
        capability: SourceCapability,
    ) -> Result<(), LocalDiagnosticError> {
        if !source.allowed(self.state.config.resource.os)
            || matches!(
                capability,
                SourceCapability::Unknown | SourceCapability::Unsupported
            )
        {
            return Err(LocalDiagnosticError::InvalidHostSource);
        }
        if self.state.shutdown.is_started() {
            return Err(LocalDiagnosticError::AlreadyShutdown);
        }
        self.state.telemetry.recording.register_source(
            source.source(),
            capability,
            SourceCollection::Enabled,
        );
        Ok(())
    }

    pub fn record_host_diagnostic(
        &self,
        source: HostDiagnosticSource,
        event: HostDiagnosticEvent,
    ) -> HostDiagnosticReceipt {
        if self.state.shutdown.is_started() {
            return HostDiagnosticReceipt::status(HostDiagnosticRecordStatus::AlreadyShutdown);
        }
        if self.state.local_file.is_none() {
            return HostDiagnosticReceipt::status(HostDiagnosticRecordStatus::Unavailable);
        }
        self.state.telemetry.recording.record_host(source, event)
    }
    pub fn start_local_diagnostic_capture(
        &self,
        request: DetailedCaptureRequest,
    ) -> Result<LocalCaptureStatus, LocalDiagnosticError> {
        if self.state.shutdown.is_started() {
            return Err(LocalDiagnosticError::AlreadyShutdown);
        }
        if self.state.local_file.is_none() {
            return Err(LocalDiagnosticError::LocalSinkUnavailable);
        }
        self.state.telemetry.recording.start_capture(request)
    }

    pub fn stop_local_diagnostic_capture(
        &self,
        capture_id: &str,
    ) -> Result<StopCaptureResult, LocalDiagnosticError> {
        if self.state.shutdown.is_started() {
            return Err(LocalDiagnosticError::AlreadyShutdown);
        }
        self.state.telemetry.recording.stop_capture(capture_id)
    }

    pub fn query_local_diagnostic_status(&self) -> LocalDiagnosticStatus {
        self.state.telemetry.recording.status(
            self.state.health.local_file,
            self.state.shutdown.is_started(),
        )
    }

    pub fn prepare_local_diagnostic_export(
        &self,
        deadline: Duration,
    ) -> Result<LocalDiagnosticExportReport, LocalDiagnosticError> {
        if deadline < Duration::from_millis(1) || deadline > Duration::from_secs(5) {
            return Err(LocalDiagnosticError::InvalidDeadline);
        }
        let requested_at_utc = chrono::Utc::now().to_rfc3339();
        let before_flush = self.query_local_diagnostic_status();
        if !self.state.shutdown.is_started() {
            self.state.telemetry.recording.checkpoint("diagnostics.export.snapshot", serde_json::json!({
                "capture": before_flush.capture, "observed_records": before_flush.observed_records,
                "policy_filtered_records": before_flush.policy_filtered_records, "schema_rejected_records": before_flush.schema_rejected_records,
                "counter_scope": before_flush.counter_scope,
            }));
            for source in &before_flush.sources {
                self.state.telemetry.recording.checkpoint(
                    "diagnostics.export.source",
                    serde_json::json!({ "coverage": source }),
                );
            }
        }
        let flush = if self.state.shutdown.is_started() {
            SignalResult::AlreadyShutdown
        } else {
            match &self.state.local_file {
                Some(file) => file.flush_result(deadline),
                _ => SignalResult::Failed,
            }
        };
        Ok(LocalDiagnosticExportReport {
            flush,
            status: self.query_local_diagnostic_status(),
            requested_at_utc,
            completed_at_utc: chrono::Utc::now().to_rfc3339(),
            other_processes_flushed: false,
            files: self
                .state
                .local_file
                .as_ref()
                .map_or_else(Vec::new, |file| file.statistics()),
        })
    }
    pub fn health(&self) -> ObservabilityHealth {
        let mut health = self.state.health;
        health.dropped_local_records = self
            .state
            .local_file
            .as_ref()
            .map_or(0, |local_file| local_file.dropped_records());
        let remote = self.state.telemetry.health();
        health.dropped_remote_spans = remote.dropped_spans;
        health.dropped_remote_logs = remote.dropped_logs;
        health.failed_remote_span_batches = remote.failed_span_batches;
        health.failed_remote_log_batches = remote.failed_log_batches;
        health
    }

    pub fn force_flush(&self, deadline: Duration) -> FlushSummary {
        if self.state.shutdown.is_started() {
            return FlushSummary::already_shutdown();
        }
        if !self.state.lifecycle.try_reserve() {
            return FlushSummary::timed_out();
        }
        let telemetry = self.state.telemetry.clone();
        let local_file = self.state.local_file.clone();
        match run_reserved_with_deadline(deadline, Arc::clone(&self.state.lifecycle), move || {
            let signals = telemetry.force_flush();
            FlushSummary {
                traces: signals.traces,
                logs: combine_local_result(
                    signals.logs,
                    local_file.is_none_or(|local_file| local_file.flush(deadline)),
                ),
            }
        }) {
            DeadlineOutcome::Completed(summary) => summary,
            DeadlineOutcome::TimedOut => FlushSummary::timed_out(),
            DeadlineOutcome::Failed => FlushSummary::failed(),
        }
    }

    pub fn shutdown(&self, deadline: Duration) -> ShutdownSummary {
        match self.state.shutdown.begin() {
            ShutdownStart::Completed(summary) => return summary,
            ShutdownStart::Waiting => {
                return self
                    .state
                    .shutdown
                    .wait(deadline)
                    .unwrap_or_else(ShutdownSummary::timed_out);
            }
            ShutdownStart::Started => {}
        }
        self.state.health_accepting.store(true, Ordering::Release);
        self.state.telemetry.recording.finish_run();
        self.state.telemetry.seal();
        let telemetry = self.state.telemetry.clone();
        let local_file = self.state.local_file.clone();
        let health_accepting = Arc::clone(&self.state.health_accepting);
        spawn_shutdown(
            Arc::clone(&self.state.shutdown),
            Arc::clone(&self.state.lifecycle),
            move || {
                let _health_guard = HealthAcceptanceGuard(health_accepting);
                let signals = telemetry.shutdown(deadline);
                let local_completed = local_file
                    .as_ref()
                    .is_none_or(|local_file| local_file.shutdown(deadline));
                let summary = ShutdownSummary {
                    traces: signals.traces,
                    logs: combine_local_result(signals.logs, local_completed),
                };
                let retryable = local_file
                    .as_ref()
                    .is_some_and(|local_file| local_file.shutdown_cleanup_incomplete());
                let terminal_failures = TerminalShutdownFailures {
                    traces: terminal_signal_failure(signals.traces),
                    logs: if !local_completed && !retryable {
                        Some(SignalResult::Failed)
                    } else {
                        terminal_signal_failure(signals.logs)
                    },
                };
                ShutdownAttempt {
                    summary,
                    retryable,
                    terminal_failures,
                }
            },
        );
        self.state
            .shutdown
            .wait(deadline)
            .unwrap_or_else(ShutdownSummary::timed_out)
    }
}

struct RuntimeState {
    config: ObservabilityConfig,
    telemetry: TelemetryRuntime,
    health: ObservabilityHealth,
    local_file: Option<Arc<LocalFileRuntime>>,
    shutdown: Arc<ShutdownCoordinator>,
    lifecycle: Arc<LifecycleGate>,
    health_accepting: Arc<AtomicBool>,
}

fn build_runtime(
    config: ObservabilityConfig,
    host_layers: Vec<HostLogLayer>,
) -> (Arc<RuntimeState>, impl tracing::Subscriber + Send + Sync) {
    let health_accepting = Arc::new(AtomicBool::new(true));
    let mut layers = Vec::new();
    let (local_file_status, local_file) = match config.local_logs.as_ref() {
        Some(local) => match local_health_layer(&local.directory, Arc::clone(&health_accepting)) {
            Ok((layer, local_file)) => {
                layers.push(layer);
                (SetupStatus::Ready, Some(local_file))
            }
            Err(()) => (SetupStatus::Unavailable, None),
        },
        None => (SetupStatus::Disabled, None),
    };

    let (telemetry, remote) = TelemetryRuntime::new(&config, local_file.clone());
    let telemetry_accepting = telemetry.accepting();
    layers.extend(telemetry.layers());
    layers.push(system_layer());
    let global_telemetry_accepting = Arc::clone(&telemetry_accepting);
    let global_health_accepting = Arc::clone(&health_accepting);
    let engine_layer = layers.with_filter(dynamic_filter_fn(move |metadata, _| {
        let accepting =
            if metadata.target() == uc_observability_contract::diagnostics::HEALTH_TARGET {
                &global_health_accepting
            } else {
                &global_telemetry_accepting
            };
        accepting.load(Ordering::Acquire) && local_sink_enabled(metadata)
    }));
    let mut all_layers: Vec<HostLogLayer> = vec![Box::new(engine_layer)];
    if !host_layers.is_empty() {
        all_layers.push(Box::new(host_layers.with_filter(
            tracing_subscriber::filter::filter_fn(host_metadata_enabled),
        )));
    }
    let subscriber = tracing_subscriber::registry().with(all_layers);
    let state = Arc::new(RuntimeState {
        config,
        telemetry,
        health: ObservabilityHealth {
            remote,
            local_file: local_file_status,
            dropped_local_records: 0,
            dropped_remote_spans: 0,
            dropped_remote_logs: 0,
            failed_remote_span_batches: 0,
            failed_remote_log_batches: 0,
        },
        local_file,
        shutdown: Arc::new(ShutdownCoordinator::default()),
        lifecycle: Arc::new(LifecycleGate::default()),
        health_accepting,
    });
    (state, subscriber)
}

fn host_metadata_enabled(metadata: &tracing::Metadata<'_>) -> bool {
    if matches!(
        metadata.target(),
        "uc.telemetry" | "uc.connectivity" | "observability.health"
    ) {
        return false;
    }
    let engine_source = |name: &str| {
        [
            "uc_core",
            "uc_application",
            "uc_infra",
            "uc_engine",
            "uc_observability_contract",
            "uc_observability_runtime",
            "uc_mobile",
            "uc_mobile_lan",
            "uc_mobile_proto",
        ]
        .iter()
        .any(|prefix| {
            name.strip_prefix(prefix)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with("::"))
        })
    };
    // 网络依赖的原始地址、标识和错误正文也不能绕行到宿主输出。
    let network_source = |name: &str| {
        [
            "iroh",
            "noq",
            "netwatch",
            "swarm_discovery",
            "hickory",
            "pkarr",
            "quinn",
            "portmapper",
            "netdev",
        ]
        .iter()
        .any(|prefix| {
            name.strip_prefix(prefix).is_some_and(|rest| {
                rest.is_empty()
                    || rest.starts_with("::")
                    || rest.starts_with('_')
                    || rest.starts_with('.')
            })
        })
    };
    !engine_source(metadata.target())
        && !metadata.module_path().is_some_and(engine_source)
        && !network_source(metadata.target())
        && !metadata.module_path().is_some_and(network_source)
}

struct HealthAcceptanceGuard(Arc<AtomicBool>);

impl Drop for HealthAcceptanceGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

fn combine_local_result(remote: SignalResult, local_completed: bool) -> SignalResult {
    if !local_completed {
        SignalResult::Failed
    } else {
        remote
    }
}

#[derive(Default)]
struct LifecycleGate {
    active: Mutex<bool>,
    idle: Condvar,
}

impl LifecycleGate {
    fn try_reserve(&self) -> bool {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *active {
            return false;
        }
        *active = true;
        true
    }

    fn reserve_when_idle(&self) {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while *active {
            active = self
                .idle
                .wait(active)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        *active = true;
    }

    fn release(&self) {
        *self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = false;
        self.idle.notify_all();
    }
}

fn run_reserved_with_deadline<T: Send + 'static>(
    deadline: Duration,
    gate: Arc<LifecycleGate>,
    operation: impl FnOnce() -> T + Send + 'static,
) -> DeadlineOutcome<T> {
    let (sender, receiver) = mpsc::sync_channel(1);
    let worker_gate = Arc::clone(&gate);
    if std::thread::Builder::new()
        .name("uc-observability-lifecycle".to_owned())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation));
            worker_gate.release();
            let _ = sender.send(result.map_err(|_| ()));
        })
        .is_err()
    {
        gate.release();
        return DeadlineOutcome::Failed;
    }
    match receiver.recv_timeout(deadline) {
        Ok(Ok(result)) => DeadlineOutcome::Completed(result),
        Ok(Err(())) | Err(mpsc::RecvTimeoutError::Disconnected) => DeadlineOutcome::Failed,
        Err(mpsc::RecvTimeoutError::Timeout) => DeadlineOutcome::TimedOut,
    }
}

enum DeadlineOutcome<T> {
    Completed(T),
    TimedOut,
    Failed,
}

#[derive(Clone, Copy)]
enum ShutdownPhase {
    Running,
    InProgress {
        terminal_failures: TerminalShutdownFailures,
    },
    Completed {
        summary: ShutdownSummary,
        retryable: bool,
        terminal_failures: TerminalShutdownFailures,
    },
}

struct ShutdownCoordinator {
    phase: Mutex<ShutdownPhase>,
    changed: Condvar,
}

impl Default for ShutdownCoordinator {
    fn default() -> Self {
        Self {
            phase: Mutex::new(ShutdownPhase::Running),
            changed: Condvar::new(),
        }
    }
}

enum ShutdownStart {
    Started,
    Waiting,
    Completed(ShutdownSummary),
}

impl ShutdownCoordinator {
    fn begin(&self) -> ShutdownStart {
        let mut phase = self
            .phase
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match *phase {
            ShutdownPhase::Running => {
                *phase = ShutdownPhase::InProgress {
                    terminal_failures: TerminalShutdownFailures::default(),
                };
                ShutdownStart::Started
            }
            ShutdownPhase::InProgress { .. } => ShutdownStart::Waiting,
            ShutdownPhase::Completed {
                retryable: true,
                terminal_failures,
                ..
            } => {
                *phase = ShutdownPhase::InProgress { terminal_failures };
                ShutdownStart::Started
            }
            ShutdownPhase::Completed { summary, .. }
                if shutdown_completed_successfully(summary) =>
            {
                ShutdownStart::Completed(ShutdownSummary::already_shutdown())
            }
            ShutdownPhase::Completed { summary, .. } => ShutdownStart::Completed(summary),
        }
    }

    fn is_started(&self) -> bool {
        !matches!(
            *self
                .phase
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            ShutdownPhase::Running
        )
    }

    fn complete(&self, attempt: ShutdownAttempt) {
        let mut phase = self
            .phase
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous_failures = match *phase {
            ShutdownPhase::InProgress { terminal_failures } => terminal_failures,
            ShutdownPhase::Running | ShutdownPhase::Completed { .. } => {
                TerminalShutdownFailures::default()
            }
        };
        let terminal_failures = previous_failures.merge(attempt.terminal_failures);
        *phase = ShutdownPhase::Completed {
            summary: terminal_failures.apply(attempt.summary),
            retryable: attempt.retryable,
            terminal_failures,
        };
        self.changed.notify_all();
    }

    fn wait(&self, deadline: Duration) -> Option<ShutdownSummary> {
        let started = Instant::now();
        let mut phase = self
            .phase
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if let ShutdownPhase::Completed { summary, .. } = *phase {
                return Some(summary);
            }
            let remaining = deadline.checked_sub(started.elapsed())?;
            let (next, timeout) = self
                .changed
                .wait_timeout(phase, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            phase = next;
            if timeout.timed_out() && !matches!(*phase, ShutdownPhase::Completed { .. }) {
                return None;
            }
        }
    }
}

fn shutdown_completed_successfully(summary: ShutdownSummary) -> bool {
    [summary.traces, summary.logs].into_iter().all(|result| {
        matches!(
            result,
            SignalResult::Completed | SignalResult::AlreadyShutdown
        )
    })
}

#[derive(Clone, Copy, Default)]
struct TerminalShutdownFailures {
    traces: Option<SignalResult>,
    logs: Option<SignalResult>,
}

impl TerminalShutdownFailures {
    fn merge(self, current: Self) -> Self {
        Self {
            traces: self.traces.or(current.traces),
            logs: self.logs.or(current.logs),
        }
    }

    fn apply(self, mut summary: ShutdownSummary) -> ShutdownSummary {
        if let Some(traces) = self.traces {
            summary.traces = traces;
        }
        if let Some(logs) = self.logs {
            summary.logs = logs;
        }
        summary
    }
}

fn terminal_signal_failure(result: SignalResult) -> Option<SignalResult> {
    match result {
        SignalResult::Failed | SignalResult::TimedOut => Some(result),
        SignalResult::Completed | SignalResult::AlreadyShutdown => None,
    }
}

struct ShutdownAttempt {
    summary: ShutdownSummary,
    retryable: bool,
    terminal_failures: TerminalShutdownFailures,
}

fn spawn_shutdown(
    shutdown: Arc<ShutdownCoordinator>,
    gate: Arc<LifecycleGate>,
    operation: impl FnOnce() -> ShutdownAttempt + Send + 'static,
) {
    let worker_gate = Arc::clone(&gate);
    let worker_shutdown = Arc::clone(&shutdown);
    if std::thread::Builder::new()
        .name("uc-observability-shutdown".to_owned())
        .spawn(move || {
            worker_gate.reserve_when_idle();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation))
                .unwrap_or_else(|_| ShutdownAttempt {
                    summary: ShutdownSummary::failed(),
                    retryable: true,
                    terminal_failures: TerminalShutdownFailures {
                        traces: Some(SignalResult::Failed),
                        logs: Some(SignalResult::Failed),
                    },
                });
            worker_gate.release();
            worker_shutdown.complete(result);
        })
        .is_err()
    {
        shutdown.complete(ShutdownAttempt {
            summary: ShutdownSummary::failed(),
            retryable: true,
            terminal_failures: TerminalShutdownFailures::default(),
        });
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Barrier;

    use super::*;
    use crate::{
        CaptureEndReason, DeploymentEnvironment, LocalCaptureMode, LocalLogConfig,
        ObservabilityResource, OperatingSystem,
    };

    #[test]
    fn shutdown_rejects_capture_that_passed_the_outer_check_before_closing() {
        for initially_active in [false, true] {
            let directory = tempfile::tempdir().expect("日志目录");
            let config = ObservabilityConfig::new(
                ObservabilityResource::new(
                    "1.1.0",
                    DeploymentEnvironment::Test,
                    OperatingSystem::Macos,
                    "test",
                )
                .expect("资源配置"),
            )
            .with_local_logs(LocalLogConfig::new(directory.path()));
            let (state, _subscriber) = build_runtime(config, Vec::new());
            let handle = ProcessObservabilityHandle { state };
            if initially_active {
                handle
                    .start_local_diagnostic_capture(DetailedCaptureRequest::default())
                    .expect("初始采集");
            }
            let (checked, observed_check) = mpsc::channel();
            let (resume, resumed) = mpsc::channel();
            let worker_handle = handle.clone();
            let worker = std::thread::spawn(move || {
                // 固定请求已通过外层检查、但尚未进入采集负责人的并发顺序。
                assert!(!worker_handle.state.shutdown.is_started());
                checked.send(()).expect("检查完成");
                resumed.recv().expect("继续请求");
                worker_handle
                    .state
                    .telemetry
                    .recording
                    .start_capture(DetailedCaptureRequest::default())
            });
            observed_check.recv().expect("请求已通过检查");
            let shutdown = handle.shutdown(Duration::from_secs(2));
            resume.send(()).expect("关闭后继续请求");
            let result = worker.join().expect("采集线程");
            assert_eq!(shutdown.logs, SignalResult::Completed);
            assert!(matches!(result, Err(LocalDiagnosticError::AlreadyShutdown)));
            let status = handle.query_local_diagnostic_status();
            assert!(status.closed);
            assert_eq!(status.capture.mode, LocalCaptureMode::Standard);
            assert!(status.capture.capture_id.is_none());
            assert_eq!(status.capture.remaining_ms, 0);
            if initially_active {
                assert_eq!(
                    status.capture.end_reason,
                    Some(CaptureEndReason::RuntimeShutdown)
                );
            }
        }
    }

    #[test]
    fn timed_out_lifecycle_work_keeps_later_flushes_out_until_it_finishes() {
        let gate = Arc::new(LifecycleGate::default());
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));

        assert!(gate.try_reserve());
        let worker_gate = Arc::clone(&gate);
        let worker_entered = Arc::clone(&entered);
        let worker_release = Arc::clone(&release);
        let worker_active = Arc::clone(&active);
        let worker_max = Arc::clone(&max_active);
        let timed_out =
            run_reserved_with_deadline(Duration::from_millis(1), worker_gate, move || {
                let now = worker_active.fetch_add(1, Ordering::AcqRel) + 1;
                worker_max.fetch_max(now, Ordering::AcqRel);
                worker_entered.wait();
                worker_release.wait();
                worker_active.fetch_sub(1, Ordering::AcqRel);
            });
        assert!(matches!(timed_out, DeadlineOutcome::TimedOut));
        entered.wait();
        assert!(!gate.try_reserve());
        release.wait();

        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while !gate.try_reserve() && std::time::Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert_eq!(max_active.load(Ordering::Acquire), 1);
        gate.release();
    }

    #[test]
    fn shutdown_waits_for_a_timed_out_flush_without_overlapping_it() {
        let gate = Arc::new(LifecycleGate::default());
        let flush_entered = Arc::new(Barrier::new(2));
        let release_flush = Arc::new(Barrier::new(2));
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let shutdown_completed = Arc::new(AtomicBool::new(false));

        assert!(gate.try_reserve());
        let flush_gate = Arc::clone(&gate);
        let flush_active = Arc::clone(&active);
        let flush_max = Arc::clone(&max_active);
        let worker_entered = Arc::clone(&flush_entered);
        let worker_release = Arc::clone(&release_flush);
        assert!(matches!(
            run_reserved_with_deadline(Duration::from_millis(1), flush_gate, move || {
                let now = flush_active.fetch_add(1, Ordering::AcqRel) + 1;
                flush_max.fetch_max(now, Ordering::AcqRel);
                worker_entered.wait();
                worker_release.wait();
                flush_active.fetch_sub(1, Ordering::AcqRel);
            },),
            DeadlineOutcome::TimedOut
        ));
        flush_entered.wait();

        let shutdown_active = Arc::clone(&active);
        let shutdown_max = Arc::clone(&max_active);
        let completed = Arc::clone(&shutdown_completed);
        let shutdown = Arc::new(ShutdownCoordinator::default());
        assert!(matches!(shutdown.begin(), ShutdownStart::Started));
        spawn_shutdown(Arc::clone(&shutdown), Arc::clone(&gate), move || {
            let now = shutdown_active.fetch_add(1, Ordering::AcqRel) + 1;
            shutdown_max.fetch_max(now, Ordering::AcqRel);
            shutdown_active.fetch_sub(1, Ordering::AcqRel);
            completed.store(true, Ordering::Release);
            ShutdownAttempt {
                summary: ShutdownSummary {
                    traces: SignalResult::Completed,
                    logs: SignalResult::Completed,
                },
                retryable: false,
                terminal_failures: TerminalShutdownFailures::default(),
            }
        });
        assert!(shutdown.wait(Duration::from_millis(1)).is_none());
        assert!(matches!(shutdown.begin(), ShutdownStart::Waiting));
        assert!(!shutdown_completed.load(Ordering::Acquire));

        release_flush.wait();
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        let summary = shutdown
            .wait(deadline.saturating_duration_since(std::time::Instant::now()))
            .expect("shutdown eventually completes");
        assert!(shutdown_completed.load(Ordering::Acquire));
        assert_eq!(max_active.load(Ordering::Acquire), 1);
        assert_eq!(summary.traces, SignalResult::Completed);
        assert!(matches!(
            shutdown.begin(),
            ShutdownStart::Completed(ShutdownSummary {
                traces: SignalResult::AlreadyShutdown,
                logs: SignalResult::AlreadyShutdown,
            })
        ));
    }

    #[test]
    fn panicked_lifecycle_work_is_failed_instead_of_reported_as_a_timeout() {
        let gate = Arc::new(LifecycleGate::default());
        assert!(gate.try_reserve());

        let outcome = run_reserved_with_deadline(Duration::from_secs(1), gate, || {
            panic!("test lifecycle panic")
        });

        assert!(matches!(outcome, DeadlineOutcome::Failed));
    }

    #[test]
    fn completed_failed_or_timed_out_shutdown_remains_visible() {
        for summary in [ShutdownSummary::failed(), ShutdownSummary::timed_out()] {
            let shutdown = ShutdownCoordinator::default();
            assert!(matches!(shutdown.begin(), ShutdownStart::Started));
            shutdown.complete(ShutdownAttempt {
                summary,
                retryable: false,
                terminal_failures: TerminalShutdownFailures {
                    traces: terminal_signal_failure(summary.traces),
                    logs: terminal_signal_failure(summary.logs),
                },
            });
            assert!(matches!(
                shutdown.begin(),
                ShutdownStart::Completed(stored) if stored == summary
            ));
        }
    }

    #[test]
    fn incomplete_local_cleanup_can_be_retried_without_inventing_a_terminal_failure() {
        let shutdown = ShutdownCoordinator::default();
        assert!(matches!(shutdown.begin(), ShutdownStart::Started));
        shutdown.complete(ShutdownAttempt {
            summary: ShutdownSummary::failed(),
            retryable: true,
            terminal_failures: TerminalShutdownFailures::default(),
        });
        assert!(matches!(shutdown.begin(), ShutdownStart::Started));
        shutdown.complete(ShutdownAttempt {
            summary: ShutdownSummary {
                traces: SignalResult::Completed,
                logs: SignalResult::Completed,
            },
            retryable: false,
            terminal_failures: TerminalShutdownFailures::default(),
        });
        assert!(matches!(
            shutdown.begin(),
            ShutdownStart::Completed(ShutdownSummary {
                traces: SignalResult::AlreadyShutdown,
                logs: SignalResult::AlreadyShutdown,
            })
        ));
    }

    #[test]
    fn remote_failure_remains_visible_while_incomplete_local_cleanup_retries() {
        let shutdown = ShutdownCoordinator::default();
        assert!(matches!(shutdown.begin(), ShutdownStart::Started));
        shutdown.complete(ShutdownAttempt {
            summary: ShutdownSummary {
                traces: SignalResult::Failed,
                logs: SignalResult::Failed,
            },
            retryable: true,
            terminal_failures: TerminalShutdownFailures {
                traces: Some(SignalResult::Failed),
                logs: Some(SignalResult::TimedOut),
            },
        });
        assert!(matches!(shutdown.begin(), ShutdownStart::Started));
        shutdown.complete(ShutdownAttempt {
            summary: ShutdownSummary {
                traces: SignalResult::AlreadyShutdown,
                logs: SignalResult::Completed,
            },
            retryable: false,
            terminal_failures: TerminalShutdownFailures::default(),
        });

        let summary = shutdown
            .wait(Duration::ZERO)
            .expect("stored shutdown result");
        assert_eq!(summary.traces, SignalResult::Failed);
        assert_eq!(summary.logs, SignalResult::TimedOut);
        assert!(matches!(
            shutdown.begin(),
            ShutdownStart::Completed(stored) if stored == summary
        ));
    }

    #[test]
    fn successful_shutdown_is_reported_as_already_shutdown_when_repeated() {
        let shutdown = ShutdownCoordinator::default();
        assert!(matches!(shutdown.begin(), ShutdownStart::Started));
        shutdown.complete(ShutdownAttempt {
            summary: ShutdownSummary {
                traces: SignalResult::Completed,
                logs: SignalResult::Completed,
            },
            retryable: false,
            terminal_failures: TerminalShutdownFailures::default(),
        });
        assert!(matches!(
            shutdown.begin(),
            ShutdownStart::Completed(ShutdownSummary {
                traces: SignalResult::AlreadyShutdown,
                logs: SignalResult::AlreadyShutdown,
            })
        ));
    }

    #[test]
    fn mixed_terminal_shutdown_result_remains_visible() {
        let shutdown = ShutdownCoordinator::default();
        let summary = ShutdownSummary {
            traces: SignalResult::Completed,
            logs: SignalResult::Failed,
        };
        assert!(matches!(shutdown.begin(), ShutdownStart::Started));
        shutdown.complete(ShutdownAttempt {
            summary,
            retryable: false,
            terminal_failures: TerminalShutdownFailures {
                traces: terminal_signal_failure(summary.traces),
                logs: terminal_signal_failure(summary.logs),
            },
        });
        assert!(matches!(
            shutdown.begin(),
            ShutdownStart::Completed(stored) if stored == summary
        ));
    }

    #[test]
    fn panicked_shutdown_worker_reports_failure_and_allows_cleanup_retry() {
        let shutdown = Arc::new(ShutdownCoordinator::default());
        let gate = Arc::new(LifecycleGate::default());
        assert!(matches!(shutdown.begin(), ShutdownStart::Started));
        spawn_shutdown(Arc::clone(&shutdown), gate, || {
            panic!("test shutdown panic")
        });

        let summary = shutdown
            .wait(Duration::from_secs(1))
            .expect("failed shutdown result");
        assert_eq!(summary, ShutdownSummary::failed());
        assert!(matches!(shutdown.begin(), ShutdownStart::Started));
        shutdown.complete(ShutdownAttempt {
            summary: ShutdownSummary {
                traces: SignalResult::Completed,
                logs: SignalResult::Completed,
            },
            retryable: false,
            terminal_failures: TerminalShutdownFailures::default(),
        });
        assert_eq!(
            shutdown.wait(Duration::ZERO),
            Some(ShutdownSummary::failed())
        );
    }
}
