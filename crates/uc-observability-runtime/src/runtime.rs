use std::fmt;
#[cfg(feature = "test-process-release")]
use std::sync::atomic::AtomicU64;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex, MutexGuard, OnceLock, TryLockError};
use std::time::{Duration, Instant};

use tracing_subscriber::filter::dynamic_filter_fn;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Layer;
use uc_observability_contract::diagnostics::connectivity::CONNECTIVITY_TARGET;
use uc_observability_contract::diagnostics::{
    HEALTH_TARGET, LOCAL_DIAGNOSTIC_TARGET, NETWORK_SCOPE_TARGET, TELEMETRY_TARGET,
};

use crate::config::ObservabilityConfig;
#[cfg(feature = "test-process-release")]
use crate::engine_activity::{fenced_registration_attempts, retained_engine_count};
use crate::engine_activity::{
    has_retryable_engine_closer, has_unconfirmed_engine, insert_engine_closer, resume_engines,
    retained_engine_errors, retry_engine_closers, wait_fenced_idle, RetainedEngineCloser,
    RetainedEngineState, RetainedEngineToken,
};
use crate::filter::local_sink_enabled;
use crate::local_file::LocalFileRuntime;
use crate::module_log::{is_engine_source, module_metadata, ModuleLogLayer, ModuleLogStats};
#[cfg(feature = "test-process-release")]
use crate::network_log::PROBE_NETWORK_PARK_FAILURE;
use crate::network_log::{
    NetworkCaptureStatus, NetworkLogError, NetworkLogExportReport, NetworkLogRuntime,
    NetworkLogStatus,
};
#[cfg(feature = "test-process-release")]
use crate::network_log::{PROBE_NETWORK_PARK_BLOCK, PROBE_NETWORK_PARK_ENTERED};
use crate::process_census::ProcessCensus;
use crate::process_release::{
    ProcessFailureKind, ProcessReleaseCoordinator, ProcessReleaseErrors, ProcessReleaseReport,
    ProcessReleaseResult, ProcessResumeResult, ReleaseObservation, ReleaseStart, ResumeObservation,
    ResumeStart,
};
use crate::remote_health::record_remote_setup_degraded;
use crate::status::{
    FlushSummary, ObservabilityHealth, SetupStatus, ShutdownSummary, SignalResult,
};
use crate::subscriber::{local_health_layer, system_layer, HealthRecording};
use crate::telemetry::TelemetryRuntime;
use crate::{
    DetailedCaptureRequest, HostDiagnosticEvent, HostDiagnosticReceipt, HostDiagnosticRecordStatus,
    HostDiagnosticSource, HostLogLayer, LocalCaptureStatus, LocalDiagnosticError,
    LocalDiagnosticExportReport, LocalDiagnosticStatus, SourceCapability, SourceCollection,
    StopCaptureResult,
};
use crate::{OfflineDiagnosticSnapshot, OfflineHistoryStorage};

static INSTALL_GUARD: Mutex<()> = Mutex::new(());
static INSTALLED: OnceLock<Arc<RuntimeState>> = OnceLock::new();
#[cfg(feature = "test-process-release")]
pub(crate) static PROBE_RETRY_GATE_ARM: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "test-process-release")]
pub(crate) static PROBE_RETRY_GATE_ENTERED: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "test-process-release")]
pub(crate) static PROBE_RETRY_GATE_RELEASE: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "test-process-release")]
pub(crate) static PROBE_CENSUS_RESUME_BLOCK: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "test-process-release")]
pub(crate) static PROBE_CENSUS_RESUME_ENTERED: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "test-process-release")]
pub(crate) static PROBE_RELEASE_SPAWN_FAILURE: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "test-process-release")]
static PROBE_RETRY_JOIN_COUNT: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "test-process-release")]
pub(crate) static PROBE_QUEUED_RESUME_RETRY_COUNT: AtomicU64 = AtomicU64::new(0);

pub struct ProcessObservabilityRuntime;

/// 调用期限内未获得准入，因此没有接受新的操作。
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ProcessAdmissionError {
    #[error("process action admission deadline exceeded")]
    DeadlineExceeded,
}

/// 已接受的进程释放；重复等待始终观察同一代次。
pub struct ProcessReleaseObservation {
    handle: ProcessObservabilityHandle,
    observation: Arc<ReleaseObservation>,
}

impl ProcessReleaseObservation {
    pub fn wait(&self, deadline: Duration) -> ProcessReleaseReport {
        let result = self.observation.wait(deadline);
        self.handle
            .safe_release_report_internal(result, &self.observation)
    }
}

/// 已接受的恢复；重复等待始终观察同一代次。
pub struct ProcessResumeObservation {
    observation: Arc<ResumeObservation>,
}

impl ProcessResumeObservation {
    pub fn wait(&self, deadline: Duration) -> ProcessResumeResult {
        self.observation.wait(deadline)
    }
}

pub fn installed_process_observability() -> Option<ProcessObservabilityHandle> {
    INSTALLED.get().map(|state| ProcessObservabilityHandle {
        state: Arc::clone(state),
    })
}

/// 已安装运行期的网络日志；未安装或未配置时为空。
pub(crate) fn installed_network_log() -> Option<Arc<NetworkLogRuntime>> {
    INSTALLED.get().and_then(|state| state.network.clone())
}

/// 仅对同一封口的不可重试失败拒绝登记，保留封口及其资源责任。
pub(crate) fn installed_process_failure_blocks_registration(fence: &Arc<()>) -> bool {
    let Some(state) = INSTALLED.get() else {
        return false;
    };
    let _terminal = match state.terminal_gate.try_lock() {
        Ok(guard) => guard,
        Err(TryLockError::Poisoned(error)) => error.into_inner(),
        Err(TryLockError::WouldBlock) => return false,
    };
    let closers_live = !state.shutdown.is_started()
        && state
            .local_file
            .as_ref()
            .is_none_or(|file| file.queue.has_live_closer())
        && state
            .network
            .as_ref()
            .is_none_or(|network| network.has_live_closer());
    state
        .release
        .failed_without_retry(fence, has_retryable_engine_closer(), closers_live)
}

