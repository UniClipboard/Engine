use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex, MutexGuard, TryLockError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use chrono::{Days, NaiveDate, Utc};
use tracing_subscriber::fmt::MakeWriter;
use uc_observability_contract::diagnostics::{managed_log_file_date, managed_log_file_name};

use crate::{LOCAL_LOG_MAX_BYTES, LOCAL_LOG_RETENTION_DAYS};

const ASYNC_QUEUE_CAPACITY: usize = 4_096;

#[derive(Clone)]
pub(crate) struct BoundedDailyMakeWriter {
    state: Arc<Mutex<WriterState>>,
    dropped_records: Arc<AtomicU64>,
}

impl BoundedDailyMakeWriter {
    fn new_with_limits(
        directory: &Path,
        retention_days: u64,
        max_bytes: u64,
    ) -> io::Result<(Self, Arc<AtomicU64>)> {
        std::fs::create_dir_all(directory)?;
        let today = Utc::now().date_naive();
        cleanup(directory, today, retention_days, max_bytes)?;
        let state = WriterState::open(directory.to_path_buf(), today, retention_days, max_bytes)?;
        let dropped_records = Arc::new(AtomicU64::new(0));
        Ok((
            Self {
                state: Arc::new(Mutex::new(state)),
                dropped_records: Arc::clone(&dropped_records),
            },
            dropped_records,
        ))
    }
}

impl<'writer> MakeWriter<'writer> for BoundedDailyMakeWriter {
    type Writer = BoundedDailyWriter;

    fn make_writer(&'writer self) -> Self::Writer {
        BoundedDailyWriter {
            owner: self.clone(),
        }
    }
}

impl Write for BoundedDailyMakeWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        BoundedDailyWriter {
            owner: self.clone(),
        }
        .write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        BoundedDailyWriter {
            owner: self.clone(),
        }
        .flush()
    }
}

pub(crate) struct BoundedDailyWriter {
    owner: BoundedDailyMakeWriter,
}

#[derive(Clone)]
pub(crate) struct AsyncFileWriter {
    sender: mpsc::SyncSender<FileMessage>,
    dropped_records: Arc<AtomicU64>,
    closed: Arc<AtomicBool>,
    submission: Arc<Mutex<()>>,
}

pub(crate) struct LocalFileRuntime {
    writer: AsyncFileWriter,
    worker: Arc<LocalFileWorker>,
    dropped_records: Arc<AtomicU64>,
}

struct LocalFileWorker {
    sender: mpsc::SyncSender<FileMessage>,
    thread: Arc<Mutex<Option<JoinHandle<()>>>>,
    closed: Arc<AtomicBool>,
    submission: Arc<Mutex<()>>,
    shutdown: Arc<FileShutdownCoordinator>,
}

struct FileShutdownCoordinator {
    phase: Mutex<FileShutdownPhase>,
    changed: Condvar,
}

#[derive(Clone, Copy)]
enum FileShutdownPhase {
    Running,
    InProgress,
    Completed { result: bool, retryable: bool },
}

enum FileShutdownStart {
    Started,
    Waiting,
    Completed(bool),
}

impl Default for FileShutdownCoordinator {
    fn default() -> Self {
        Self {
            phase: Mutex::new(FileShutdownPhase::Running),
            changed: Condvar::new(),
        }
    }
}

enum FileMessage {
    Line(Vec<u8>),
    Flush(mpsc::Sender<bool>),
    Shutdown(mpsc::Sender<bool>),
}

impl LocalFileRuntime {
    pub(crate) fn new(directory: &Path) -> io::Result<Self> {
        Self::new_internal(directory, LOCAL_LOG_RETENTION_DAYS, LOCAL_LOG_MAX_BYTES)
    }

    #[cfg(test)]
    fn new_with_limits(directory: &Path, retention_days: u64, max_bytes: u64) -> io::Result<Self> {
        Self::new_internal(directory, retention_days, max_bytes)
    }

