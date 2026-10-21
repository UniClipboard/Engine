//! 独立网络日志的进程内装配：队列、写入策略、层与对外的控制和导出。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

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
    closing: Arc<Mutex<ClosePhase>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ClosePhase {
    Open,
    /// 进程内没有工作中的 Engine：文件与租约已交还，记录直接丢弃，直到重开。
    Parked,
    Closing,
    Closed,
}

impl NetworkLogRuntime {
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
            closing: Arc::new(Mutex::new(ClosePhase::Open)),
        });
        Ok((
            runtime,
            Box::new(layer.with_filter(filter_fn(callsite_enabled))),
        ))
    }

    fn phase(&self) -> ClosePhase {
        *lock(&self.closing)
    }

    fn accepts_control(&self) -> bool {
        matches!(self.phase(), ClosePhase::Open | ClosePhase::Parked)
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
            ClosePhase::Closed | ClosePhase::Closing | ClosePhase::Parked => {
                NetworkLogState::Closed
            }
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
        {
            let mut phase = lock(&self.closing);
            match *phase {
                ClosePhase::Open => *phase = ClosePhase::Parked,
                ClosePhase::Parked => return SignalResult::Completed,
                ClosePhase::Closing | ClosePhase::Closed => return SignalResult::AlreadyShutdown,
            }
        }
        self.queue.park(deadline)
    }

    /// 以同一运行编号重新取得租约并打开新分段；运行目录若已被其他进程回收则重建。
    pub(crate) fn unpark(&self, deadline: Duration) -> SignalResult {
        {
            let mut phase = lock(&self.closing);
            match *phase {
                ClosePhase::Parked => *phase = ClosePhase::Open,
                ClosePhase::Open => return SignalResult::Completed,
                ClosePhase::Closing | ClosePhase::Closed => return SignalResult::AlreadyShutdown,
            }
        }
        self.queue.unpark(deadline)
    }

    /// 在独立线程里排空并关闭网络队列：它的耗时和结果不进入业务日志的关闭摘要。
    pub(crate) fn shutdown_detached(self: &Arc<Self>, deadline: Duration) {
        {
            let mut phase = lock(&self.closing);
            if !matches!(*phase, ClosePhase::Open | ClosePhase::Parked) {
                return;
            }
            *phase = ClosePhase::Closing;
        }
        let runtime = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("uc-network-log-shutdown".to_owned())
            .spawn(move || {
                runtime.queue.shutdown(deadline);
                *lock(&runtime.closing) = ClosePhase::Closed;
            });
        if spawned.is_err() {
            *lock(&self.closing) = ClosePhase::Closed;
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
