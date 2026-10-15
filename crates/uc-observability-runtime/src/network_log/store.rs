//! 网络日志的运行目录与分段文件。
//!
//! 目录：`<日志根>/network/<role>/<run_id>/`，内含 `lease.lock`（活动写入者持有的独占锁）、
//! `run.json`（本次运行的来源清单）和 `network.<UTC日期>.<序号>.jsonl` 分段文件。
//! 每个进程每次运行独占一个目录，不同进程或不同运行不会追加到同一个文件。

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::SystemTime;

use chrono::Utc;

use super::config::NetworkLogBudget;
use super::rules::{Library, RULES};
use super::status::{NetworkCounters, NETWORK_LOG_SCHEMA_VERSION};
use crate::build_source::build_source;
use crate::file_statistics::increment;
use crate::local_file::{open_managed_file, same_file, RecordStore};

const LEASE_FILE: &str = "lease.lock";
const CLEANUP_LOCK_FILE: &str = ".cleanup.lock";
const MANIFEST_FILE: &str = "run.json";
const SEGMENT_PREFIX: &str = "network.";
const SEGMENT_SUFFIX: &str = ".jsonl";

/// 一次运行目录的写入策略；打开是惰性且可重试的，初始化失败与运行中失败走同一个恢复路径。
pub(crate) struct RunStore {
    network_root: PathBuf,
    role: &'static str,
    run_id: String,
    budget: NetworkLogBudget,
    started_at: String,
    counters: Arc<NetworkCounters>,
    prepared: Arc<AtomicBool>,
    open: Option<OpenRun>,
    reopened: u64,
}

struct OpenRun {
    directory: PathBuf,
    /// 持有到本次运行结束：进程退出时由操作系统释放，其他进程据此判断运行是否仍然活动。
    _lease: File,
    file: File,
    file_name: String,
    next_sequence: u32,
    current_bytes: u64,
    /// 包含当前分段，最旧的在前。
    segments: VecDeque<(String, u64)>,
    total_bytes: u64,
}

impl RunStore {
    pub(crate) fn new(
        log_root: &Path,
        role: &'static str,
        run_id: String,
        budget: NetworkLogBudget,
        counters: Arc<NetworkCounters>,
        prepared: Arc<AtomicBool>,
    ) -> Self {
        Self {
            network_root: log_root.join("network"),
            role,
            run_id,
            budget,
            started_at: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            counters,
            prepared,
            open: None,
            reopened: 0,
        }
    }

    fn run_directory(&self) -> PathBuf {
        self.network_root.join(self.role).join(&self.run_id)
    }

    fn ensure_open(&mut self) -> io::Result<&mut OpenRun> {
        if self.open.is_none() {
            let run = self.open_run()?;
            self.open = Some(run);
            self.prepared.store(true, Ordering::Release);
        }
        self.open
            .as_mut()
            .ok_or_else(|| io::Error::other("network log run is not open"))
    }

    fn open_run(&mut self) -> io::Result<OpenRun> {
        let directory = self.run_directory();
        create_private_directory(&directory)?;
        let lease = open_lease(&directory.join(LEASE_FILE))?;
        // 回收旧运行不影响本次写入；失败只留到下次，不让日志清理拖垮采集。
        let _ = collect_old_runs(&self.network_root, &directory, &self.budget);
        let manifest = self.manifest();
        write_atomically(&directory.join(MANIFEST_FILE), &manifest)?;
        let sequence = next_sequence(&directory)?;
        let (file, file_name) = open_segment(&directory, sequence)?;
        Ok(OpenRun {
            directory,
            _lease: lease,
            file,
            segments: VecDeque::from([(file_name.clone(), 0)]),
            file_name,
            next_sequence: sequence.saturating_add(1),
            current_bytes: 0,
            total_bytes: 0,
        })
    }