    fn new_internal(directory: &Path, retention_days: u64, max_bytes: u64) -> io::Result<Self> {
        let (writer, dropped_records) =
            BoundedDailyMakeWriter::new_with_limits(directory, retention_days, max_bytes)?;
        let (writer, worker) = non_blocking_file_writer(writer, Arc::clone(&dropped_records))?;
        Ok(Self {
            writer,
            worker,
            dropped_records,
        })
    }

    pub(crate) fn writer(&self) -> AsyncFileWriter {
        self.writer.clone()
    }

    pub(crate) fn dropped_records(&self) -> u64 {
        self.dropped_records.load(Ordering::Relaxed)
    }

    pub(crate) fn flush(&self, deadline: Duration) -> bool {
        self.worker.flush(deadline)
    }

    pub(crate) fn shutdown(&self, deadline: Duration) -> bool {
        self.worker.shutdown(deadline)
    }

    pub(crate) fn shutdown_cleanup_incomplete(&self) -> bool {
        self.worker.shutdown_cleanup_incomplete()
    }
}

fn non_blocking_file_writer(
    writer: BoundedDailyMakeWriter,
    dropped_records: Arc<AtomicU64>,
) -> io::Result<(AsyncFileWriter, Arc<LocalFileWorker>)> {
    let (sender, receiver) = mpsc::sync_channel(ASYNC_QUEUE_CAPACITY);
    let closed = Arc::new(AtomicBool::new(false));
    let submission = Arc::new(Mutex::new(()));
    let worker_dropped_records = Arc::clone(&dropped_records);
    let thread = std::thread::Builder::new()
        .name("uc-observability-file".to_owned())
        .spawn(move || run_file_worker(writer, receiver, worker_dropped_records))?;
    Ok((
        AsyncFileWriter {
            sender: sender.clone(),
            dropped_records,
            closed: Arc::clone(&closed),
            submission: Arc::clone(&submission),
        },
        Arc::new(LocalFileWorker {
            sender,
            thread: Arc::new(Mutex::new(Some(thread))),
            closed,
            submission,
            shutdown: Arc::new(FileShutdownCoordinator::default()),
        }),
    ))
}

impl Write for AsyncFileWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let _submission = match self.submission.try_lock() {
            Ok(submission) => submission,
            Err(TryLockError::WouldBlock) => {
                self.dropped_records.fetch_add(1, Ordering::Relaxed);
                return Ok(buffer.len());
            }
            Err(TryLockError::Poisoned(error)) => error.into_inner(),
        };
        if self.closed.load(Ordering::Acquire)
            || self
                .sender
                .try_send(FileMessage::Line(buffer.to_vec()))
                .is_err()
        {
            self.dropped_records.fetch_add(1, Ordering::Relaxed);
        }
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'writer> MakeWriter<'writer> for AsyncFileWriter {
    type Writer = Self;

    fn make_writer(&'writer self) -> Self::Writer {
        self.clone()
    }
}

impl LocalFileWorker {
    fn seal(&self) {
        let _submission = lock(&self.submission);
        self.closed.store(true, Ordering::Release);
    }