pub(crate) fn notify_engine_closer_changed() {
    if let Some(state) = INSTALLED.get() {
        state.telemetry.notify_span_wait();
    }
}

/// 在 Engine 宿主句柄消失前接管完整关闭能力。
pub fn retain_engine_closer(
    closer: Arc<dyn RetainedEngineCloser>,
    status: RetainedEngineState,
) -> RetainedEngineToken {
    if let Some(state) = INSTALLED.get() {
        let _terminal = state
            .terminal_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let pending = status == RetainedEngineState::Pending;
        let start = state.release.invalidate_for_engine_retention(pending);
        let token = insert_engine_closer(closer, status);
        if let Some((generation, fence)) = start {
            ProcessObservabilityHandle {
                state: Arc::clone(state),
            }
            .start_process_release(generation, fence, false, false);
        }
        token
    } else {
        insert_engine_closer(closer, status)
    }
}

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
            // discarded-source[no-information]: the error value carries no usable diagnostic information
            .map_err(|_| InstallError::SubscriberAlreadyInstalled)?;
        let _ = tracing_log::LogTracer::init();
        INSTALLED
            .set(Arc::clone(&state))
            // discarded-source[no-information]: the error value carries no usable diagnostic information
            .map_err(|_| InstallError::AlreadyInstalled)?;
        if let Some(failure) = state.health.remote_setup_failure {
            record_remote_setup_degraded(failure);
        }
        state.telemetry.recording.checkpoint(
            "diagnostics.run.started",
            serde_json::json!({ "capture_mode": "standard" }),
        );
        // 周期资源记录只写进本地模块日志；没有本地日志目录时没有读者，不启动线程。
        if state.local_file.is_some() {
            if let Some(census) = ProcessCensus::start() {
                *state
                    .census
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(census);
                state.census_expected.store(true, Ordering::Release);
            }
        }
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
    /// 接受或加入进程释放。观察对象为空时，没有可保留的已接受释放。
    pub fn release_process(
        &self,
        deadline: Duration,
    ) -> Result<(ProcessReleaseReport, Option<ProcessReleaseObservation>), ProcessAdmissionError>
    {
        let (result, observation) = self.release_process_observed_internal(deadline);
        if observation.is_none() && matches!(result, ProcessReleaseResult::Pending { .. }) {
            return Err(ProcessAdmissionError::DeadlineExceeded);
        }
        Ok(self.release_result_with_observation(result, observation))
    }

    /// 显式重试仍由活着的负责人持有的失败关闭。
    pub fn retry_process_release(
        &self,
        deadline: Duration,
    ) -> Result<(ProcessReleaseReport, Option<ProcessReleaseObservation>), ProcessAdmissionError>
    {
        let (result, observation) = self.retry_process_release_observed_internal(deadline);
        if observation.is_none() && matches!(result, ProcessReleaseResult::Pending { .. }) {
            return Err(ProcessAdmissionError::DeadlineExceeded);
        }
        Ok(self.release_result_with_observation(result, observation))
    }

    fn release_result_with_observation(
        &self,
        result: ProcessReleaseResult,
        observation: Option<Arc<ReleaseObservation>>,
    ) -> (ProcessReleaseReport, Option<ProcessReleaseObservation>) {
        match observation {
            Some(observation) => {
                let report = self.safe_release_report_internal(result, &observation);
                (
                    report,
                    Some(ProcessReleaseObservation {
                        handle: self.clone(),
                        observation,
                    }),
                )
            }
            None => {
                let cleanup_errors = if matches!(result, ProcessReleaseResult::Failed { .. }) {
                    vec![ProcessFailureKind::Unconfirmed]
                } else {
                    Vec::new()
                };
                let operation_errors = if matches!(
                    result,
                    ProcessReleaseResult::Released {
                        operation_failed: true,
                        ..
                    }
                ) {
                    vec![ProcessFailureKind::OtherOperation]
                } else {
                    Vec::new()
                };
                (
                    ProcessReleaseReport {
                        result,
                        cleanup_errors,
                        operation_errors,
                        explicit_retry_supported: false,
                    },
                    None,
                )
            }
        }
    }

    /// 接受或加入恢复；观察对象为空时，没有可保留的已接受恢复。
    pub fn resume_process(
        &self,
        deadline: Duration,
    ) -> Result<(ProcessResumeResult, Option<ProcessResumeObservation>), ProcessAdmissionError>
    {
        let (result, observation) = self.resume_process_observed_internal(deadline);
        if observation.is_none() && matches!(result, ProcessResumeResult::Pending { .. }) {
            return Err(ProcessAdmissionError::DeadlineExceeded);
        }
        Ok((
            result,
            observation.map(|observation| ProcessResumeObservation { observation }),
        ))
    }
    #[cfg(feature = "test-process-release")]
    #[doc(hidden)]
    pub fn probe_hold_terminal_gate(&self, entered: mpsc::Sender<()>, release: mpsc::Receiver<()>) {
        let _guard = self
            .state
            .terminal_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = entered.send(());
        let _ = release.recv();
    }

    #[cfg(feature = "test-process-release")]
    #[doc(hidden)]
    pub fn test_network_park_hold(&self, hold: bool) {
        if hold {
            PROBE_NETWORK_PARK_ENTERED.store(false, Ordering::SeqCst);
        }
        PROBE_NETWORK_PARK_BLOCK.store(hold, Ordering::SeqCst);
    }

    #[cfg(feature = "test-process-release")]
    #[doc(hidden)]
    pub fn test_network_park_entered(&self) -> bool {
        PROBE_NETWORK_PARK_ENTERED.load(Ordering::SeqCst)
    }

    #[cfg(feature = "test-process-release")]
    #[doc(hidden)]
    pub fn test_retry_join_count(&self) -> u64 {
        PROBE_RETRY_JOIN_COUNT.load(Ordering::SeqCst)
    }
    #[cfg(feature = "test-process-release")]
    #[doc(hidden)]
    pub fn test_fail_next_network_park(&self) -> bool {
        if self.state.network.is_none() {
            return false;
        }
        PROBE_NETWORK_PARK_FAILURE.store(true, Ordering::SeqCst);
        true
    }

    #[cfg(feature = "test-process-release")]
    #[doc(hidden)]
    pub fn test_process_release_report(
        &self,
        deadline: Duration,
    ) -> (&'static str, u64, Vec<String>, Vec<String>, bool) {
        let (result, observation) = self.release_process_observed_internal(deadline);
        self.test_report_from_observation(result, observation)
    }

    #[cfg(feature = "test-process-release")]
    #[doc(hidden)]
    pub fn test_process_retry_report(
        &self,
        deadline: Duration,
    ) -> (&'static str, u64, Vec<String>, Vec<String>, bool) {
        let (result, observation) = self.retry_process_release_observed_internal(deadline);
        self.test_report_from_observation(result, observation)
    }

    #[cfg(feature = "test-process-release")]
    fn test_report_from_observation(
        &self,
        result: ProcessReleaseResult,
        observation: Option<Arc<ReleaseObservation>>,
    ) -> (&'static str, u64, Vec<String>, Vec<String>, bool) {
        let (status, generation) = match result {
            ProcessReleaseResult::Pending { generation } => ("pending", generation),
            ProcessReleaseResult::Released { generation, .. } => ("released", generation),
            ProcessReleaseResult::Failed { generation } => ("failed", generation),
        };
        if let Some(observation) = observation {
            let report = self.safe_release_report_internal(result, &observation);
            (
                status,
                generation,
                report
                    .cleanup_errors
                    .iter()
                    .map(|error| format!("{error:?}"))
                    .collect(),
                report
                    .operation_errors
                    .iter()
                    .map(|error| format!("{error:?}"))
                    .collect(),
                report.explicit_retry_supported,
            )
        } else {
            let cleanup = if status == "failed" {
                vec!["Unconfirmed".to_owned()]
            } else {
                Vec::new()
            };
            let operation = if matches!(
                result,
                ProcessReleaseResult::Released {
                    operation_failed: true,
                    ..
                }
            ) {
                vec!["OtherOperation".to_owned()]
            } else {
                Vec::new()
            };
            (status, generation, cleanup, operation, false)
        }
    }
    #[cfg(feature = "test-process-release")]
    #[doc(hidden)]
    pub fn test_process_safe_report(&self) -> Option<(Vec<String>, Vec<String>, bool)> {
        let observation = self.state.release.current_release_observation()?;
        let result = observation.wait(Duration::ZERO);
        let report = self.safe_release_report_internal(result, &observation);
        Some((
            report
                .cleanup_errors
                .iter()
                .map(|error| format!("{error:?}"))
                .collect(),
            report
                .operation_errors
                .iter()
                .map(|error| format!("{error:?}"))
                .collect(),
            report.explicit_retry_supported,
        ))
    }
    pub(crate) fn safe_release_report_internal(
        &self,
        result: ProcessReleaseResult,
        observation: &ReleaseObservation,
    ) -> ProcessReleaseReport {
        let supported = match result {
            ProcessReleaseResult::Failed { generation } => {
                !self.state.shutdown.is_started()
                    && self
                        .state
                        .local_file
                        .as_ref()
                        .is_none_or(|file| file.queue.has_live_closer())
                    && self
                        .state
                        .network
                        .as_ref()
                        .is_none_or(|network| network.has_live_closer())
                    && self
                        .state
                        .release
                        .retry_is_supported(generation, has_retryable_engine_closer())
            }
            _ => false,
        };
        observation.safe_report(result, supported)
    }
    #[cfg(feature = "test-process-release")]
    #[doc(hidden)]
    pub fn test_process_engine_errors(
        &self,
    ) -> (
        Vec<Arc<dyn std::error::Error + Send + Sync>>,
        Vec<Arc<dyn std::error::Error + Send + Sync>>,
    ) {
        self.state
            .release
            .current_release_observation()
            .map_or_else(
                || (Vec::new(), Vec::new()),
                |observation| observation.probe_engine_errors(),
            )
    }
    #[cfg(feature = "test-process-release")]
    #[allow(dead_code, reason = "Used by source-including probes")]
    pub(crate) fn probe_stop_census(&self) {
        if let Some(census) = self
            .state
            .census
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            census.stop();
        }
    }
    fn terminal_or_ongoing_release(
        &self,
        queue_release: bool,
        deadline: Duration,
    ) -> Result<MutexGuard<'_, ()>, Option<ReleaseStart>> {
        let started = Instant::now();
        loop {
            match self.state.terminal_gate.try_lock() {
                Ok(guard) => return Ok(guard),
                Err(TryLockError::Poisoned(error)) => return Ok(error.into_inner()),
                Err(TryLockError::WouldBlock) => {
                    if queue_release {
                        if let Some(start) = self.state.release.begin_release_during_queued_resume()
                        {
                            return Err(Some(start));
                        }
                    }
                    if !queue_release && self.state.release.resume_is_queued() {
                        return Err(Some(ReleaseStart::Existing { observation: None }));
                    }
                    if let Some(observation) = self.state.release.ongoing_release_observation() {
                        return Err(Some(ReleaseStart::Existing {
                            observation: Some(observation),
                        }));
                    }
                    let remaining = deadline.saturating_sub(started.elapsed());
                    if remaining.is_zero() {
                        return Err(None);
                    }
                    std::thread::sleep(remaining.min(Duration::from_millis(1)));
                }
            }
        }
    }
    #[cfg(feature = "test-process-release")]
    #[doc(hidden)]
    pub fn test_fenced_registration_attempts(&self) -> u64 {
        fenced_registration_attempts()
    }
    #[cfg(feature = "test-process-release")]
    #[doc(hidden)]
    pub fn test_process_release_snapshot(&self, deadline: Duration) -> (&'static str, u64) {
        match self.release_process_internal(deadline) {
            ProcessReleaseResult::Pending { generation } => ("pending", generation),
            ProcessReleaseResult::Released { generation, .. } => ("released", generation),
            ProcessReleaseResult::Failed { generation } => ("failed", generation),
        }
    }

    #[cfg(feature = "test-process-release")]
    #[doc(hidden)]
    pub fn test_process_resume_snapshot(&self, deadline: Duration) -> (&'static str, u64) {
        match self.resume_process_internal(deadline) {
            ProcessResumeResult::Pending { generation } => ("pending", generation),
            ProcessResumeResult::Resumed { generation } => ("resumed", generation),
            ProcessResumeResult::Failed { generation } => ("failed", generation),
            ProcessResumeResult::Superseded { generation } => ("superseded", generation),
        }
    }
    /// 仅供端到端场景验证已确认关闭的责任被移除。
    #[cfg(feature = "test-process-release")]
    #[doc(hidden)]
    pub fn test_retained_engine_count(&self) -> usize {
        retained_engine_count()
    }

    /// 仅供跨 crate 端到端场景观察私有释放结果。
    #[cfg(feature = "test-process-release")]
    #[doc(hidden)]
    pub fn test_process_release(&self, deadline: Duration) -> &'static str {
        match self.release_process_internal(deadline) {
            ProcessReleaseResult::Pending { .. } => "pending",
            ProcessReleaseResult::Released { .. } => "released",
            ProcessReleaseResult::Failed { .. } => "failed",
        }
    }

    /// 仅供跨 crate 端到端场景观察私有显式重试。
    #[cfg(feature = "test-process-release")]
    #[doc(hidden)]
    pub fn test_retry_process_release(&self, deadline: Duration) -> (&'static str, bool) {
        match self.retry_process_release_observed_internal(deadline).0 {
            ProcessReleaseResult::Pending { .. } => ("pending", false),
            ProcessReleaseResult::Released {
                operation_failed, ..
            } => ("released", operation_failed),
            ProcessReleaseResult::Failed { .. } => ("failed", false),
        }
    }

    /// 仅供端到端场景比较私有释放代次。
    #[cfg(feature = "test-process-release")]
    #[doc(hidden)]
    pub fn test_process_release_generation(&self) -> u64 {
        match self.state.release.wait_release(Duration::ZERO) {
            ProcessReleaseResult::Pending { generation }
            | ProcessReleaseResult::Released { generation, .. }
            | ProcessReleaseResult::Failed { generation } => generation,
        }
    }

    #[allow(dead_code, reason = "Used by the standalone process owner probe")]
    pub(crate) fn observe_process_release_internal(
        &self,
        deadline: Duration,
    ) -> ProcessReleaseResult {
        self.state.release.wait_release(deadline)
    }

    /// 私有执行切片；完成并验证全部契约前不向宿主导出。
    pub(crate) fn release_process_internal(&self, deadline: Duration) -> ProcessReleaseResult {
        self.release_process_observed_internal(deadline).0
    }

    /// 返回已接受释放的观察对象；旧代次不能授权后续恢复后的资源。
    pub(crate) fn release_process_observed_internal(
        &self,
        deadline: Duration,
    ) -> (ProcessReleaseResult, Option<Arc<ReleaseObservation>>) {
        let started = Instant::now();
        if let Some(observation) = self.state.release.ongoing_release_observation() {
            return (observation.wait(deadline), Some(observation));
        }
        let start = {
            match self.terminal_or_ongoing_release(true, deadline.saturating_sub(started.elapsed()))
            {
                Ok(_terminal) => {
                    self.state.exports.close_admission();
                    self.state.telemetry.close_admission();
                    self.state.release.begin_release()
                }
                Err(Some(start)) => start,
                Err(None) => {
                    return (
                        ProcessReleaseResult::Pending {
                            generation: self.state.release.current_generation(),
                        },
                        None,
                    )
                }
            }
        };
        let observation = match start {
            ReleaseStart::Started {
                generation,
                fence,
                observation,
            } => {
                self.start_process_release(generation, fence, false, false);
                Some(observation)
            }
            ReleaseStart::Existing { observation } => observation,
        };
        match observation {
            Some(observation) => (
                observation.wait(deadline.saturating_sub(started.elapsed())),
                Some(observation),
            ),
            None => self
                .state
                .release
                .wait_release_observed(deadline.saturating_sub(started.elapsed())),
        }
    }

    pub(crate) fn retry_process_release_observed_internal(
        &self,
        deadline: Duration,
    ) -> (ProcessReleaseResult, Option<Arc<ReleaseObservation>>) {
        let started = Instant::now();
        if self.state.release.resume_is_queued() {
            #[cfg(feature = "test-process-release")]
            PROBE_QUEUED_RESUME_RETRY_COUNT.fetch_add(1, Ordering::SeqCst);
            return self.state.release.wait_release_observed(deadline);
        }
        if let Some(observation) = self.state.release.ongoing_release_observation() {
            #[cfg(feature = "test-process-release")]
            PROBE_RETRY_JOIN_COUNT.fetch_add(1, Ordering::SeqCst);
            return (observation.wait(deadline), Some(observation));
        }
        #[cfg(feature = "test-process-release")]
        if PROBE_RETRY_GATE_ARM.swap(false, Ordering::SeqCst) {
            PROBE_RETRY_GATE_ENTERED.store(true, Ordering::SeqCst);
            let limit = Instant::now() + Duration::from_secs(20);
            while !PROBE_RETRY_GATE_RELEASE.load(Ordering::SeqCst) && Instant::now() < limit {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        let (start, retry_engines) = {
            let _terminal = match self
                .terminal_or_ongoing_release(false, deadline.saturating_sub(started.elapsed()))
            {
                Ok(guard) => guard,
                Err(Some(ReleaseStart::Existing {
                    observation: Some(observation),
                })) => {
                    return (
                        observation.wait(deadline.saturating_sub(started.elapsed())),
                        Some(observation),
                    )
                }
                Err(Some(_)) => {
                    return self
                        .state
                        .release
                        .wait_release_observed(deadline.saturating_sub(started.elapsed()))
                }
                Err(None) => {
                    return (
                        ProcessReleaseResult::Pending {
                            generation: self.state.release.current_generation(),
                        },
                        None,
                    )
                }
            };
            if self.state.shutdown.is_started()
                || !self
                    .state
                    .local_file
                    .as_ref()
                    .is_none_or(|file| file.queue.has_live_closer())
                || !self
                    .state
                    .network
                    .as_ref()
                    .is_none_or(|network| network.has_live_closer())
            {
                (
                    ReleaseStart::Existing {
                        observation: self.state.release.current_release_observation(),
                    },
                    false,
                )
            } else {
                let retry_engines = has_retryable_engine_closer();
                (
                    self.state.release.begin_retry_with_engines(retry_engines),
                    retry_engines,
                )
            }
        };
        let observation = match start {
            ReleaseStart::Started {
                generation,
                fence,
                observation,
            } => {
                self.start_process_release(generation, fence, retry_engines, true);
                Some(observation)
            }
            ReleaseStart::Existing { observation } => observation,
        };
        match observation {
            Some(observation) => (
                observation.wait(deadline.saturating_sub(started.elapsed())),
                Some(observation),
            ),
            None => self
                .state
                .release
                .wait_release_observed(deadline.saturating_sub(started.elapsed())),
        }
    }

    fn start_process_release(
        &self,
        generation: u64,
        fence: Arc<()>,
        retry_engines: bool,
        explicit_retry: bool,
    ) {
        self.state.lifecycle.close_admission();
        self.state.telemetry.close_admission();
        self.state.health_accepting.store(false, Ordering::Release);
        let state = Arc::clone(&self.state);
        let release = Arc::clone(&state.release);
        let worker = move || {
            let Some(prior_operation_failed) = release.wait_release_ready(generation) else {
                return;
            };
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if retry_engines {
                    release.record_operation_errors(
                        generation,
                        ProcessReleaseErrors {
                            engines: retained_engine_errors(),
                            ..ProcessReleaseErrors::default()
                        },
                    );
                    retry_engine_closers();
                }
                let spans_confirmed = loop {
                    if !wait_fenced_idle(&fence, Duration::MAX) {
                        return false;
                    }
                    if let Some(confirmed) =
                        state.telemetry.wait_accepted_spans(&has_unconfirmed_engine)
                    {
                        break confirmed;
                    }
                };
                state.telemetry.pause();
                let _terminal = loop {
                    if !wait_fenced_idle(&fence, Duration::MAX) {
                        return false;
                    }
                    state.exports.wait_idle();
                    let terminal = state
                        .terminal_gate
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if !release.release_is_current(generation) {
                        return false;
                    }
                    if wait_fenced_idle(&fence, Duration::ZERO) {
                        break terminal;
                    }
                    drop(terminal);
                };
                if let Some(census) = state
                    .census
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take()
                {
                    census.stop();
                }
                state.lifecycle.reserve_when_idle();
                let _guard = LifecycleReservation(Arc::clone(&state.lifecycle));
                let signals = state.telemetry.force_flush();
                let operational_error = signals.traces != SignalResult::Completed
                    || signals.logs != SignalResult::Completed;
                release.record_operation_errors(
                    generation,
                    ProcessReleaseErrors {
                        traces: signals.trace_error.clone(),
                        logs: signals.log_error.clone(),
                        ..ProcessReleaseErrors::default()
                    },
                );
                let local = state
                    .local_file
                    .as_ref()
                    .is_none_or(|file| file.queue.park(Duration::MAX) == SignalResult::Completed);
                let network = state.network.as_ref().is_none_or(|network| {
                    if explicit_retry {
                        network.retry_failed_park();
                    }
                    network.park(Duration::MAX) == SignalResult::Completed
                });
                let retryable_files =
                    spans_confirmed && !state.shutdown.is_started() && (!local || !network);
                release.finish_release(
                    generation,
                    spans_confirmed && local && network && !state.shutdown.is_started(),
                    operational_error || prior_operation_failed,
                    retryable_files,
                    ProcessReleaseErrors {
                        local: if local {
                            None
                        } else {
                            state.local_file.as_ref().and_then(|file| file.park_error())
                        },
                        network: if network {
                            None
                        } else {
                            state
                                .network
                                .as_ref()
                                .and_then(|network| network.park_error())
                        },
                        ..ProcessReleaseErrors::default()
                    },
                );
                true
            }));
            if !matches!(outcome, Ok(true)) {
                let _terminal = state
                    .terminal_gate
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                release.finish_release(
                    generation,
                    false,
                    prior_operation_failed,
                    false,
                    ProcessReleaseErrors {
                        engines: retained_engine_errors(),
                        ..ProcessReleaseErrors::default()
                    },
                );
            }
        };
        #[cfg(feature = "test-process-release")]
        let spawned = if PROBE_RELEASE_SPAWN_FAILURE.swap(false, Ordering::SeqCst) {
            Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "controlled process worker failure",
                ),
            ))
        } else {
            std::thread::Builder::new()
                .name("uc-process-release".to_owned())
                .spawn(worker)
        };
        #[cfg(not(feature = "test-process-release"))]
        let spawned = std::thread::Builder::new()
            .name("uc-process-release".to_owned())
            .spawn(worker);
        if let Err(error) = spawned {
            self.state
                .release
                .fail_release_spawn(generation, Arc::new(error));
        }
    }

    /// 私有执行切片；等待超时不撤销已经开始的恢复。
    pub(crate) fn resume_process_internal(&self, deadline: Duration) -> ProcessResumeResult {
        self.resume_process_observed_internal(deadline).0
    }

    /// 返回已接受恢复的观察对象；调用方可在后续代次开始后继续等待它。
    pub(crate) fn resume_process_observed_internal(
        &self,
        deadline: Duration,
    ) -> (ProcessResumeResult, Option<Arc<ResumeObservation>>) {
        let started = Instant::now();
        let pending = if self.state.shutdown.is_started() {
            None
        } else {
            self.state.release.begin_resume_during_release()
        };
        let start = if let Some(start) = pending {
            Some(start)
        } else {
            loop {
                match self.state.terminal_gate.try_lock() {
                    Ok(_terminal) => {
                        break if self.state.shutdown.is_started() {
                            None
                        } else {
                            Some(self.state.release.begin_resume())
                        }
                    }
                    Err(TryLockError::Poisoned(error)) => {
                        let _terminal = error.into_inner();
                        break if self.state.shutdown.is_started() {
                            None
                        } else {
                            Some(self.state.release.begin_resume())
                        };
                    }
                    Err(TryLockError::WouldBlock) => {
                        if !self.state.shutdown.is_started() {
                            if let Some(start) = self.state.release.begin_resume_during_release() {
                                break Some(start);
                            }
                        }
                        let remaining = deadline.saturating_sub(started.elapsed());
                        if remaining.is_zero() {
                            return (
                                ProcessResumeResult::Pending {
                                    generation: self.state.release.current_generation(),
                                },
                                None,
                            );
                        }
                        std::thread::sleep(remaining.min(Duration::from_millis(1)));
                    }
                }
            }
        };
        let Some(start) = start else {
            return (
                self.state
                    .release
                    .terminal_resume_result(deadline.saturating_sub(started.elapsed())),
                None,
            );
        };
        let observation = match start {
            ResumeStart::Started {
                generation,
                fence,
                wait_release,
                observation,
            } => {
                let state = Arc::clone(&self.state);
                let release = Arc::clone(&state.release);
                let spawned = std::thread::Builder::new()
                    .name("uc-process-resume".to_owned())
                    .spawn(move || {
                        if wait_release && !release.wait_resume_ready(generation) {
                            return;
                        }
                        let success =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                if state.shutdown.is_started() {
                                    return false;
                                }
                                if !state.local_file.as_ref().is_none_or(|file| {
                                    file.queue.unpark(Duration::MAX) == SignalResult::Completed
                                }) {
                                    return false;
                                }
                                if !state.network.as_ref().is_none_or(|network| {
                                    network.unpark(Duration::MAX) == SignalResult::Completed
                                }) {
                                    return false;
                                }
                                #[cfg(feature = "test-process-release")]
                                if state.census_expected.load(Ordering::Acquire)
                                    && PROBE_CENSUS_RESUME_BLOCK.load(Ordering::SeqCst)
                                {
                                    PROBE_CENSUS_RESUME_ENTERED.store(true, Ordering::SeqCst);
                                    let limit = Instant::now() + Duration::from_secs(20);
                                    while PROBE_CENSUS_RESUME_BLOCK.load(Ordering::SeqCst)
                                        && Instant::now() < limit
                                    {
                                        std::thread::sleep(Duration::from_millis(10));
                                    }
                                }
                                let _terminal = state
                                    .terminal_gate
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                                if !release.resume_is_current(generation) {
                                    release.finish_resume(generation, true);
                                    return true;
                                }
                                if state.shutdown.is_started() || has_unconfirmed_engine() {
                                    return false;
                                }
                                if state.census_expected.load(Ordering::Acquire) {
                                    let Some(census) = ProcessCensus::start() else {
                                        return false;
                                    };
                                    *state
                                        .census
                                        .lock()
                                        .unwrap_or_else(std::sync::PoisonError::into_inner) =
                                        Some(census);
                                }
                                if !state.telemetry.resume() {
                                    return false;
                                }
                                state.health_accepting.store(true, Ordering::Release);
                                if !resume_engines(&fence) {
                                    return false;
                                }
                                state.exports.open_admission();
                                state.lifecycle.open_admission();
                                release.finish_resume(generation, true);
                                true
                            }))
                            .unwrap_or(false);
                        release.finish_resume(generation, success);
                    });
                if spawned.is_err() {
                    self.state.release.finish_resume(generation, false);
                }
                Some(observation)
            }
            ResumeStart::Existing { observation } => observation,
        };
        let result = observation.as_ref().map_or_else(
            || {
                self.state
                    .release
                    .wait_resume(deadline.saturating_sub(started.elapsed()))
            },
            |observation| observation.wait(deadline.saturating_sub(started.elapsed())),
        );
        (result, observation)
    }

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

    /// Register a host authenticated by this profile's local daemon channel.
    /// The caller must own the process; discovery by process name is not supported.
    pub fn register_host_performance(&self, pid: u32, role: &str) -> std::io::Result<()> {
        self.state
            .telemetry
            .recording
            .offline
            .register_host(pid, role)
    }

    /// Attach the profile-owned cipher without exposing key material to the runtime.
    pub fn protect_offline_diagnostic_history(&self, storage: OfflineHistoryStorage) {
        self.state.telemetry.recording.offline.protect(storage);
    }

    /// Start daemon-only performance collection independently of remote telemetry.
    pub fn start_process_performance(&self) -> std::io::Result<()> {
        self.state
            .telemetry
            .recording
            .offline
            .start(Arc::downgrade(&self.state.telemetry.recording))
    }

    /// Freeze bounded in-memory OTLP records without writing plaintext history.
    pub fn offline_diagnostic_snapshot(&self) -> OfflineDiagnosticSnapshot {
        self.state.telemetry.recording.offline.snapshot()
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
            module_logs: self.state.module_log.snapshot(),
        })
    }
    /// 开启当前进程的限时网络详细采集（1–900 秒）。重复开启不延长；窗口只在内存里，重启不恢复。
    pub fn start_network_capture(
        &self,
        duration: Duration,
    ) -> Result<NetworkCaptureStatus, NetworkLogError> {
        self.network()?.start_capture(duration)
    }

    pub fn stop_network_capture(&self, capture_id: &str) -> Result<(), NetworkLogError> {
        self.network()?.stop_capture(capture_id)
    }

    /// 网络日志的状态与计数。未配置时返回 `NotConfigured`，不伪装成零事件。
    pub fn query_network_log_status(&self) -> Result<NetworkLogStatus, NetworkLogError> {
        Ok(self.network()?.status())
    }

    /// 有界刷新本进程的网络日志并列出本次运行的文件；不代表扩展或其他进程已经刷新。
    pub fn prepare_network_log_export(
        &self,
        deadline: Duration,
    ) -> Result<NetworkLogExportReport, NetworkLogError> {
        let network = self.network()?;
        if deadline < Duration::from_millis(1) || deadline > Duration::from_secs(5) {
            return Err(NetworkLogError::InvalidDeadline);
        }
        let _export = self
            .state
            .exports
            .enter()
            .ok_or(NetworkLogError::AlreadyShutdown)?;
        network.prepare_export(deadline)
    }

    fn network(&self) -> Result<&Arc<NetworkLogRuntime>, NetworkLogError> {
        self.state
            .network
            .as_ref()
            .ok_or(NetworkLogError::NotConfigured)
    }

    pub fn health(&self) -> ObservabilityHealth {
        let mut health = self.state.health;
        health.dropped_local_records = self
            .state
            .local_file
            .as_ref()
            .map_or(0, |local_file| local_file.dropped_records());
        if self
            .state
            .local_file
            .as_ref()
            .is_some_and(|local_file| local_file.unavailable())
        {
            health.local_file = SetupStatus::Unavailable;
        }
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
        let network = self.state.network.clone();
        let (summary_sender, summary_receiver) = mpsc::sync_channel(1);
        match run_reserved_with_deadline(deadline, Arc::clone(&self.state.lifecycle), move || {
            let signals = telemetry.force_flush();
            let summary = FlushSummary {
                traces: signals.traces,
                logs: combine_local_result(
                    signals.logs,
                    local_file.is_none_or(|local_file| local_file.flush(Duration::MAX)),
                ),
            };
            // 业务摘要独立于网络结果，但网络刷新仍属于本次生命周期操作。
            let _ = summary_sender.send(summary);
            if let Some(network) = network {
                let _ = network.flush(Duration::MAX);
            }
            summary
        }) {
            DeadlineOutcome::Completed(summary) => summary,
            DeadlineOutcome::TimedOut => summary_receiver
                .try_recv()
                .unwrap_or_else(|_| FlushSummary::timed_out()),
            DeadlineOutcome::Failed => FlushSummary::failed(),
        }
    }

    pub fn shutdown(&self, deadline: Duration) -> ShutdownSummary {
        let start = {
            let _terminal = self
                .state
                .terminal_gate
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.state.shutdown.begin()
        };
        match start {
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
        // 网络日志在自己的线程里排空，耗时与结果不占用业务日志的关闭预算。
        if let Some(network) = &self.state.network {
            network.shutdown_detached();
        }
        // 资源记录线程只在周期等待中，停止是立即的；先停，避免关闭后继续写记录。
        if let Some(census) = self
            .state
            .census
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            census.stop();
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
    exports: Arc<ExportGate>,
    health_accepting: Arc<AtomicBool>,
    module_log: Arc<ModuleLogStats>,
    /// 独立的网络日志；与业务日志互不依赖，没有配置时为空。
    network: Option<Arc<NetworkLogRuntime>>,
    /// 进程资源周期记录；安装成功且有本地日志目录时设置，关闭时停止。
    census: Mutex<Option<ProcessCensus>>,
    census_expected: AtomicBool,
    release: Arc<ProcessReleaseCoordinator>,
    terminal_gate: Mutex<()>,
}

fn build_runtime(
    config: ObservabilityConfig,
    host_layers: Vec<HostLogLayer>,
) -> (Arc<RuntimeState>, impl tracing::Subscriber + Send + Sync) {
    let health_accepting = Arc::new(AtomicBool::new(true));
    let health_recording = HealthRecording::default();
    let mut layers = Vec::new();
    let (local_file_status, local_file) = match config.local_logs.as_ref() {
        Some(local) => match local_health_layer(
            &local.directory,
            Arc::clone(&health_accepting),
            Arc::clone(&health_recording),
        ) {
            Ok((layer, local_file)) => {
                layers.push(layer);
                (SetupStatus::Ready, Some(local_file))
            }
            Err(()) => (SetupStatus::Unavailable, None),
        },
        None => (SetupStatus::Disabled, None),
    };

    let (telemetry, remote_setup) = TelemetryRuntime::new(&config, local_file.clone());
    let _ = health_recording.set(Arc::clone(&telemetry.recording));
    let telemetry_accepting = telemetry.accepting();
    layers.extend(telemetry.layers());
    layers.push(system_layer());
    let global_telemetry_accepting = Arc::clone(&telemetry_accepting);
    let global_health_accepting = Arc::clone(&health_accepting);
    let engine_layer = layers.with_filter(dynamic_filter_fn(move |metadata, _| {
        let accepting = if metadata.target() == HEALTH_TARGET {
            &global_health_accepting
        } else {
            &global_telemetry_accepting
        };
        accepting.load(Ordering::Acquire) && local_sink_enabled(metadata)
    }));
    let mut all_layers: Vec<HostLogLayer> = Vec::new();
    if let Some(layer) = telemetry.trace_boundary(true) {
        all_layers.push(layer);
    }
    all_layers.push(Box::new(engine_layer));
    let module_log = Arc::new(ModuleLogStats::default());
    if let (Some(file), Some(local)) = (local_file.as_ref(), config.local_logs.as_ref()) {
        let budget_bytes = local.module_log_budget_bytes;
        let recording = Arc::clone(&telemetry.recording);
        let gate = Arc::clone(&recording);
        let layer = ModuleLogLayer::new(
            Arc::clone(file),
            recording,
            Arc::clone(&module_log),
            budget_bytes,
        );
        // 合同 span 只用于取得关联上下文，事件与模块 span 才会被记录。
        all_layers.push(Box::new(layer.with_filter(dynamic_filter_fn(
            move |metadata, _| {
                module_metadata(metadata)
                    && (metadata.is_span()
                        || ModuleLogLayer::level_enabled(&gate, metadata.level()))
            },
        ))));
    }
    // 线程无法创建时降级为不采集；业务日志与 Engine 启动不受影响。
    let network = config
        .network_logs
        .clone()
        .and_then(|network_config| NetworkLogRuntime::new(network_config).ok())
        .map(|(runtime, layer)| {
            all_layers.push(layer);
            runtime
        });
    if !host_layers.is_empty() {
        all_layers.push(Box::new(host_layers.with_filter(
            tracing_subscriber::filter::filter_fn(host_metadata_enabled),
        )));
    }
    if let Some(layer) = telemetry.trace_boundary(false) {
        all_layers.push(layer);
    }
    let subscriber = telemetry.guard_subscriber(tracing_subscriber::registry().with(all_layers));
    let state = Arc::new(RuntimeState {
        config,
        telemetry,
        health: ObservabilityHealth {
            remote: remote_setup.status,
            remote_setup_failure: remote_setup.failure,
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
        exports: Arc::new(ExportGate::default()),
        health_accepting,
        module_log,
        network,
        census: Mutex::new(None),
        census_expected: AtomicBool::new(false),
        release: Arc::new(ProcessReleaseCoordinator::default()),
        terminal_gate: Mutex::new(()),
    });
    (state, subscriber)
}

fn host_metadata_enabled(metadata: &tracing::Metadata<'_>) -> bool {
    if matches!(
        metadata.target(),
        TELEMETRY_TARGET
            | CONNECTIVITY_TARGET
            | LOCAL_DIAGNOSTIC_TARGET
            | HEALTH_TARGET
            | NETWORK_SCOPE_TARGET
    ) {
        return false;
    }
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
    !is_engine_source(metadata.target())
        && !metadata.module_path().is_some_and(is_engine_source)
        && !network_source(metadata.target())
        && !metadata.module_path().is_some_and(network_source)
}

struct HealthAcceptanceGuard(Arc<AtomicBool>);

struct LifecycleReservation(Arc<LifecycleGate>);

impl Drop for LifecycleReservation {
    fn drop(&mut self) {
        self.0.release();
    }
}

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
struct ExportGate {
    state: Mutex<ExportState>,
    idle: Condvar,
}

#[derive(Default)]
struct ExportState {
    active: usize,
    closed: bool,
}

struct ExportReservation(Arc<ExportGate>);

impl ExportGate {
    fn enter(self: &Arc<Self>) -> Option<ExportReservation> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.closed {
            return None;
        }
        state.active = state.active.checked_add(1)?;
        Some(ExportReservation(Arc::clone(self)))
    }

    fn close_admission(&self) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .closed = true;
    }

    fn open_admission(&self) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .closed = false;
    }

    fn wait_idle(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while state.active != 0 {
            state = self
                .idle
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
}

impl Drop for ExportReservation {
    fn drop(&mut self) {
        let mut state = self
            .0
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.active -= 1;
        if state.active == 0 {
            self.0.idle.notify_all();
        }
    }
}

#[derive(Default)]
struct LifecycleGate {
    active: Mutex<LifecycleState>,
    idle: Condvar,
}

#[derive(Default)]
struct LifecycleState {
    active: bool,
    closed: bool,
}

impl LifecycleGate {
    fn try_reserve(&self) -> bool {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if active.active || active.closed {
            return false;
        }
        active.active = true;
        true
    }

    fn reserve_when_idle(&self) {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while active.active {
            active = self
                .idle
                .wait(active)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        active.active = true;
    }

    fn release(&self) {
        self.active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .active = false;
        self.idle.notify_all();
    }

    fn close_admission(&self) {
        self.active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .closed = true;
    }

    fn open_admission(&self) {
        self.active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .closed = false;
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
            // discarded-source[no-information]: the error value carries no usable diagnostic information
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
    use crate::status::RemoteSetupFailure;
    use crate::{
        CaptureEndReason, DeploymentEnvironment, LocalCaptureMode, LocalLogConfig,
        ObservabilityResource, OperatingSystem,
    };

    #[test]
    fn remote_setup_failure_is_written_to_the_local_sink_as_a_closed_category() {
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
        let (state, subscriber) = build_runtime(config, Vec::new());
        let handle = ProcessObservabilityHandle { state };

        tracing::subscriber::with_default(subscriber, || {
            record_remote_setup_degraded(RemoteSetupFailure::TraceExporter);
        });
        assert_eq!(
            handle.shutdown(Duration::from_secs(2)).logs,
            SignalResult::Completed
        );

        let written = crate::managed_log_files(directory.path())
            .expect("日志文件")
            .into_iter()
            .map(|path| std::fs::read_to_string(path).expect("读取日志"))
            .collect::<String>();
        assert!(written.contains("uc.observability.setup_degraded"));
        assert!(written.contains("trace_exporter"));
    }

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