    fn manifest(&self) -> Vec<u8> {
        let source = build_source();
        let libraries: Vec<_> = Library::ALL
            .iter()
            .map(|library| {
                let version_dir = RULES
                    .iter()
                    .find(|rule| rule.library == *library)
                    .map(|rule| rule.version_dir);
                serde_json::json!({
                    "library": library.as_str(),
                    "version_dir": version_dir,
                    "approved_rules": RULES.iter().filter(|rule| rule.library == *library).count(),
                })
            })
            .collect();
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": NETWORK_LOG_SCHEMA_VERSION,
            "run_id": self.run_id,
            "role": self.role,
            "pid": std::process::id(),
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "started_at_utc": self.started_at,
            "reopened": self.reopened,
            "engine_source_commit": source.commit,
            "engine_source_state": source.state,
            "libraries": libraries,
            "budget": {
                "record_bytes": self.budget.record_bytes,
                "queue_records": self.budget.queue_records,
                "file_bytes": self.budget.file_bytes,
                "run_bytes": self.budget.run_bytes,
                "root_bytes": self.budget.root_bytes,
                "retention_secs": self.budget.retention.as_secs(),
            },
        }))
        .unwrap_or_default()
    }

    /// 目录或当前分段被外部删除、替换后重建，不让记录落入已脱离目录的文件。
    fn ensure_current(&mut self) -> io::Result<()> {
        let intact = self.open.as_ref().is_some_and(|run| {
            std::fs::symlink_metadata(run.directory.join(&run.file_name))
                .is_ok_and(|metadata| same_file(&metadata, &run.file))
                && std::fs::symlink_metadata(run.directory.join(LEASE_FILE)).is_ok()
        });
        if intact {
            return Ok(());
        }
        if self.open.take().is_some() {
            increment(&self.counters.path_recovered);
            self.reopened = self.reopened.saturating_add(1);
        }
        self.ensure_open().map(|_| ())
    }
}

impl RecordStore for RunStore {
    fn prepare(&mut self) -> io::Result<()> {
        self.ensure_open().map(|_| ())
    }

    fn store_record(&mut self, bytes: &[u8], _source: usize) -> io::Result<()> {
        self.ensure_current()?;
        let budget = self.budget;
        let counters = Arc::clone(&self.counters);
        let run = self.ensure_open()?;
        let additional = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if run.current_bytes.saturating_add(additional) > budget.file_bytes {
            run.start_segment()?;
        }
        run.make_room(additional, budget.run_bytes, &counters)?;
        run.file.write_all(bytes)?;
        run.current_bytes = run.current_bytes.saturating_add(additional);
        run.total_bytes = run.total_bytes.saturating_add(additional);
        if let Some(last) = run.segments.back_mut() {
            last.1 = run.current_bytes;
        }
        increment(&counters.written);
        Ok(())
    }

    fn settle(&mut self) -> io::Result<()> {
        self.ensure_current()?;
        self.ensure_open()?.file.flush()
    }
}

impl OpenRun {
    fn start_segment(&mut self) -> io::Result<()> {
        let (file, name) = open_segment(&self.directory, self.next_sequence)?;
        self.file = file;
        self.file_name = name.clone();
        self.next_sequence = self.next_sequence.saturating_add(1);
        self.current_bytes = 0;
        self.segments.push_back((name, 0));
        Ok(())
    }

    /// 为新记录腾出本次运行的预算：从最旧的分段起淘汰，永不删除当前分段。
    fn make_room(
        &mut self,
        additional: u64,
        run_bytes: u64,
        counters: &NetworkCounters,
    ) -> io::Result<()> {
        while self.total_bytes.saturating_add(additional) > run_bytes && self.segments.len() > 1 {
            let Some((name, size)) = self.segments.pop_front() else {
                break;
            };
            std::fs::remove_file(self.directory.join(&name))?;
            self.total_bytes = self.total_bytes.saturating_sub(size);
            increment(&counters.rotated_out_files);
        }
        Ok(())
    }
}

fn next_sequence(directory: &Path) -> io::Result<u32> {
    let mut highest = 0_u32;
    for entry in std::fs::read_dir(directory)? {
        let name = entry?.file_name();
        if let Some(sequence) = name.to_str().and_then(segment_sequence) {
            highest = highest.max(sequence.saturating_add(1));
        }
    }
    Ok(highest)
}

/// 严格识别 `network.<8位日期>.<序号>.jsonl`。
fn segment_sequence(name: &str) -> Option<u32> {
    let rest = name
        .strip_prefix(SEGMENT_PREFIX)?
        .strip_suffix(SEGMENT_SUFFIX)?;
    let (date, sequence) = rest.split_once('.')?;
    (date.len() == 8 && date.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| sequence.parse().ok())
        .flatten()
}