    pub(crate) fn flush(&self, deadline: Duration) -> bool {
        let started = Instant::now();
        let (sender, receiver) = mpsc::channel();
        let mut message = FileMessage::Flush(sender);
        {
            let Some(_submission) = lock_before_deadline(&self.submission, started, deadline)
            else {
                return false;
            };
            if started.elapsed() >= deadline {
                return false;
            }
            if self.closed.load(Ordering::Acquire) {
                return false;
            }
            loop {
                match self.sender.try_send(message) {
                    Ok(()) => break,
                    Err(mpsc::TrySendError::Full(returned)) => {
                        if started.elapsed() >= deadline {
                            return false;
                        }
                        message = returned;
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(mpsc::TrySendError::Disconnected(_)) => return false,
                }
            }
        }
        receiver
            .recv_timeout(deadline.saturating_sub(started.elapsed()))
            .unwrap_or(false)
    }

    pub(crate) fn shutdown(&self, deadline: Duration) -> bool {
        match self.shutdown.begin() {
            FileShutdownStart::Completed(result) => return result,
            FileShutdownStart::Waiting => return self.shutdown.wait(deadline).unwrap_or(false),
            FileShutdownStart::Started => {}
        }
        self.seal();
        let sender = self.sender.clone();
        let thread = Arc::clone(&self.thread);
        let shutdown = Arc::clone(&self.shutdown);
        let worker_shutdown = Arc::clone(&shutdown);
        if std::thread::Builder::new()
            .name("uc-observability-file-shutdown".to_owned())
            .spawn(move || {
                let worker = lock(thread.as_ref()).take();
                let completed = match worker {
                    Some(worker) => {
                        let (reply, response) = mpsc::channel();
                        let worker_replied = sender
                            .send(FileMessage::Shutdown(reply))
                            .ok()
                            .and_then(|()| response.recv().ok())
                            .unwrap_or(false);
                        let joined = worker.join().is_ok();
                        worker_replied && joined
                    }
                    None => true,
                };
                worker_shutdown.complete(completed, false);
            })
            .is_err()
        {
            shutdown.complete(false, true);
        }
        self.shutdown.wait(deadline).unwrap_or(false)
    }

    fn shutdown_cleanup_incomplete(&self) -> bool {
        self.shutdown.cleanup_incomplete()
    }
}

impl FileShutdownCoordinator {
    fn begin(&self) -> FileShutdownStart {
        let mut phase = lock(&self.phase);
        match *phase {
            FileShutdownPhase::Running => {
                *phase = FileShutdownPhase::InProgress;
                FileShutdownStart::Started
            }
            FileShutdownPhase::InProgress => FileShutdownStart::Waiting,
            FileShutdownPhase::Completed {
                result: false,
                retryable: true,
            } => {
                *phase = FileShutdownPhase::InProgress;
                FileShutdownStart::Started
            }
            FileShutdownPhase::Completed { result, .. } => FileShutdownStart::Completed(result),
        }
    }

    fn complete(&self, result: bool, retryable: bool) {
        *lock(&self.phase) = FileShutdownPhase::Completed { result, retryable };
        self.changed.notify_all();
    }

    fn cleanup_incomplete(&self) -> bool {
        matches!(
            *lock(&self.phase),
            FileShutdownPhase::Running
                | FileShutdownPhase::InProgress
                | FileShutdownPhase::Completed {
                    retryable: true,
                    ..
                }
        )
    }

    fn wait(&self, deadline: Duration) -> Option<bool> {
        let started = Instant::now();
        let mut phase = lock(&self.phase);
        loop {
            if let FileShutdownPhase::Completed { result, .. } = *phase {
                return Some(result);
            }
            let remaining = deadline.checked_sub(started.elapsed())?;
            let (next, timeout) = self
                .changed
                .wait_timeout(phase, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            phase = next;
            if timeout.timed_out() && !matches!(*phase, FileShutdownPhase::Completed { .. }) {
                return None;
            }
        }
    }
}

fn run_file_worker(
    mut writer: BoundedDailyMakeWriter,
    receiver: mpsc::Receiver<FileMessage>,
    dropped_records: Arc<AtomicU64>,
) {
    let mut healthy = true;
    while let Ok(message) = receiver.recv() {
        match message {
            FileMessage::Line(bytes) => {
                if !healthy {
                    dropped_records.fetch_add(1, Ordering::Relaxed);
                } else if writer.write_all(&bytes).is_err() {
                    dropped_records.fetch_add(1, Ordering::Relaxed);
                    healthy = false;
                }
            }
            FileMessage::Flush(reply) => {
                if healthy && writer.flush().is_err() {
                    healthy = false;
                }
                let _ = reply.send(healthy);
            }
            FileMessage::Shutdown(reply) => {
                if healthy && writer.flush().is_err() {
                    healthy = false;
                }
                let _ = reply.send(healthy);
                return;
            }
        }
    }
}

impl Write for BoundedDailyWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let mut state = lock(&self.owner.state);
        state.rotate_if_needed()?;
        let additional = u64::try_from(buffer.len()).unwrap_or(u64::MAX);
        if state.total_bytes.saturating_add(additional) > state.max_bytes {
            self.owner.dropped_records.fetch_add(1, Ordering::Relaxed);
            return Ok(buffer.len());
        }
        state.file.write_all(buffer)?;
        state.total_bytes = state.total_bytes.saturating_add(additional);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        lock(&self.owner.state).file.flush()
    }
}

