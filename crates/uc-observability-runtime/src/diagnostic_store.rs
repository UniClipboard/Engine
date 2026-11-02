//! 诊断三种信号共用的分段存储与编码预算。
use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use chrono::Utc;
use serde::Serialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::build_source::build_source;
use crate::file_statistics::{increment, FileStatistics};
use crate::local_file::{open_managed_file, FileQueue, HealthHooks, RecordCounters, RecordStore};
use crate::run_storage::{
    collect_old_runs, create_private_directory, open_cleanup_lock, open_lease, write_atomically,
};

pub(crate) const RECORD_BYTES: usize = 64 * 1024;
const QUEUE_RECORDS: usize = 8;
const FILE_BYTES: u64 = 4 * 1024 * 1024;
const RUN_BYTES: u64 = 16 * 1024 * 1024;
const ROOT_BYTES: u64 = 100_000_000;
const RETENTION: Duration = Duration::from_secs(7 * 86400);
pub(crate) const METRICS: usize = 100;
pub(crate) const TRACES: usize = 101;

#[derive(Default)]
pub(crate) struct StreamCounters {
    pub(crate) files: Arc<FileStatistics>,
    accepted: AtomicU64,
    written: AtomicU64,
    failed: AtomicU64,
    full: AtomicU64,
    closed: AtomicU64,
    busy: AtomicU64,
    oversized: AtomicU64,
    queued_bytes: AtomicU64,
    high_water: AtomicU64,
    terminated: AtomicU64,
    rotated: AtomicU64,
    rotated_bytes: AtomicU64,
    cleanup_failed: AtomicU64,
}
impl RecordCounters for StreamCounters {
    fn accepted(&self, source: usize) {
        increment(&self.accepted);
        self.files.accepted(source);
    }
    fn queue_dropped(&self, source: usize) {
        increment(&self.full);
        self.files.queue_dropped(source);
    }
    fn closed_dropped(&self, source: usize) {
        increment(&self.closed);
        self.files.queue_dropped(source);
    }
    fn write_failed(&self, source: usize) {
        increment(&self.failed);
        self.files.write_failed(source);
    }
    fn queued(&self, bytes: usize) -> bool {
        let Ok(previous) =
            self.queued_bytes
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                    (current + bytes as u64 <= (QUEUE_RECORDS * RECORD_BYTES) as u64)
                        .then_some(current + bytes as u64)
                })
        else {
            return false;
        };
        self.high_water
            .fetch_max(previous + bytes as u64, Ordering::Relaxed);
        true
    }
    fn dequeued(&self, bytes: usize) {
        self.queued_bytes.fetch_sub(bytes as u64, Ordering::Relaxed);
    }
    fn worker_failed(&self) {
        increment(&self.terminated);
    }
}

