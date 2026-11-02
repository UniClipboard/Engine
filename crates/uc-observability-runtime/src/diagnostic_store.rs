//! Segmented storage and encoding budgets shared by all three diagnostic signals.
use std::borrow::Cow;
use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use chrono::Utc;
use serde::Serialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::build_source::build_source;
use crate::file_statistics::{increment, FileStatistics};
use crate::local_file::{
    open_managed_file, same_file, FileQueue, HealthHooks, RecordCounters, RecordStore,
};
#[cfg(feature = "test-process-release")]
use crate::local_file::{
    PROBE_DAILY_PARK_BLOCK, PROBE_DAILY_PARK_ENTERED, PROBE_DAILY_PARK_FAILURE,
    PROBE_DAILY_PARK_PANIC,
};
use crate::run_storage::{
    collect_old_runs, create_private_directory, open_cleanup_lock, open_lease, write_atomically,
};

pub(crate) const RECORD_BYTES: usize = 64 * 1024;
#[cfg(feature = "test-process-release")]
pub(crate) static PROBE_FAULT: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
const QUEUE_RECORDS: usize = 256;
const QUEUE_BYTES: usize = 512 * 1024;
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
    written_bytes: AtomicU64,
    submission_busy: AtomicU64,
    failed: AtomicU64,
    full: AtomicU64,
    closed: AtomicU64,
    oversized: AtomicU64,
    pub(crate) schema_rejected: AtomicU64,
    queued_bytes: AtomicU64,
    high_water: AtomicU64,
    terminated: AtomicU64,
    rotated: AtomicU64,
    rotated_bytes: AtomicU64,
    cleanup_failed: AtomicU64,
    metadata_failed: AtomicU64,
}
impl RecordCounters for StreamCounters {
    fn accepted(&self, source: usize) {
        increment(&self.accepted);
        if !matches!(source, METRICS | TRACES) {
            self.files.accepted(source);
        }
    }
    fn queue_dropped(&self, source: usize) {
        increment(&self.full);
        if !matches!(source, METRICS | TRACES) {
            self.files.queue_dropped(source);
        }
    }
    fn closed_dropped(&self, source: usize) {
        increment(&self.closed);
        if !matches!(source, METRICS | TRACES) {
            self.files.queue_dropped(source);
        }
    }
    fn write_failed(&self, source: usize) {
        increment(&self.failed);
        if !matches!(source, METRICS | TRACES) {
            self.files.write_failed(source);
        }
    }
    fn queued(&self, bytes: usize) -> bool {
        let Ok(previous) =
            self.queued_bytes
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                    (current + bytes as u64 <= (QUEUE_BYTES) as u64)
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
    fn submission_dropped(&self, source: usize) {
        increment(&self.submission_busy);
        if !matches!(source, METRICS | TRACES) {
            self.files.queue_dropped(source);
        }
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
            last_status: None,
            started_at: Utc::now().to_rfc3339(),
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
        // This gate covers bounded encoding and nonblocking submission only. Disk I/O stays on the worker.
        Some(
            self.encoding
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    pub(crate) fn record(&self, source: usize, value: &impl Serialize) {
        let Some(_slot) = self.encoding_slot() else {
            return;
        };
        match encode_record(value, RECORD_BYTES) {
            Some(bytes) => self
                .queue
                .writer()
                .write_record(Cow::Borrowed(&bytes), source, false),
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
        json!({"schemaVersion": 1,  "reservedQueueBytes": load(&c.queued_bytes),
            "reservedQueueBytesHighWater": load(&c.high_water), "queueByteBudget": QUEUE_BYTES, "queueRecordLimit": QUEUE_RECORDS,
            "recordByteLimit": RECORD_BYTES, "accepted": load(&c.accepted), "written": load(&c.written),
            "writtenBytes": load(&c.written_bytes), "submissionBusy": load(&c.submission_busy), "writeFailed": load(&c.failed), "queueFull": load(&c.full), "closedDropped": load(&c.closed),
            "schemaRejected": load(&c.schema_rejected), "encodingRejected": load(&c.oversized), "workerTerminated": load(&c.terminated),
            "rotatedFiles": load(&c.rotated), "rotatedBytes": load(&c.rotated_bytes), "cleanupFailures": load(&c.cleanup_failed), "metadataWriteFailures": load(&c.metadata_failed),
             "countsComplete": load(&c.terminated) == 0, "counterSnapshot": "non_transactional", "updatedAt": Utc::now().to_rfc3339()})
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
        // Reserve only the record limit so Vec growth stays within the encoding budget.
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
    last_status: Option<Instant>,
    started_at: String,
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
            "startedAt": self.started_at, "engineSourceCommit": source.commit, "engineSourceState": source.state,
            "budget": {"fileBytes": FILE_BYTES, "runBytes": RUN_BYTES, "rootTargetBytes": ROOT_BYTES,
                "retentionSeconds": RETENTION.as_secs(), "queueBytes": QUEUE_BYTES, "queueRecords": QUEUE_RECORDS, "recordBytes": RECORD_BYTES}}))?;
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
        let intact = self.open.as_ref().is_none_or(|run| {
            run.directory.is_dir()
                && run.files.iter().flatten().all(|(file, name, _)| {
                    std::fs::symlink_metadata(run.directory.join(name))
                        .is_ok_and(|m| same_file(&m, file))
                })
        });
        if !intact {
            self.open = None;
        }
        self.prepare()?;
        #[cfg(feature = "test-process-release")]
        let fault = PROBE_FAULT.swap(0, Ordering::SeqCst);
        #[cfg(feature = "test-process-release")]
        match fault {
            1 => std::thread::sleep(Duration::from_millis(500)),
            2 => {
                return Err(io::Error::new(
                    io::ErrorKind::StorageFull,
                    "controlled disk full",
                ))
            }
            3 => {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "controlled write denial",
                ))
            }
            4 => panic!("controlled diagnostic writer panic"),
            _ => {}
        }
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
        #[cfg(feature = "test-process-release")]
        let result = if fault == 5 {
            file.write_all(&bytes[..bytes.len() / 2])
                .and_then(|_| Err(io::Error::other("controlled partial write")))
        } else {
            file.write_all(bytes)
        };
        #[cfg(not(feature = "test-process-release"))]
        let result = file.write_all(bytes);
        if let Err(error) = result {
            // Close a partial segment. Write subsequent records to a new segment.
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
        self.counters
            .written_bytes
            .fetch_add(bytes.len() as u64, Ordering::Relaxed);
        if !matches!(source, METRICS | TRACES) {
            let counts = self.counters.files.source_at(source);
            increment(&counts.written);
            counts.last_written_at_ms.store(
                Utc::now().timestamp_millis().max(0) as u64,
                Ordering::Relaxed,
            );
        }
        if source == METRICS
            && self
                .last_status
                .is_none_or(|last| last.elapsed() >= Duration::from_secs(5))
        {
            self.last_status = Some(Instant::now());
            let mut status = self.counters.summary();
            status["countsComplete"] = json!(false);
            if write_atomically(
                &run.directory.join("status.json"),
                &serde_json::to_vec(&status)?,
            )
            .is_err()
            {
                increment(&self.counters.metadata_failed);
            }
        }
        Ok(())
    }
    fn settle(&mut self) -> io::Result<()> {
        if self.open.as_ref().is_some_and(|run| {
            !run.directory.is_dir()
                || run.files.iter().flatten().any(|(file, name, _)| {
                    !std::fs::symlink_metadata(run.directory.join(name))
                        .is_ok_and(|metadata| same_file(&metadata, file))
                })
        }) {
            self.open = None;
        }
        if !self.parked {
            self.prepare()?;
        }
        #[cfg(feature = "test-process-release")]
        if PROBE_FAULT
            .compare_exchange(6, 0, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            std::thread::sleep(Duration::from_millis(500));
        }
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
        #[cfg(feature = "test-process-release")]
        if PROBE_DAILY_PARK_PANIC.swap(false, Ordering::SeqCst) {
            panic!("controlled daily park worker panic");
        }
        #[cfg(feature = "test-process-release")]
        if PROBE_DAILY_PARK_FAILURE.swap(false, Ordering::SeqCst) {
            #[cfg(unix)]
            let source = io::Error::new(io::ErrorKind::PermissionDenied, "controlled write denial");
            #[cfg(not(unix))]
            let source = io::Error::new(io::ErrorKind::PermissionDenied, "controlled park failure");
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, source));
        }
        #[cfg(feature = "test-process-release")]
        if PROBE_DAILY_PARK_BLOCK.load(Ordering::SeqCst) {
            PROBE_DAILY_PARK_ENTERED.store(true, Ordering::SeqCst);
            let deadline = Instant::now() + Duration::from_secs(20);
            while PROBE_DAILY_PARK_BLOCK.load(Ordering::SeqCst) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
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
                increment(&self.counters.metadata_failed);
            }
        }
    }
}