struct WriterState {
    directory: PathBuf,
    date: NaiveDate,
    file: File,
    total_bytes: u64,
    retention_days: u64,
    max_bytes: u64,
}

impl WriterState {
    fn open(
        directory: PathBuf,
        date: NaiveDate,
        retention_days: u64,
        max_bytes: u64,
    ) -> io::Result<Self> {
        let path = directory.join(file_name(date));
        let file = open_managed_file(&path)?;
        let total_bytes =
            managed_files(&directory)?
                .into_iter()
                .try_fold(0_u64, |total, entry| {
                    entry
                        .path
                        .metadata()
                        .map(|metadata| total.saturating_add(metadata.len()))
                })?;
        Ok(Self {
            directory,
            date,
            file,
            total_bytes,
            retention_days,
            max_bytes,
        })
    }

    fn rotate_if_needed(&mut self) -> io::Result<()> {
        let today = Utc::now().date_naive();
        if today == self.date {
            return Ok(());
        }
        cleanup(&self.directory, today, self.retention_days, self.max_bytes)?;
        *self = Self::open(
            self.directory.clone(),
            today,
            self.retention_days,
            self.max_bytes,
        )?;
        Ok(())
    }
}

fn open_managed_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    prevent_symlink_follow(&mut options);
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "managed log target is not a regular file",
        ));
    }
    Ok(file)
}

#[cfg(unix)]
fn prevent_symlink_follow(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;

    options.custom_flags(libc::O_NOFOLLOW);
}

#[cfg(windows)]
fn prevent_symlink_follow(options: &mut OpenOptions) {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
}

#[cfg(not(any(unix, windows)))]
fn prevent_symlink_follow(_options: &mut OpenOptions) {}

#[derive(Debug)]
struct ManagedFile {
    date: NaiveDate,
    path: PathBuf,
}

fn managed_files(directory: &Path) -> io::Result<Vec<ManagedFile>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some(date) = managed_log_file_date(&name) else {
            continue;
        };
        files.push(ManagedFile {
            date,
            path: entry.path(),
        });
    }
    files.sort_by_key(|entry| entry.date);
    Ok(files)
}

fn cleanup(
    directory: &Path,
    today: NaiveDate,
    retention_days: u64,
    max_bytes: u64,
) -> io::Result<()> {
    let oldest = today
        .checked_sub_days(Days::new(retention_days.saturating_sub(1)))
        .unwrap_or(NaiveDate::MIN);
    for entry in managed_files(directory)? {
        if entry.date < oldest {
            std::fs::remove_file(entry.path)?;
        }
    }

    let files = managed_files(directory)?;
    let mut total = files.iter().try_fold(0_u64, |total, entry| {
        entry
            .path
            .metadata()
            .map(|metadata| total.saturating_add(metadata.len()))
    })?;
    for entry in files {
        if total <= max_bytes {
            break;
        }
        let size = entry.path.metadata()?.len();
        std::fs::remove_file(entry.path)?;
        total = total.saturating_sub(size);
    }
    Ok(())
}

