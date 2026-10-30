//! 独立网络日志的进程内装配：队列、写入策略、层与对外的控制和导出。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::Layer;

use super::config::NetworkLogConfig;
use super::layer::{callsite_enabled, NetworkLogLayer, Shared, MAX_DETAILED_WINDOW};
use super::rules::{Library, RULES};
use super::status::{
    NetworkCaptureMode, NetworkCaptureStatus, NetworkCounters, NetworkLibraryStatus,
    NetworkLogError, NetworkLogExportReport, NetworkLogFile, NetworkLogState, NetworkLogStatus,
    NETWORK_LOG_SCHEMA_VERSION,
};
use super::store::{list_segments, run_directory, RunStore};
use crate::local_file::{lock, FileQueue, HealthHooks, RecordCounters};
use crate::{HostLogLayer, SignalResult};

/// 网络日志不写 Engine 的健康记录：它的可用性只通过自己的状态与导出报告表达，与业务日志互不牵连。
const NETWORK_HOOKS: HealthHooks = HealthHooks {
    failed: |_| {},
    recovered: || {},
};

pub(crate) struct NetworkLogRuntime {
    config: NetworkLogConfig,
    run_id: String,
    queue: FileQueue,
    shared: Arc<Shared>,
    counters: Arc<NetworkCounters>,
    prepared: Arc<AtomicBool>,
    closing: Arc<Mutex<CloseState>>,
}

struct CloseState {
    phase: ClosePhase,
    operation: Arc<()>,
    parking: Option<Arc<ParkWait>>,
}

#[derive(Default)]
struct ParkWait {
    accepted: AtomicBool,
    result: Mutex<Option<SignalResult>>,
    changed: Condvar,
}

impl ParkWait {
    fn complete(&self, result: SignalResult) {
        *lock(&self.result) = Some(result);
        self.changed.notify_all();
    }

