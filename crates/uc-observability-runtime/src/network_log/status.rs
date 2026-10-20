//! 独立网络日志的状态、计数与导出摘要。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;

use super::config::NetworkProcessRole;
use super::rules::Library;
use crate::file_statistics::increment;
use crate::local_file::RecordCounters;
use crate::SignalResult;

pub const NETWORK_LOG_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkLogState {
    /// 工作线程还没有完成首次打开。
    Starting,
    Ready,
    /// 最近一次打开、写入或刷新失败且尚未恢复；业务日志与业务流程不受影响。
    Unavailable,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkCaptureMode {
    Standard,
    Detailed,
}

/// 当前进程内的单调累计计数；崩溃前未落盘的最终值未知，导出方不得用零代替。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct NetworkLogCounters {
    /// 进入写入队列的记录。
    pub accepted: u64,
    /// 已写入文件的记录。
    pub written: u64,
    /// 调用点已审定但当前采集深度不放行，或来自未审查的调用点。
    pub policy_filtered: u64,
    /// 超出调用点令牌桶而丢弃。
    pub rate_limited: u64,
    /// 编码后超过单条上限而丢弃。
    pub oversized: u64,
    pub queue_dropped: u64,
    /// 队列封口后到达而丢弃。
    pub closed_dropped: u64,
    pub write_failed: u64,
    /// 超出本次运行预算而淘汰的旧分段文件数。
    pub rotated_out_files: u64,
    /// 目录或文件被外部删除、替换后重建的次数。
    pub path_recovered: u64,
    /// 未审查的 WARN/ERROR 调用点出现的次数。
    pub unreviewed_callsites: u64,
}

#[derive(Default)]
pub(crate) struct NetworkCounters {
    pub(crate) accepted: AtomicU64,
    pub(crate) written: AtomicU64,
    pub(crate) policy_filtered: AtomicU64,
    pub(crate) rate_limited: AtomicU64,
    pub(crate) oversized: AtomicU64,
    pub(crate) queue_dropped: AtomicU64,
    pub(crate) closed_dropped: AtomicU64,
    pub(crate) write_failed: AtomicU64,
    pub(crate) rotated_out_files: AtomicU64,
    pub(crate) path_recovered: AtomicU64,
    pub(crate) unreviewed_callsites: AtomicU64,
    /// 按库统计放行的记录数，下标是 `Library as usize`。
    pub(crate) by_library: [AtomicU64; Library::COUNT],
}

impl NetworkCounters {
    pub(crate) fn snapshot(&self) -> NetworkLogCounters {
        let load = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
        NetworkLogCounters {
            accepted: load(&self.accepted),
            written: load(&self.written),
            policy_filtered: load(&self.policy_filtered),
            rate_limited: load(&self.rate_limited),
            oversized: load(&self.oversized),
            queue_dropped: load(&self.queue_dropped),
            closed_dropped: load(&self.closed_dropped),
            write_failed: load(&self.write_failed),
            rotated_out_files: load(&self.rotated_out_files),
            path_recovered: load(&self.path_recovered),
            unreviewed_callsites: load(&self.unreviewed_callsites),
        }
    }
}

impl RecordCounters for NetworkCounters {
    fn accepted(&self, _source: usize) {
        increment(&self.accepted);
    }

    fn queue_dropped(&self, _source: usize) {
        increment(&self.queue_dropped);
    }

    fn closed_dropped(&self, _source: usize) {
        increment(&self.closed_dropped);
    }

    fn write_failed(&self, _source: usize) {
        increment(&self.write_failed);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NetworkLibraryStatus {
    pub library: &'static str,
    /// 规则绑定的精确库版本目录。
    pub version_dir: Option<&'static str>,
    /// 该库已审定的事件规则数。
    pub approved_rules: usize,
    /// 本次运行里该库放行的记录数；零只表示本进程没有观察到，不是“库没有活动”的证明。
    pub observed_records: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NetworkLogStatus {
    pub schema_version: u32,
    pub state: NetworkLogState,
    pub run_id: String,
    pub role: NetworkProcessRole,
    pub mode: NetworkCaptureMode,
    /// 详细采集窗口的剩余毫秒；标准档为空。
    pub detailed_remaining_ms: Option<u64>,
    pub counters: NetworkLogCounters,
    pub libraries: Vec<NetworkLibraryStatus>,
    pub sampled_at_utc: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NetworkCaptureStatus {
    pub capture_id: String,
    pub started_at_utc: String,
    pub duration_ms: u64,
    /// 已有窗口时重复开始不延长，返回现有窗口并置为 `true`。
    pub already_active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NetworkLogFile {
    pub name: String,
    pub bytes: u64,
    /// 导出时按打开时的长度计算的完整文件 SHA-256（十六进制）。
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NetworkLogExportReport {
    pub flush: SignalResult,
    pub status: NetworkLogStatus,
    pub requested_at_utc: String,
    pub completed_at_utc: String,
    /// 本进程刷新不代表其他进程（扩展、其他角色）已经刷新。
    pub other_processes_flushed: bool,
    /// 本次运行目录；只列出本进程自己的运行，旧运行由宿主按目录发现。
    #[serde(skip)]
    pub run_directory: PathBuf,
    pub files: Vec<NetworkLogFile>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum NetworkLogError {
    #[error("network logging is not configured for this process")]
    NotConfigured,
    #[error("network log capture window must be between 1 and 900 seconds")]
    InvalidDuration,
    #[error("network log capture id does not match the active window")]
    CaptureMismatch,
    #[error("network logging is already shut down")]
    AlreadyShutdown,
    #[error("network log export deadline must be between 1 and 5000 ms")]
    InvalidDeadline,
}