pub fn managed_log_files(directory: &Path) -> io::Result<Vec<PathBuf>> {
    managed_files(directory).map(|files| files.into_iter().map(|entry| entry.path).collect())
}

fn file_name(date: NaiveDate) -> String {
    managed_log_file_name(date)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn lock_before_deadline<T>(
    mutex: &Mutex<T>,
    started: Instant,
    deadline: Duration,
) -> Option<MutexGuard<'_, T>> {
    loop {
        match mutex.try_lock() {
            Ok(guard) => return Some(guard),
            Err(TryLockError::Poisoned(error)) => return Some(error.into_inner()),
            Err(TryLockError::WouldBlock) if started.elapsed() >= deadline => return None,
            Err(TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(1)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn cleanup_removes_expired_and_oldest_managed_files_only() {
        let directory = tempdir().expect("temp dir");
        let today = NaiveDate::from_ymd_opt(2026, 9, 4).expect("date");
        std::fs::write(directory.path().join("engine.2026-08-28.jsonl"), b"old").expect("old");
        std::fs::write(directory.path().join("engine.2026-09-01.jsonl"), b"12345").expect("first");
        std::fs::write(directory.path().join("engine.2026-09-02.jsonl"), b"67890").expect("second");
        std::fs::write(directory.path().join("keep.txt"), b"private host file").expect("unmanaged");

        cleanup(directory.path(), today, 7, 5).expect("cleanup");

        assert!(!directory.path().join("engine.2026-08-28.jsonl").exists());
        assert!(!directory.path().join("engine.2026-09-01.jsonl").exists());
        assert!(directory.path().join("engine.2026-09-02.jsonl").exists());
        assert!(directory.path().join("keep.txt").exists());
    }

    #[test]
    fn managed_file_enumeration_ignores_similar_and_nested_names() {
        let directory = tempdir().expect("temp dir");
        std::fs::write(directory.path().join("engine.2026-09-04.jsonl"), b"{}").expect("managed");
        std::fs::write(directory.path().join("engine.latest.jsonl"), b"{}").expect("similar");
        std::fs::create_dir(directory.path().join("engine.2026-09-03.jsonl")).expect("directory");

        let files = managed_log_files(directory.path()).expect("enumeration");
        assert_eq!(
            files,
            vec![directory.path().join("engine.2026-09-04.jsonl")]
        );
    }

    #[cfg(unix)]
    #[test]
    fn writer_refuses_a_managed_name_that_is_a_symbolic_link() {
        let directory = tempdir().expect("temp dir");
        let target = directory.path().join("outside.txt");
        std::fs::write(&target, b"outside").expect("outside target");
        let today = Utc::now().date_naive();
        std::os::unix::fs::symlink(&target, directory.path().join(file_name(today)))
            .expect("managed-name symlink");

        assert!(BoundedDailyMakeWriter::new_with_limits(directory.path(), 7, 1_024).is_err());
        assert_eq!(std::fs::read(&target).expect("outside target"), b"outside");
    }

    #[test]
    fn writer_drops_whole_records_before_exceeding_the_total_cap() {
        let directory = tempdir().expect("temp dir");
        let (mut writer, dropped) =
            BoundedDailyMakeWriter::new_with_limits(directory.path(), 7, 10)
                .expect("bounded writer");

        writer.write_all(b"12345678").expect("first record");
        writer
            .write_all(b"abcd")
            .expect("dropped record is non-fatal");
        writer.flush().expect("flush");

        let files = managed_log_files(directory.path()).expect("files");
        assert_eq!(std::fs::metadata(&files[0]).expect("metadata").len(), 8);
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn explicit_flush_waits_until_all_queued_records_are_readable() {
        let directory = tempdir().expect("temp dir");
        let (writer, dropped) = BoundedDailyMakeWriter::new_with_limits(directory.path(), 7, 1_024)
            .expect("bounded writer");
        let dropped_after_shutdown = Arc::clone(&dropped);
        let (mut writer, worker) =
            non_blocking_file_writer(writer, dropped).expect("non-blocking writer");

        writer.write_all(b"queued-record\n").expect("queue record");
        assert!(worker.flush(Duration::from_secs(1)));

        let files = managed_log_files(directory.path()).expect("files");
        let content = std::fs::read_to_string(&files[0]).expect("read flushed file");
        assert_eq!(content, "queued-record\n");
        assert!(worker.shutdown(Duration::from_secs(1)));
        writer
            .write_all(b"after-shutdown\n")
            .expect("closed writer remains non-fatal");
        assert_eq!(dropped_after_shutdown.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn flush_is_ordered_after_a_submission_that_already_holds_the_gate() {
        let directory = tempdir().expect("temp dir");
        let (writer, dropped) = BoundedDailyMakeWriter::new_with_limits(directory.path(), 7, 1_024)
            .expect("bounded writer");
        let (_writer, worker) =
            non_blocking_file_writer(writer, dropped).expect("non-blocking writer");
        let submission = lock(&worker.submission);
        let (started_sender, started_receiver) = mpsc::channel();
        let flush_worker = Arc::clone(&worker);
        let flush = std::thread::spawn(move || {
            started_sender.send(()).expect("flush started");
            flush_worker.flush(Duration::from_secs(1))
        });
        started_receiver.recv().expect("flush start signal");
        worker
            .sender
            .try_send(FileMessage::Line(b"before-flush\n".to_vec()))
            .expect("line accepted before flush");
        drop(submission);

        assert!(flush.join().expect("flush thread"));
        let files = managed_log_files(directory.path()).expect("files");
        assert_eq!(
            std::fs::read_to_string(&files[0]).expect("flushed file"),
            "before-flush\n"
        );
        assert!(worker.shutdown(Duration::from_secs(1)));
    }

    #[test]
    fn concurrent_flush_deadline_includes_waiting_for_the_submission_gate() {
        let (sender, receiver) = mpsc::sync_channel(0);
        let worker = Arc::new(LocalFileWorker {
            sender,
            thread: Arc::new(Mutex::new(None)),
            closed: Arc::new(AtomicBool::new(false)),
            submission: Arc::new(Mutex::new(())),
            shutdown: Arc::new(FileShutdownCoordinator::default()),
        });
        let long_worker = Arc::clone(&worker);
        let long_flush = std::thread::spawn(move || long_worker.flush(Duration::from_millis(100)));
        let wait_deadline = Instant::now() + Duration::from_millis(50);
        loop {
            match worker.submission.try_lock() {
                Err(TryLockError::WouldBlock) => break,
                Ok(guard) => drop(guard),
                Err(TryLockError::Poisoned(error)) => drop(error.into_inner()),
            }
            assert!(
                Instant::now() < wait_deadline,
                "long flush did not acquire gate"
            );
            std::thread::yield_now();
        }

        let started = Instant::now();
        assert!(!worker.flush(Duration::from_millis(5)));
        assert!(started.elapsed() < Duration::from_millis(50));

        assert!(!long_flush.join().expect("long flush thread"));
        drop(receiver);
    }

    #[test]
    fn local_file_runtime_owns_its_complete_lifecycle_and_health() {
        let directory = tempdir().expect("temp dir");
        let runtime =
            LocalFileRuntime::new_with_limits(directory.path(), 7, 10).expect("local file runtime");
        let mut writer = runtime.writer();

        writer.write_all(b"12345678").expect("first record");
        writer
            .write_all(b"abcd")
            .expect("over-limit record is non-fatal");
        assert!(runtime.flush(Duration::from_secs(1)));

        let files = managed_log_files(directory.path()).expect("files");
        assert_eq!(std::fs::read(&files[0]).expect("read file"), b"12345678");
        assert_eq!(runtime.dropped_records(), 1);

        assert!(runtime.shutdown(Duration::from_secs(1)));
        writer
            .write_all(b"after-shutdown")
            .expect("closed writer remains non-fatal");
        assert_eq!(runtime.dropped_records(), 2);
    }

    #[test]
    fn control_request_honors_deadline_when_the_queue_cannot_accept_it() {
        let (sender, receiver) = mpsc::sync_channel(0);
        let worker = Arc::new(LocalFileWorker {
            sender,
            thread: Arc::new(Mutex::new(None)),
            closed: Arc::new(AtomicBool::new(false)),
            submission: Arc::new(Mutex::new(())),
            shutdown: Arc::new(FileShutdownCoordinator::default()),
        });
        let (result_sender, result_receiver) = mpsc::channel();
        let worker_for_request = Arc::clone(&worker);
        let request = std::thread::spawn(move || {
            let result = worker_for_request.flush(Duration::from_millis(5));
            let _ = result_sender.send(result);
        });

        assert_eq!(
            result_receiver.recv_timeout(Duration::from_millis(100)),
            Ok(false)
        );
        drop(receiver);
        request.join().expect("request thread");
    }

    #[test]
    fn timed_out_shutdown_still_queues_and_a_later_call_observes_completion() {
        let (sender, receiver) = mpsc::sync_channel(0);
        let worker = LocalFileWorker {
            sender,
            thread: Arc::new(Mutex::new(Some(std::thread::spawn(|| {})))),
            closed: Arc::new(AtomicBool::new(false)),
            submission: Arc::new(Mutex::new(())),
            shutdown: Arc::new(FileShutdownCoordinator::default()),
        };

        assert!(!worker.shutdown(Duration::from_millis(1)));
        let FileMessage::Shutdown(reply) = receiver.recv().expect("queued shutdown") else {
            panic!("unexpected file worker message");
        };
        reply.send(true).expect("shutdown reply");
        assert!(worker.shutdown(Duration::from_secs(1)));
    }

    #[test]
    fn completed_failed_file_shutdown_remains_failed() {
        let (sender, receiver) = mpsc::sync_channel(0);
        drop(receiver);
        let worker = LocalFileWorker {
            sender,
            thread: Arc::new(Mutex::new(Some(std::thread::spawn(|| {})))),
            closed: Arc::new(AtomicBool::new(false)),
            submission: Arc::new(Mutex::new(())),
            shutdown: Arc::new(FileShutdownCoordinator::default()),
        };

        assert!(!worker.shutdown(Duration::from_secs(1)));
        assert!(!worker.shutdown(Duration::from_secs(1)));
        assert!(!worker.shutdown_cleanup_incomplete());
    }

    #[test]
    fn file_shutdown_retries_only_when_cleanup_did_not_start() {
        let shutdown = FileShutdownCoordinator::default();

        assert!(matches!(shutdown.begin(), FileShutdownStart::Started));
        shutdown.complete(false, true);
        assert!(shutdown.cleanup_incomplete());
        assert!(matches!(shutdown.begin(), FileShutdownStart::Started));

        shutdown.complete(false, false);
        assert!(!shutdown.cleanup_incomplete());
        assert!(matches!(
            shutdown.begin(),
            FileShutdownStart::Completed(false)
        ));
    }

    #[test]
    fn contended_submission_is_dropped_without_blocking_the_writer() {
        let (sender, _receiver) = mpsc::sync_channel(1);
        let dropped_records = Arc::new(AtomicU64::new(0));
        let submission = Arc::new(Mutex::new(()));
        let _held = lock(submission.as_ref());
        let mut writer = AsyncFileWriter {
            sender,
            dropped_records: Arc::clone(&dropped_records),
            closed: Arc::new(AtomicBool::new(false)),
            submission: Arc::clone(&submission),
        };

        let started = Instant::now();
        writer.write_all(b"not-blocked").expect("non-fatal drop");

        assert!(started.elapsed() < Duration::from_millis(50));
        assert_eq!(dropped_records.load(Ordering::Relaxed), 1);
    }
}