    fn wait(&self, deadline: Duration) -> SignalResult {
        let started = Instant::now();
        let mut result = lock(&self.result);
        loop {
            if let Some(result) = *result {
                return result;
            }
            let Some(remaining) = deadline.checked_sub(started.elapsed()) else {
                return SignalResult::TimedOut;
            };
            let (next, timeout) = self
                .changed
                .wait_timeout(result, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            result = next;
            if timeout.timed_out() && result.is_none() {
                return SignalResult::TimedOut;
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ClosePhase {
    Open,
    /// 文件线程尚未确认停车；不能将等待超时当作关闭回执。
    Parking,
    /// 文件线程尚未确认重开；旧停车结果不能证明当前状态。
    Reopening,
    /// 文件线程已确认文件与租约交还。
    Parked,
    Closing,
    CloseUnconfirmed,
    Closed,
}

impl NetworkLogRuntime {
    pub(crate) fn park_error(&self) -> Option<Arc<std::io::Error>> {
        self.queue.park_error()
    }
    pub(crate) fn retry_failed_park(&self) {
        let mut state = lock(&self.closing);
        if state.phase == ClosePhase::Parking
            && state
                .parking
                .as_ref()
                .is_some_and(|parking| *lock(&parking.result) == Some(SignalResult::Failed))
            && self.queue.has_live_closer()
        {
            state.parking = None;
        }
    }
    pub(crate) fn has_live_closer(&self) -> bool {
        matches!(
            lock(&self.closing).phase,
            ClosePhase::Open | ClosePhase::Parking | ClosePhase::Reopening | ClosePhase::Parked
        ) && self.queue.has_live_closer()
    }
    /// 构造运行时和要装入进程 subscriber 的层。目录打开在工作线程里完成且可重试，
    /// 这里失败只可能是线程无法创建，由调用方降级为不采集网络日志。
    pub(crate) fn new(config: NetworkLogConfig) -> std::io::Result<(Arc<Self>, HostLogLayer)> {
        let run_id = uuid::Uuid::new_v4().simple().to_string();
        let counters = Arc::new(NetworkCounters::default());
        let prepared = Arc::new(AtomicBool::new(false));
        let store = RunStore::new(
            &config.root,
            config.role.as_str(),
            run_id.clone(),
            config.budget,
            Arc::clone(&counters),
            Arc::clone(&prepared),
        );
        let queue = FileQueue::spawn(
            store,
            Arc::clone(&counters) as Arc<dyn RecordCounters>,
            NETWORK_HOOKS,
            "uc-network-log-file",
            config.budget.queue_records,
        )?;
        let (layer, shared) = NetworkLogLayer::new(
            queue.writer(),
            Arc::clone(&counters),
            run_id.clone(),
            config.known_alpns.clone(),
            config.budget.record_bytes,
        );
        let runtime = Arc::new(Self {
            config,
            run_id,
            queue,
            shared,
            counters,
            prepared,
            closing: Arc::new(Mutex::new(CloseState {
                phase: ClosePhase::Open,
                operation: Arc::new(()),
                parking: None,
            })),
        });
        Ok((
            runtime,
            Box::new(layer.with_filter(filter_fn(callsite_enabled))),
        ))
    }

    fn phase(&self) -> ClosePhase {
        lock(&self.closing).phase
    }

    fn accepts_control(&self) -> bool {
        matches!(
            self.phase(),
            ClosePhase::Open | ClosePhase::Parking | ClosePhase::Reopening | ClosePhase::Parked
        )
    }

    pub(crate) fn start_capture(
        &self,
        duration: Duration,
    ) -> Result<NetworkCaptureStatus, NetworkLogError> {
        if !self.accepts_control() {
            return Err(NetworkLogError::AlreadyShutdown);
        }
        if duration < Duration::from_secs(1) || duration > MAX_DETAILED_WINDOW {
            return Err(NetworkLogError::InvalidDuration);
        }
        let (capture, already_active) = self.shared.start_capture(duration);
        Ok(NetworkCaptureStatus {
            capture_id: capture.id,
            started_at_utc: capture.started_at_utc,
            duration_ms: u64::try_from(capture.duration.as_millis()).unwrap_or(u64::MAX),
            already_active,
        })
    }

    pub(crate) fn stop_capture(&self, capture_id: &str) -> Result<(), NetworkLogError> {
        if !self.accepts_control() {
            return Err(NetworkLogError::AlreadyShutdown);
        }
        if self.shared.stop_capture(capture_id) {
            Ok(())
        } else {
            Err(NetworkLogError::CaptureMismatch)
        }
    }

    pub(crate) fn status(&self) -> NetworkLogStatus {
        let remaining = self.shared.detailed_remaining_ms();
        let state = match self.phase() {
            // 停车期间文件已关闭；Engine 恢复后回到 Ready。
            ClosePhase::Closed | ClosePhase::Parked => NetworkLogState::Closed,
            ClosePhase::Parking
            | ClosePhase::Reopening
            | ClosePhase::Closing
            | ClosePhase::CloseUnconfirmed => NetworkLogState::Unavailable,
            ClosePhase::Open if self.queue.unavailable() => NetworkLogState::Unavailable,
            ClosePhase::Open if !self.prepared.load(Ordering::Acquire) => NetworkLogState::Starting,
            ClosePhase::Open => NetworkLogState::Ready,
        };
        let libraries = Library::ALL
            .iter()
            .map(|library| NetworkLibraryStatus {
                library: library.as_str(),
                version_dir: RULES
                    .iter()
                    .find(|rule| rule.library == *library)
                    .map(|rule| rule.version_dir),
                approved_rules: RULES.iter().filter(|rule| rule.library == *library).count(),
                observed_records: self.counters.by_library[*library as usize]
                    .load(Ordering::Relaxed),
            })
            .collect();
        NetworkLogStatus {
            schema_version: NETWORK_LOG_SCHEMA_VERSION,
            state,
            run_id: self.run_id.clone(),
            role: self.config.role,
            mode: if remaining.is_some() {
                NetworkCaptureMode::Detailed
            } else {
                NetworkCaptureMode::Standard
            },
            detailed_remaining_ms: remaining,
            counters: self.counters.snapshot(),
            libraries,
            sampled_at_utc: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        }
    }

    pub(crate) fn flush(&self, deadline: Duration) -> SignalResult {
        self.queue.flush_result(deadline)
    }

    /// 刷新后列出本次运行的文件及完整 SHA-256。刷新失败时仍报告能读到的文件，由 `flush` 说明原因。
    pub(crate) fn prepare_export(
        &self,
        deadline: Duration,
    ) -> Result<NetworkLogExportReport, NetworkLogError> {
        if deadline < Duration::from_millis(1) || deadline > Duration::from_secs(5) {
            return Err(NetworkLogError::InvalidDeadline);
        }
        let requested_at_utc =
            chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let flush = if self.phase() == ClosePhase::Open {
            self.queue.flush_result(deadline)
        } else {
            SignalResult::AlreadyShutdown
        };
        let run_directory =
            run_directory(&self.config.root, self.config.role.as_str(), &self.run_id);
        let files = list_segments(&run_directory)
            .map(|segments| {
                segments
                    .into_iter()
                    .filter_map(|(name, length)| describe_file(&run_directory, name, length))
                    .collect()
            })
            .unwrap_or_default();
        Ok(NetworkLogExportReport {
            flush,
            status: self.status(),
            requested_at_utc,
            completed_at_utc: chrono::Utc::now()
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            other_processes_flushed: false,
            run_directory,
            files,
        })
    }

    /// 已接受的记录落盘后关闭当前分段并交还运行租约；之后到达的记录计入封口丢弃，不打开任何文件。
    /// 重复停车不重复执行。会等待文件线程，不能在异步运行线程上调用。
    pub(crate) fn park(&self, deadline: Duration) -> SignalResult {
        let (operation, parking) = {
            let mut state = lock(&self.closing);
            match state.phase {
                ClosePhase::Parking if state.parking.is_some() => {
                    let parking = state.parking.as_ref().cloned();
                    drop(state);
                    return parking.map_or(SignalResult::Failed, |parking| parking.wait(deadline));
                }
                ClosePhase::Parked => return SignalResult::Completed,
                ClosePhase::Open | ClosePhase::Parking | ClosePhase::Reopening => {
                    state.phase = ClosePhase::Parking;
                    state.operation = Arc::new(());
                    let parking = Arc::new(ParkWait::default());
                    state.parking = Some(Arc::clone(&parking));
                    (Arc::clone(&state.operation), parking)
                }
                ClosePhase::Closing | ClosePhase::CloseUnconfirmed | ClosePhase::Closed => {
                    return SignalResult::AlreadyShutdown;
                }
            }
        };
        let closing = Arc::clone(&self.closing);
        let observed = Arc::clone(&parking);
        let accepted = Arc::clone(&parking);
        let result = self.queue.park_with_observer(
            deadline,
            Box::new(move |success| {
                let mut state = lock(&closing);
                if success
                    && state.phase == ClosePhase::Parking
                    && Arc::ptr_eq(&state.operation, &operation)
                {
                    state.phase = ClosePhase::Parked;
                }
                observed.complete(if success {
                    SignalResult::Completed
                } else {
                    SignalResult::Failed
                });
            }),
            Box::new(move || accepted.accepted.store(true, Ordering::Release)),
        );
        if !parking.accepted.load(Ordering::Acquire) {
            parking.complete(result);
            let mut state = lock(&self.closing);
            if state
                .parking
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &parking))
            {
                state.parking = None;
            }
        } else if result == SignalResult::Failed {
            // 线程断开时不会再调用确认回调，等待者必须看到终止失败。
            parking.complete(SignalResult::Failed);
        }
        result
    }

    /// 以同一运行编号重新取得租约并打开新分段；运行目录若已被其他进程回收则重建。
    pub(crate) fn unpark(&self, deadline: Duration) -> SignalResult {
        let operation = {
            let mut state = lock(&self.closing);
            match state.phase {
                ClosePhase::Parked | ClosePhase::Parking | ClosePhase::Reopening => {
                    state.phase = ClosePhase::Reopening;
                    state.operation = Arc::new(());
                    state.parking = None;
                    Arc::clone(&state.operation)
                }
                ClosePhase::Open => return SignalResult::Completed,
                ClosePhase::Closing | ClosePhase::CloseUnconfirmed | ClosePhase::Closed => {
                    return SignalResult::AlreadyShutdown;
                }
            }
        };
        let closing = Arc::clone(&self.closing);
        self.queue.unpark_with_observer(
            deadline,
            Box::new(move |success| {
                let mut state = lock(&closing);
                if success
                    && state.phase == ClosePhase::Reopening
                    && Arc::ptr_eq(&state.operation, &operation)
                {
                    state.phase = ClosePhase::Open;
                }
            }),
        )
    }

    /// 在独立线程里排空并关闭网络队列：它的耗时和结果不进入业务日志的关闭摘要。
    pub(crate) fn shutdown_detached(self: &Arc<Self>) {
        {
            let mut state = lock(&self.closing);
            if matches!(state.phase, ClosePhase::Closing | ClosePhase::Closed) {
                return;
            }
            state.phase = ClosePhase::Closing;
            state.operation = Arc::new(());
        }
        let runtime = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("uc-network-log-shutdown".to_owned())
            .spawn(move || {
                let completed = runtime.queue.shutdown_until_complete();
                lock(&runtime.closing).phase = if completed {
                    ClosePhase::Closed
                } else {
                    ClosePhase::CloseUnconfirmed
                };
            });
        if spawned.is_err() {
            lock(&self.closing).phase = ClosePhase::CloseUnconfirmed;
        }
    }
}

/// 只读取打开时已有的长度，避免写入方继续追加造成长度与摘要不一致。
fn describe_file(directory: &std::path::Path, name: String, length: u64) -> Option<NetworkLogFile> {
    use std::io::Read;
    let file = std::fs::File::open(directory.join(&name)).ok()?;
    let mut reader = file.take(length);
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 16 * 1024];
    let mut read_total = 0_u64;
    loop {
        let count = reader.read(&mut buffer).ok()?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        read_total += u64::try_from(count).ok()?;
    }
    Some(NetworkLogFile {
        name,
        bytes: read_total,
        sha256: hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    })
}