fn create_private_directory(directory: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(directory)
    }
}

fn open_lease(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?;
    file.try_lock().map_err(|error| match error {
        std::fs::TryLockError::WouldBlock => io::Error::new(
            io::ErrorKind::WouldBlock,
            "network log run lease is held by another writer",
        ),
        std::fs::TryLockError::Error(error) => error,
    })?;
    Ok(file)
}

fn open_segment(directory: &Path, sequence: u32) -> io::Result<(File, String)> {
    let name = format!(
        "{SEGMENT_PREFIX}{}.{sequence:03}{SEGMENT_SUFFIX}",
        Utc::now().format("%Y%m%d")
    );
    let file = open_managed_file(&directory.join(&name))?;
    Ok((file, name))
}

fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(&temporary, path)
}

struct RunEntry {
    path: PathBuf,
    bytes: u64,
    modified: SystemTime,
}

/// 回收没有活动写入者的旧运行目录：先按保留时间，再在根目录超额时从最旧的起。
///
/// 清理锁串行不同进程的扫描；运行是否活动只看它的 lease 锁，成功取得锁才可删除。
/// 锁忙、权限失败或任何 I/O 错误都跳过，不等待、不影响采集。
fn collect_old_runs(
    network_root: &Path,
    own_directory: &Path,
    budget: &NetworkLogBudget,
) -> io::Result<()> {
    let cleanup = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(network_root.join(CLEANUP_LOCK_FILE))?;
    if cleanup.try_lock().is_err() {
        return Ok(());
    }
    let mut runs = Vec::new();
    for role in std::fs::read_dir(network_root)? {
        let role = role?;
        if !role.file_type()?.is_dir() {
            continue;
        }
        for run in std::fs::read_dir(role.path())? {
            let run = run?;
            if run.file_type()?.is_dir() {
                runs.push(describe_run(run.path()));
            }
        }
    }
    runs.sort_by_key(|run| run.modified);
    let now = SystemTime::now();
    let mut total: u64 = runs.iter().map(|run| run.bytes).sum();
    for run in runs {
        if run.path == own_directory {
            continue;
        }
        let expired = now
            .duration_since(run.modified)
            .is_ok_and(|age| age > budget.retention);
        if !expired && total <= budget.root_bytes {
            continue;
        }
        if remove_inactive_run(&run.path) {
            total = total.saturating_sub(run.bytes);
        }
    }
    Ok(())
}

fn describe_run(path: PathBuf) -> RunEntry {
    let mut bytes = 0_u64;
    let mut modified = SystemTime::UNIX_EPOCH;
    if let Ok(entries) = std::fs::read_dir(&path) {
        for entry in entries.flatten() {
            if let Ok(metadata) = entry.metadata() {
                bytes = bytes.saturating_add(metadata.len());
                if let Ok(time) = metadata.modified() {
                    modified = modified.max(time);
                }
            }
        }
    }
    RunEntry {
        path,
        bytes,
        modified,
    }
}

/// 取得 lease 锁才说明运行不再活动；持锁期间删除目录，缺少 lease 的目录视为孤儿。
fn remove_inactive_run(directory: &Path) -> bool {
    let lease_path = directory.join(LEASE_FILE);
    let lease = match OpenOptions::new().write(true).open(&lease_path) {
        Ok(lease) => Some(lease),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(_) => return false,
    };
    if let Some(lease) = &lease {
        if lease.try_lock().is_err() {
            return false;
        }
    }
    #[cfg(windows)]
    drop(lease);
    std::fs::remove_dir_all(directory).is_ok()
}

/// 列出运行目录里的分段文件及其长度（按名称升序），供导出使用。
pub(crate) fn list_segments(directory: &Path) -> io::Result<Vec<(String, u64)>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if segment_sequence(&name).is_some() && entry.file_type()?.is_file() {
            files.push((name, entry.metadata()?.len()));
        }
    }
    files.sort();
    Ok(files)
}

pub(crate) fn run_directory(log_root: &Path, role: &'static str, run_id: &str) -> PathBuf {
    log_root.join("network").join(role).join(run_id)
}