pub(crate) struct DiagnosticWriter {
    pub(crate) run_id: Uuid,
    pub(crate) counters: Arc<StreamCounters>,
    pub(crate) queue: FileQueue,
    encoding: Mutex<()>,
}
impl DiagnosticWriter {
    pub(crate) fn new(root: &Path) -> io::Result<Self> {
        let run_id = Uuid::new_v4();
        let counters = Arc::new(StreamCounters::default());
        let store = DiagnosticStore {
            root: root.join("diagnostics"),
            run_id,
            counters: counters.clone(),
            open: None,
            sequence: 0,
            parked: false,
        };
        let queue = FileQueue::spawn(
            store,
            counters.clone(),
            HealthHooks::ENGINE,
            "uc-diagnostic-file",
            QUEUE_RECORDS,
        )?;
        Ok(Self {
            run_id,
            counters,
            queue,
            encoding: Mutex::new(()),
        })
    }
    pub(crate) fn encoding_slot(&self) -> Option<MutexGuard<'_, ()>> {
        match self.encoding.try_lock() {
            Ok(slot) => Some(slot),
            Err(_) => {
                increment(&self.counters.busy);
                None
            }
        }
    }
    pub(crate) fn record(&self, source: usize, value: &impl Serialize) {
        let Some(_slot) = self.encoding_slot() else {
            return;
        };
        match encode_record(value, RECORD_BYTES) {
            Some(bytes) => self.queue.writer().write_owned(bytes, source),
            None => increment(&self.counters.oversized),
        }
    }
    pub(crate) fn summary(&self) -> Value {
        let mut summary = self.counters.summary();
        summary["runId"] = json!(self.run_id);
        summary["unavailable"] = json!(self.queue.unavailable());
        summary
    }
}
impl StreamCounters {
    fn summary(&self) -> Value {
        let c = self;
        let load = |v: &AtomicU64| v.load(Ordering::Relaxed);
        json!({"schemaVersion": 1,  "queueBytes": load(&c.queued_bytes),
            "queueBytesHighWater": load(&c.high_water), "queueByteBudget": QUEUE_RECORDS * RECORD_BYTES,
            "recordByteLimit": RECORD_BYTES, "accepted": load(&c.accepted), "written": load(&c.written),
            "writeFailed": load(&c.failed), "queueFull": load(&c.full), "closedDropped": load(&c.closed),
            "encodingBusy": load(&c.busy), "encodingRejected": load(&c.oversized), "workerTerminated": load(&c.terminated),
            "rotatedFiles": load(&c.rotated), "rotatedBytes": load(&c.rotated_bytes), "cleanupFailures": load(&c.cleanup_failed),
             "countsComplete": true, "updatedAt": Utc::now().to_rfc3339()})
    }
}
pub(crate) fn encode_record(value: &impl Serialize, limit: usize) -> Option<Vec<u8>> {
    let mut output = LimitedRecord {
        bytes: Vec::new(),
        limit: limit.min(RECORD_BYTES),
    };
    if serde_json::to_writer(&mut output, value).is_err() || output.write_all(b"\n").is_err() {
        return None;
    }
    Some(output.bytes)
}
struct LimitedRecord {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for LimitedRecord {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.bytes.len().saturating_add(bytes.len()) > self.limit {
            return Err(io::Error::other("diagnostic record exceeds byte limit"));
        }
        // Reserve exactly the maximum once. Vec growth cannot exceed the encoding slot budget.
        if self.bytes.capacity() == 0 {
            self.bytes.reserve_exact(self.limit);
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
struct DiagnosticStore {
    root: PathBuf,
    run_id: Uuid,
    counters: Arc<StreamCounters>,
    open: Option<OpenRun>,
    sequence: u64,
    parked: bool,
}
struct OpenRun {
    directory: PathBuf,
    _lease: File,
    files: [Option<(File, String, u64)>; 3],
    segments: VecDeque<(String, u64)>,
    total: u64,
    date: String,
}

fn date() -> String {
    Utc::now().format("%Y%m%d").to_string()
}
impl RecordStore for DiagnosticStore {
    fn prepare(&mut self) -> io::Result<()> {
        if self.parked {
            return Err(io::Error::other("diagnostic writer is parked"));
        }
        if self.open.is_some() {
            return Ok(());
        }
        let _cleanup = open_cleanup_lock(&self.root)?;
        let directory = self.root.join("engine").join(self.run_id.to_string());
        create_private_directory(&directory)?;
        let lease = open_lease(&directory.join("lease.lock"))?;
        if collect_old_runs(&self.root, &directory, RETENTION, ROOT_BYTES).is_err() {
            increment(&self.counters.cleanup_failed);
        }
        let source = build_source();
        let manifest = serde_json::to_vec(&json!({"schemaVersion": 1, "runId": self.run_id,
            "startedAt": Utc::now().to_rfc3339(), "engineSourceCommit": source.commit, "engineSourceState": source.state,
            "budget": {"fileBytes": FILE_BYTES, "runBytes": RUN_BYTES, "rootTargetBytes": ROOT_BYTES,
                "retentionSeconds": RETENTION.as_secs(), "queueBytes": QUEUE_RECORDS * RECORD_BYTES, "recordBytes": RECORD_BYTES}}))?;
        write_atomically(&directory.join("run.json"), &manifest)?;
        let mut segments = VecDeque::new();
        let mut total = 0_u64;
        for entry in std::fs::read_dir(&directory)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.ends_with(".jsonl") || !entry.file_type()?.is_file() {
                continue;
            }
            if let Some(sequence) = name
                .strip_suffix(".jsonl")
                .and_then(|n| n.rsplit('.').next())
                .and_then(|n| n.parse::<u64>().ok())
            {
                self.sequence = self.sequence.max(sequence + 1);
                let size = entry.metadata()?.len();
                total = total.saturating_add(size);
                segments.push_back((name, size));
            }
        }
        segments
            .make_contiguous()
            .sort_by_key(|(name, _)| name.rsplit('.').nth(1).unwrap_or_default().to_owned());
        self.open = Some(OpenRun {
            directory,
            _lease: lease,
            files: std::array::from_fn(|_| None),
            segments,
            total,
            date: date(),
        });
        Ok(())
    }
    fn store_record(&mut self, bytes: &[u8], source: usize) -> io::Result<()> {
        self.prepare()?;
        let run = self
            .open
            .as_mut()
            .ok_or_else(|| io::Error::other("diagnostic run unavailable"))?;
        let index = match source {
            METRICS => 0,
            TRACES => 2,
            _ => 1,
        };
        let signal = ["metrics", "logs", "traces"][index];
        let today = date();
        if run.date != today {
            run.files = std::array::from_fn(|_| None);
            run.date = today;
        }
        if run.files[index]
            .as_ref()
            .is_some_and(|(_, _, size)| size + bytes.len() as u64 > FILE_BYTES)
        {
            run.files[index] = None;
        }
        if run.files[index].is_none() {
            let name = format!("{signal}.{}.{:010}.jsonl", run.date, self.sequence);
            self.sequence += 1;
            let file = open_managed_file(&run.directory.join(&name))?;
            run.segments.push_back((name.clone(), 0));
            run.files[index] = Some((file, name, 0));
        }
        while run.total + bytes.len() as u64 > RUN_BYTES || run.segments.len() > 64 {
            let position = run.segments.iter().position(|(name, _)| {
                !run.files
                    .iter()
                    .flatten()
                    .any(|(_, active, _)| active == name)
            });
            let Some(position) = position else {
                return Err(io::Error::other("diagnostic run budget exhausted"));
            };
            let (name, size) = &run.segments[position];
            if let Err(error) = std::fs::remove_file(run.directory.join(name)) {
                increment(&self.counters.cleanup_failed);
                return Err(error);
            }
            run.total = run.total.saturating_sub(*size);
            self.counters
                .rotated_bytes
                .fetch_add(*size, Ordering::Relaxed);
            increment(&self.counters.rotated);
            run.segments.remove(position);
        }
        let (file, name, size) = run.files[index]
            .as_mut()
            .ok_or_else(|| io::Error::other("diagnostic segment unavailable"))?;
        if let Err(error) = file.write_all(bytes) {
            // A partial line is never followed by another record in this segment.
            if let Ok(metadata) = file.metadata() {
                let partial = metadata.len().saturating_sub(*size);
                *size = metadata.len();
                run.total = run.total.saturating_add(partial);
                if let Some((_, recorded)) = run.segments.iter_mut().find(|(n, _)| n == name) {
                    *recorded = *size;
                }
            }
            run.files[index] = None;
            return Err(error);
        }
        *size += bytes.len() as u64;
        run.total += bytes.len() as u64;
        if let Some((_, recorded)) = run.segments.iter_mut().find(|(n, _)| n == name) {
            *recorded = *size;
        }
        increment(&self.counters.written);
        let counts = self.counters.files.source_at(source);
        increment(&counts.written);
        counts.last_written_at_ms.store(
            Utc::now().timestamp_millis().max(0) as u64,
            Ordering::Relaxed,
        );
        if source == METRICS {
            let mut status = self.counters.summary();
            status["countsComplete"] = json!(false);
            if write_atomically(
                &run.directory.join("status.json"),
                &serde_json::to_vec(&status)?,
            )
            .is_err()
            {
                increment(&self.counters.cleanup_failed);
            }
        }
        Ok(())
    }
    fn settle(&mut self) -> io::Result<()> {
        if let Some(run) = &mut self.open {
            for (file, _, _) in run.files.iter_mut().flatten() {
                file.flush()?;
            }
            let mut status = self.counters.summary();
            status["countsComplete"] = json!(false);
            write_atomically(
                &run.directory.join("status.json"),
                &serde_json::to_vec(&status)?,
            )?;
        }
        Ok(())
    }
    fn park(&mut self) -> io::Result<()> {
        self.settle()?;
        self.open = None;
        self.parked = true;
        Ok(())
    }
    fn unpark(&mut self) -> io::Result<()> {
        self.parked = false;
        if let Err(error) = self.prepare() {
            self.parked = true;
            return Err(error);
        }
        Ok(())
    }
}

impl Drop for DiagnosticStore {
    fn drop(&mut self) {
        if let Some(run) = &self.open {
            let mut status = self.counters.summary();
            status["closed"] = json!(!std::thread::panicking());
            status["countsComplete"] = json!(
                !std::thread::panicking()
                    && self.counters.terminated.load(Ordering::Relaxed) == 0
                    && self.counters.queued_bytes.load(Ordering::Relaxed) == 0
            );
            if serde_json::to_vec(&status).ok().is_none_or(|bytes| {
                write_atomically(&run.directory.join("status.json"), &bytes).is_err()
            }) {
                increment(&self.counters.cleanup_failed);
            }
        }
    }
}
