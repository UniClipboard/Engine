use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex, MutexGuard, TryLockError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use chrono::{Days, NaiveDate, Utc};
use tracing_subscriber::fmt::MakeWriter;
use uc_observability_contract::diagnostics::{managed_log_file_date, managed_log_file_name};

use crate::file_statistics::{increment, FileSourceCounts, FileStatistics, LocalDiagnosticSource};
use crate::remote_health::{record_local_file_failed, record_local_file_recovered};
use crate::{SignalResult, LOCAL_LOG_MAX_BYTES, LOCAL_LOG_RETENTION_DAYS};

const ASYNC_QUEUE_CAPACITY: usize = 4_096;
#[cfg(feature = "test-process-release")]
pub(crate) static PROBE_DAILY_PARK_FAILURE: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "test-process-release")]
pub(crate) static PROBE_DAILY_PARK_PANIC: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "test-process-release")]
pub(crate) static PROBE_DAILY_PARK_BLOCK: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "test-process-release")]
pub(crate) static PROBE_DAILY_PARK_ENTERED: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "test-process-release")]
pub(crate) static PROBE_DAILY_EXIT_GATE: Mutex<Option<PathBuf>> = Mutex::new(None);

#[cfg(feature = "test-process-release")]
struct ProbeDailyExitGate;

#[cfg(feature = "test-process-release")]
impl Drop for ProbeDailyExitGate {
    fn drop(&mut self) {
        let gate = lock(&PROBE_DAILY_EXIT_GATE).take();
        if let Some(gate) = gate {
            let _ = std::fs::write(gate.join("worker-exiting"), b"exiting");
            let limit = Instant::now() + Duration::from_secs(20);
            while !gate.join("allow-exit").is_file() && Instant::now() < limit {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

#[derive(Clone)]
pub(crate) struct BoundedDailyMakeWriter {
    state: Arc<Mutex<Option<WriterState>>>,
    directory: PathBuf,
    retention_days: u64,
    max_bytes: u64,
    dropped_records: Arc<AtomicU64>,
    statistics: Arc<FileStatistics>,
    /// 最近一次落盘是否失败；由文件线程在状态变化时更新，`health()` 据此动态报告。
    unavailable: Arc<AtomicBool>,
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
                state: Arc::new(Mutex::new(Some(state))),
                directory: directory.to_path_buf(),
                retention_days,
                max_bytes,
                dropped_records: Arc::clone(&dropped_records),
                statistics: Arc::new(FileStatistics::default()),
                unavailable: Arc::new(AtomicBool::new(false)),
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

/// 文件线程内的落盘策略。Engine 日文件与独立网络日志各实现一份，队列、封口、刷新与恢复共用同一套机制。
///
/// `source` 是调用方定义的来源序号，队列只原样透传给计数器与策略。
pub(crate) trait RecordStore: Send + 'static {
    /// 线程启动后立即调用一次；失败按写入失败处理并进入退避重试，后续写入或刷新会再次尝试。
    fn prepare(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn store_record(&mut self, bytes: &[u8], source: usize) -> io::Result<()>;
    fn settle(&mut self) -> io::Result<()>;

    /// 暂时交还本策略持有的全部文件：先落盘，再关闭。之后不再打开任何文件，直到 [`Self::unpark`]。
    /// 只有持有需交还文件（租约）的策略实现；其余策略没有可交还的内容。
    fn park(&mut self) -> io::Result<()> {
        Ok(())
    }

    /// 结束停车并重新打开；没有停车时什么也不做。
    fn unpark(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// 队列侧的按来源计数；写入策略自己的落盘计数不经过这里。
pub(crate) trait RecordCounters: Send + Sync + 'static {
    fn accepted(&self, source: usize);
    fn queue_dropped(&self, source: usize);
    /// 队列已封口后到达的记录；没有单独统计的计数器把它并入队列丢弃。
    fn closed_dropped(&self, source: usize) {
        self.queue_dropped(source);
    }
    fn write_failed(&self, source: usize);
}

/// 文件线程在可用性变化时的通知；Engine 日文件写固定健康记录，网络日志只更新自己的状态。
#[derive(Clone, Copy)]
pub(crate) struct HealthHooks {
    pub(crate) failed: fn(&'static str),
    pub(crate) recovered: fn(),
}

impl HealthHooks {
    pub(crate) const ENGINE: Self = Self {
        failed: record_local_file_failed,
        recovered: record_local_file_recovered,
    };
}

impl RecordCounters for FileStatistics {
    fn accepted(&self, source: usize) {
        increment(&self.source_at(source).accepted);
    }

    fn queue_dropped(&self, source: usize) {
        increment(&self.source_at(source).queue_dropped);
    }

    fn write_failed(&self, source: usize) {
        increment(&self.source_at(source).write_failed);
    }
}

impl RecordStore for BoundedDailyMakeWriter {
    fn store_record(&mut self, bytes: &[u8], source: usize) -> io::Result<()> {
        BoundedDailyMakeWriter::write_record(
            self,
            bytes,
            LocalDiagnosticSource::ALL
                .get(source)
                .copied()
                .unwrap_or(LocalDiagnosticSource::Runtime),
        )
    }

    fn settle(&mut self) -> io::Result<()> {
        Write::flush(self)
    }

    fn park(&mut self) -> io::Result<()> {
        #[cfg(feature = "test-process-release")]
        if PROBE_DAILY_PARK_PANIC.swap(false, Ordering::SeqCst) {
            panic!("controlled daily park worker panic");
        }
        #[cfg(feature = "test-process-release")]
        if PROBE_DAILY_PARK_FAILURE.swap(false, Ordering::SeqCst) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                io::Error::from_raw_os_error(libc::EACCES),
            ));
        }
        #[cfg(feature = "test-process-release")]
        if PROBE_DAILY_PARK_BLOCK.load(Ordering::SeqCst) {
            PROBE_DAILY_PARK_ENTERED.store(true, Ordering::SeqCst);
            let deadline = Instant::now() + Duration::from_secs(20);
            while PROBE_DAILY_PARK_BLOCK.load(Ordering::SeqCst) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        let mut state = lock(&self.state);
        if let Some(open) = state.as_mut() {
            open.file.flush()?;
        }
        *state = None;
        Ok(())
    }

    fn unpark(&mut self) -> io::Result<()> {
        let mut state = lock(&self.state);
        if state.is_none() {
            *state = Some(WriterState::open(
                self.directory.clone(),
                Utc::now().date_naive(),
                self.retention_days,
                self.max_bytes,
            )?);
        }
        Ok(())
    }
}

#[derive(Clone)]
pub(crate) struct AsyncFileWriter {
    sender: mpsc::SyncSender<FileMessage>,
    dropped_records: Arc<AtomicU64>,
    closed: Arc<AtomicBool>,
    /// 停车期间到达的记录与封口后一样直接丢弃，不进入队列，也不让文件线程重新打开文件。
    parked: Arc<AtomicBool>,
    submission: Arc<Mutex<()>>,
    counters: Arc<dyn RecordCounters>,
}

/// 一个独立的有界文件队列：写入句柄、工作线程与可用性状态。
pub(crate) struct FileQueue {
    writer: AsyncFileWriter,
    worker: Arc<LocalFileWorker>,
    dropped_records: Arc<AtomicU64>,
    unavailable: Arc<AtomicBool>,
}

pub(crate) struct LocalFileRuntime {
    queue: FileQueue,
    statistics: Arc<FileStatistics>,
}

struct LocalFileWorker {
    transition: Mutex<()>,
    sender: mpsc::SyncSender<FileMessage>,
    thread: Arc<Mutex<Option<JoinHandle<()>>>>,
    closed: Arc<AtomicBool>,
    parked: Arc<AtomicBool>,
    submission: Arc<Mutex<()>>,
    transition_id: Arc<Mutex<Arc<()>>>,
    shutdown: Arc<FileShutdownCoordinator>,
    park_error: Arc<Mutex<Option<Arc<io::Error>>>>,
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
    Line(Vec<u8>, usize),
    Flush(mpsc::Sender<bool>),
    Park(mpsc::Sender<bool>, ParkCompletion),
    Unpark(mpsc::Sender<bool>, Box<dyn FnOnce(bool) + Send>),
    Shutdown(mpsc::Sender<bool>),
}

struct ParkCompletion {
    observer: Option<Box<dyn FnOnce(bool) + Send>>,
    enqueued: bool,
}

impl ParkCompletion {
    fn finish(mut self, success: bool) {
        if let Some(observer) = self.observer.take() {
            observer(success);
        }
    }
}

impl Drop for ParkCompletion {
    fn drop(&mut self) {
        if self.enqueued {
            if let Some(observer) = self.observer.take() {
                observer(false);
            }
        }
    }
}

impl LocalFileRuntime {
    pub(crate) fn park_error(&self) -> Option<Arc<io::Error>> {
        self.queue.park_error()
    }
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
        let unavailable = Arc::clone(&writer.unavailable);
        let statistics = Arc::clone(&writer.statistics);
        let (writer, worker) = non_blocking_file_writer(writer, Arc::clone(&dropped_records))?;
        Ok(Self {
            queue: FileQueue {
                writer,
                worker,
                dropped_records,
                unavailable,
            },
            statistics,
        })
    }

    pub(crate) fn record_rejection(&self) {
        self.queue.record_rejection();
    }

    pub(crate) fn statistics(&self) -> Vec<FileSourceCounts> {
        self.statistics.snapshot()
    }

    pub(crate) fn writer(&self) -> AsyncFileWriter {
        self.queue.writer()
    }

    pub(crate) fn dropped_records(&self) -> u64 {
        self.queue.dropped_records()
    }

    /// 最近一次写入或刷新失败且尚未恢复。
    pub(crate) fn unavailable(&self) -> bool {
        self.queue.unavailable()
    }

    pub(crate) fn flush(&self, deadline: Duration) -> bool {
        self.queue.flush(deadline)
    }

    pub(crate) fn flush_result(&self, deadline: Duration) -> SignalResult {
        self.queue.flush_result(deadline)
    }

    pub(crate) fn park(&self, deadline: Duration) -> SignalResult {
        self.queue.park(deadline)
    }

    pub(crate) fn has_live_closer(&self) -> bool {
        self.queue.has_live_closer()
    }

    pub(crate) fn unpark(&self, deadline: Duration) -> SignalResult {
        self.queue.unpark(deadline)
    }

    pub(crate) fn shutdown(&self, deadline: Duration) -> bool {
        self.queue.shutdown(deadline)
    }

    pub(crate) fn shutdown_cleanup_incomplete(&self) -> bool {
        self.queue.shutdown_cleanup_incomplete()
    }
}

impl FileQueue {
    pub(crate) fn park_error(&self) -> Option<Arc<io::Error>> {
        lock(&self.worker.park_error).clone()
    }
    pub(crate) fn has_live_closer(&self) -> bool {
        !self.worker.closed.load(Ordering::Acquire)
            && lock(&self.worker.thread)
                .as_ref()
                .is_some_and(|thread| !thread.is_finished())
    }
    /// 在独立线程上启动队列；`store` 与线程只属于这一个队列。
    pub(crate) fn spawn<S: RecordStore>(
        store: S,
        counters: Arc<dyn RecordCounters>,
        hooks: HealthHooks,
        thread_name: &str,
        capacity: usize,
    ) -> io::Result<Self> {
        let dropped_records = Arc::new(AtomicU64::new(0));
        let unavailable = Arc::new(AtomicBool::new(false));
        let (writer, worker) = spawn_file_queue(
            store,
            counters,
            Arc::clone(&dropped_records),
            Arc::clone(&unavailable),
            hooks,
            thread_name,
            capacity,
        )?;
        Ok(Self {
            writer,
            worker,
            dropped_records,
            unavailable,
        })
    }

    pub(crate) fn record_rejection(&self) {
        increment(&self.dropped_records);
    }

    pub(crate) fn writer(&self) -> AsyncFileWriter {
        self.writer.clone()
    }

    pub(crate) fn dropped_records(&self) -> u64 {
        self.dropped_records.load(Ordering::Relaxed)
    }

    pub(crate) fn unavailable(&self) -> bool {
        self.unavailable.load(Ordering::Acquire)
    }

    pub(crate) fn flush(&self, deadline: Duration) -> bool {
        self.worker.flush(deadline)
    }

    pub(crate) fn flush_result(&self, deadline: Duration) -> SignalResult {
        self.worker.signal(FileMessage::Flush, deadline, || {})
    }

    pub(crate) fn shutdown(&self, deadline: Duration) -> bool {
        self.worker.shutdown(deadline)
    }

    /// 等待同一次已接受的关闭结束；等待超时不启动新的关闭尝试。
    pub(crate) fn shutdown_until_complete(&self) -> bool {
        self.worker.shutdown(Duration::ZERO);
        loop {
            if let Some(result) = self.worker.shutdown.wait(Duration::from_secs(60)) {
                return result;
            }
        }
    }

    pub(crate) fn shutdown_cleanup_incomplete(&self) -> bool {
        self.worker.shutdown_cleanup_incomplete()
    }

    pub(crate) fn park(&self, deadline: Duration) -> SignalResult {
        self.worker.park(deadline, None, Box::new(|| {}))
    }

    pub(crate) fn park_with_observer(
        &self,
        deadline: Duration,
        observer: Box<dyn FnOnce(bool) + Send>,
        accepted: Box<dyn FnOnce() + Send>,
    ) -> SignalResult {
        self.worker.park(deadline, Some(observer), accepted)
    }

    pub(crate) fn unpark(&self, deadline: Duration) -> SignalResult {
        self.worker.unpark(deadline, None)
    }

    pub(crate) fn unpark_with_observer(
        &self,
        deadline: Duration,
        observer: Box<dyn FnOnce(bool) + Send>,
    ) -> SignalResult {
        self.worker.unpark(deadline, Some(observer))
    }
}

fn non_blocking_file_writer(
    writer: BoundedDailyMakeWriter,
    dropped_records: Arc<AtomicU64>,
) -> io::Result<(AsyncFileWriter, Arc<LocalFileWorker>)> {
    let counters: Arc<dyn RecordCounters> = Arc::clone(&writer.statistics) as _;
    let unavailable = Arc::clone(&writer.unavailable);
    spawn_file_queue(
        writer,
        counters,
        dropped_records,
        unavailable,
        HealthHooks::ENGINE,
        "uc-observability-file",
        ASYNC_QUEUE_CAPACITY,
    )
}

fn spawn_file_queue<S: RecordStore>(
    store: S,
    counters: Arc<dyn RecordCounters>,
    dropped_records: Arc<AtomicU64>,
    unavailable: Arc<AtomicBool>,
    hooks: HealthHooks,
    thread_name: &str,
    capacity: usize,
) -> io::Result<(AsyncFileWriter, Arc<LocalFileWorker>)> {
    let (sender, receiver) = mpsc::sync_channel(capacity);
    let closed = Arc::new(AtomicBool::new(false));
    let parked = Arc::new(AtomicBool::new(false));
    let submission = Arc::new(Mutex::new(()));
    let worker_dropped_records = Arc::clone(&dropped_records);
    let worker_counters = Arc::clone(&counters);
    let worker_closed = Arc::clone(&closed);
    let park_error = Arc::new(Mutex::new(None));
    let worker_park_error = Arc::clone(&park_error);
    #[cfg(feature = "test-process-release")]
    let daily_worker = thread_name == "uc-observability-file";
    let thread = std::thread::Builder::new()
        .name(thread_name.to_owned())
        .spawn(move || {
            #[cfg(feature = "test-process-release")]
            let _exit_gate = daily_worker.then(|| ProbeDailyExitGate);
            run_file_worker(
                store,
                receiver,
                worker_dropped_records,
                worker_counters,
                unavailable,
                hooks,
                worker_closed,
                worker_park_error,
            )
        })?;
    Ok((
        AsyncFileWriter {
            sender: sender.clone(),
            dropped_records,
            closed: Arc::clone(&closed),
            parked: Arc::clone(&parked),
            submission: Arc::clone(&submission),
            counters,
        },
        Arc::new(LocalFileWorker {
            transition: Mutex::new(()),
            sender,
            thread: Arc::new(Mutex::new(Some(thread))),
            closed,
            parked,
            submission,
            transition_id: Arc::new(Mutex::new(Arc::new(()))),
            shutdown: Arc::new(FileShutdownCoordinator::default()),
            park_error,
        }),
    ))
}

impl AsyncFileWriter {
    pub(crate) fn write_record(&self, buffer: &[u8], source: LocalDiagnosticSource) {
        self.write_indexed(buffer, source as usize);
    }

    pub(crate) fn write_indexed(&self, buffer: &[u8], source: usize) {
        // 门只保护封口与入队的先后顺序，持有者只做不阻塞的 `try_send`，所以这里等待锁不会等待磁盘或网络；
        // 争用本身不得丢记录，只有队列已满或已封口才计入丢弃。
        let _submission = lock(&self.submission);
        if self.closed.load(Ordering::Acquire) || self.parked.load(Ordering::Acquire) {
            increment(&self.dropped_records);
            self.counters.closed_dropped(source);
        } else if self
            .sender
            .try_send(FileMessage::Line(buffer.to_vec(), source))
            .is_err()
        {
            increment(&self.dropped_records);
            self.counters.queue_dropped(source);
        } else {
            self.counters.accepted(source);
        }
    }
}

impl Write for AsyncFileWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.write_record(buffer, LocalDiagnosticSource::Runtime);
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
        self.signal(FileMessage::Flush, deadline, || {}) == SignalResult::Completed
    }

    /// 先在提交门内停止接收新记录，再把停车请求排在已入队记录之后：已接受的记录全部落盘后才交还文件。
    pub(crate) fn park(
        &self,
        deadline: Duration,
        observer: Option<Box<dyn FnOnce(bool) + Send>>,
        accepted: Box<dyn FnOnce() + Send>,
    ) -> SignalResult {
        let started = Instant::now();
        let Some(_transition) = lock_before_deadline(&self.transition, started, deadline) else {
            return SignalResult::TimedOut;
        };
        *lock(&self.park_error) = None;
        {
            let Some(_submission) = lock_before_deadline(&self.submission, started, deadline)
            else {
                return SignalResult::TimedOut;
            };
            if self.closed.load(Ordering::Acquire) {
                return SignalResult::AlreadyShutdown;
            }
            *lock(&self.transition_id) = Arc::new(());
            self.parked.store(true, Ordering::Release);
        }
        self.signal(
            |reply| {
                FileMessage::Park(
                    reply,
                    ParkCompletion {
                        observer,
                        enqueued: false,
                    },
                )
            },
            deadline.saturating_sub(started.elapsed()),
            accepted,
        )
    }

    /// 重开成功后才放行写入；等待超时不撤销已接受的重开。
    pub(crate) fn unpark(
        &self,
        deadline: Duration,
        observer: Option<Box<dyn FnOnce(bool) + Send>>,
    ) -> SignalResult {
        let started = Instant::now();
        let Some(_transition) = lock_before_deadline(&self.transition, started, deadline) else {
            return SignalResult::TimedOut;
        };
        let operation = Arc::new(());
        let transition_id = Arc::clone(&self.transition_id);
        let current = Arc::clone(&operation);
        let submission = Arc::clone(&self.submission);
        let closed = Arc::clone(&self.closed);
        let parked = Arc::clone(&self.parked);
        let accepted_id = Arc::clone(&self.transition_id);
        self.signal(
            |reply| {
                FileMessage::Unpark(
                    reply,
                    Box::new(move |success| {
                        let _submission = lock(&submission);
                        if success
                            && !closed.load(Ordering::Acquire)
                            && Arc::ptr_eq(&lock(&transition_id), &current)
                        {
                            parked.store(false, Ordering::Release);
                        }
                        if let Some(observer) = observer {
                            observer(success);
                        }
                    }),
                )
            },
            deadline.saturating_sub(started.elapsed()),
            || *lock(&accepted_id) = operation,
        )
    }

    /// 把一次带回复的控制请求（刷新、停车、重开）排入队列并等待回复；`enqueued` 在请求入队后、仍持有提交门时执行。
    fn signal(
        &self,
        request: impl FnOnce(mpsc::Sender<bool>) -> FileMessage,
        deadline: Duration,
        enqueued: impl FnOnce(),
    ) -> SignalResult {
        let started = Instant::now();
        let (sender, receiver) = mpsc::channel();
        let mut message = request(sender);
        let mut enqueued = Some(enqueued);
        loop {
            // 队列已满时在两次尝试之间释放提交门，等待期间不阻塞写入方。
            {
                let Some(_submission) = lock_before_deadline(&self.submission, started, deadline)
                else {
                    return SignalResult::TimedOut;
                };
                if started.elapsed() >= deadline {
                    return SignalResult::TimedOut;
                }
                if self.closed.load(Ordering::Acquire) {
                    return SignalResult::AlreadyShutdown;
                }
                if let FileMessage::Park(_, completion) = &mut message {
                    completion.enqueued = true;
                }
                match self.sender.try_send(message) {
                    Ok(()) => {
                        if let Some(enqueued) = enqueued.take() {
                            enqueued();
                        }
                        break;
                    }
                    Err(mpsc::TrySendError::Full(mut returned)) => {
                        if let FileMessage::Park(_, completion) = &mut returned {
                            completion.enqueued = false;
                        }
                        message = returned;
                    }
                    Err(mpsc::TrySendError::Disconnected(mut returned)) => {
                        if let FileMessage::Park(_, completion) = &mut returned {
                            completion.enqueued = false;
                        }
                        return SignalResult::Failed;
                    }
                }
            }
            if started.elapsed() >= deadline {
                return SignalResult::TimedOut;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        match receiver.recv_timeout(deadline.saturating_sub(started.elapsed())) {
            Ok(true) => SignalResult::Completed,
            Ok(false) | Err(mpsc::RecvTimeoutError::Disconnected) => SignalResult::Failed,
            Err(mpsc::RecvTimeoutError::Timeout) => SignalResult::TimedOut,
        }
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

/// 失败后的重试退避；在退避内到达的记录直接丢弃并计数，不再反复触碰失败的存储。
const RETRY_BACKOFF_MIN: Duration = Duration::from_millis(100);
const RETRY_BACKOFF_MAX: Duration = Duration::from_secs(5);

/// 文件线程内的恢复状态：写入或刷新失败进入失败态，任一次成功即恢复，不存在永久失败。
struct WorkerHealth {
    unavailable: Arc<AtomicBool>,
    hooks: HealthHooks,
    failed_since: Option<Instant>,
    retry_at: Instant,
    backoff: Duration,
}

impl WorkerHealth {
    fn new(unavailable: Arc<AtomicBool>, hooks: HealthHooks) -> Self {
        Self {
            unavailable,
            hooks,
            failed_since: None,
            retry_at: Instant::now(),
            backoff: RETRY_BACKOFF_MIN,
        }
    }

    fn in_backoff(&self) -> bool {
        self.failed_since.is_some() && Instant::now() < self.retry_at
    }

    fn failed(&mut self, class: &'static str) {
        if self.failed_since.is_none() {
            self.failed_since = Some(Instant::now());
            self.backoff = RETRY_BACKOFF_MIN;
            self.unavailable.store(true, Ordering::Release);
            (self.hooks.failed)(class);
        } else {
            self.backoff = (self.backoff * 2).min(RETRY_BACKOFF_MAX);
        }
        self.retry_at = Instant::now() + self.backoff;
    }

    fn succeeded(&mut self) {
        if self.failed_since.take().is_some() {
            self.backoff = RETRY_BACKOFF_MIN;
            self.unavailable.store(false, Ordering::Release);
            (self.hooks.recovered)();
        }
    }

    fn is_failed(&self) -> bool {
        self.failed_since.is_some()
    }
}

fn run_file_worker<S: RecordStore>(
    mut writer: S,
    receiver: mpsc::Receiver<FileMessage>,
    dropped_records: Arc<AtomicU64>,
    counters: Arc<dyn RecordCounters>,
    unavailable: Arc<AtomicBool>,
    hooks: HealthHooks,
    closed: Arc<AtomicBool>,
    park_error: Arc<Mutex<Option<Arc<io::Error>>>>,
) {
    let mut health = WorkerHealth::new(unavailable, hooks);
    if writer.prepare().is_err() {
        health.failed("open_failed");
    }
    while let Ok(message) = receiver.recv() {
        match message {
            FileMessage::Line(bytes, source) => {
                if health.in_backoff() {
                    increment(&dropped_records);
                    counters.write_failed(source);
                } else if writer.store_record(&bytes, source).is_err() {
                    increment(&dropped_records);
                    counters.write_failed(source);
                    health.failed("write_failed");
                } else {
                    health.succeeded();
                }
            }
            FileMessage::Flush(reply) => {
                // 刷新是明确的重试点：失败态下也尝试重新打开，不受退避限制。
                let _ = reply.send(settle_flush(&mut writer, &mut health));
            }
            FileMessage::Park(reply, observer) => {
                let result = match catch_unwind(AssertUnwindSafe(|| writer.park())) {
                    Ok(result) => match result {
                        Ok(()) => {
                            *lock(&park_error) = None;
                            true
                        }
                        Err(error) => {
                            *lock(&park_error) = Some(Arc::new(error));
                            false
                        }
                    },
                    Err(_) => {
                        *lock(&park_error) = None;
                        closed.store(true, Ordering::Release);
                        observer.finish(false);
                        let _ = reply.send(false);
                        return;
                    }
                };
                observer.finish(result);
                let _ = reply.send(result);
            }
            FileMessage::Unpark(reply, observer) => {
                let reopened = writer.unpark().is_ok();
                if reopened {
                    health.succeeded();
                } else {
                    health.failed("open_failed");
                }
                observer(reopened);
                let _ = reply.send(reopened);
            }
            FileMessage::Shutdown(reply) => {
                let _ = reply.send(settle_flush(&mut writer, &mut health));
                return;
            }
        }
    }
}

fn settle_flush<S: RecordStore>(writer: &mut S, health: &mut WorkerHealth) -> bool {
    if writer.settle().is_ok() {
        health.succeeded();
    } else {
        health.failed("flush_failed");
    }
    !health.is_failed()
}

impl BoundedDailyMakeWriter {
    fn write_record(&self, buffer: &[u8], source: LocalDiagnosticSource) -> io::Result<()> {
        let mut state = lock(&self.state);
        let state = state
            .as_mut()
            .ok_or_else(|| io::Error::other("daily log is parked"))?;
        state.rotate_if_needed()?;
        state.ensure_current()?;
        let additional = u64::try_from(buffer.len()).unwrap_or(u64::MAX);
        if !state.make_room(additional)? {
            increment(&self.dropped_records);
            increment(&self.statistics.source(source).quota_dropped);
            return Ok(());
        }
        state.file.write_all(buffer)?;
        state.total_bytes = state.total_bytes.saturating_add(additional);
        state.current_bytes = state.current_bytes.saturating_add(additional);
        increment(&self.statistics.source(source).written);
        if let Ok(timestamp) = u64::try_from(chrono::Utc::now().timestamp_millis()) {
            self.statistics
                .source(source)
                .last_written_at_ms
                .store(timestamp, Ordering::Relaxed);
        }
        Ok(())
    }
}

impl Write for BoundedDailyWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.owner
            .write_record(buffer, LocalDiagnosticSource::Runtime)?;
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut state = lock(&self.owner.state);
        let Some(state) = state.as_mut() else {
            return Ok(());
        };
        state.ensure_current()?;
        state.file.flush()
    }
}

struct WriterState {
    directory: PathBuf,
    date: NaiveDate,
    file: File,
    total_bytes: u64,
    /// 当天文件的字节数；它始终包含在 `total_bytes` 内。
    current_bytes: u64,
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
        std::fs::create_dir_all(&directory)?;
        let path = directory.join(file_name(date));
        let file = open_managed_file(&path)?;
        let current_bytes = file.metadata()?.len();
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
            current_bytes,
            retention_days,
            max_bytes,
        })
    }

    /// 为新记录腾出配额：从最旧的受管文件起淘汰，永不触碰当天文件（早于或晚于当天的受管文件都可淘汰，时钟回拨留下的未来日期文件也一样），使新的诊断总能写入。
    ///
    /// 即使淘汰全部旧文件也放不下（单条超额或当天文件独占配额）时不删除任何文件，返回
    /// `false` 由调用方丢弃该条。只处理受管文件名，宿主放在同目录的其他文件不受影响。
    fn make_room(&mut self, additional: u64) -> io::Result<bool> {
        if self.total_bytes.saturating_add(additional) <= self.max_bytes {
            return Ok(true);
        }
        if self.current_bytes.saturating_add(additional) > self.max_bytes {
            return Ok(false);
        }
        for entry in managed_files(&self.directory)? {
            if self.total_bytes.saturating_add(additional) <= self.max_bytes {
                break;
            }
            if entry.date == self.date {
                continue;
            }
            let size = entry.path.metadata()?.len();
            std::fs::remove_file(&entry.path)?;
            self.total_bytes = self.total_bytes.saturating_sub(size);
        }
        Ok(self.total_bytes.saturating_add(additional) <= self.max_bytes)
    }

    fn rotate_if_needed(&mut self) -> io::Result<()> {
        let today = Utc::now().date_naive();
        if today == self.date {
            return Ok(());
        }
        std::fs::create_dir_all(&self.directory)?;
        cleanup(&self.directory, today, self.retention_days, self.max_bytes)?;
        *self = Self::open(
            self.directory.clone(),
            today,
            self.retention_days,
            self.max_bytes,
        )?;
        Ok(())
    }

    /// 打开的文件仍是目录里的当天文件；目录或文件被外部删除、替换后重建并重新统计配额。
    ///
    /// 已打开的描述符在文件被删除后依旧可写，记录会静默落入不可达的文件，所以每次写入前都核对路径。
    fn ensure_current(&mut self) -> io::Result<()> {
        let path = self.directory.join(file_name(self.date));
        if std::fs::symlink_metadata(&path).is_ok_and(|metadata| same_file(&metadata, &self.file)) {
            return Ok(());
        }
        *self = Self::open(
            self.directory.clone(),
            self.date,
            self.retention_days,
            self.max_bytes,
        )?;
        Ok(())
    }
}

#[cfg(unix)]
pub(crate) fn same_file(path_metadata: &std::fs::Metadata, file: &File) -> bool {
    use std::os::unix::fs::MetadataExt;

    file.metadata().is_ok_and(|opened| {
        opened.dev() == path_metadata.dev() && opened.ino() == path_metadata.ino()
    })
}

#[cfg(not(unix))]
pub(crate) fn same_file(path_metadata: &std::fs::Metadata, _file: &File) -> bool {
    path_metadata.is_file()
}

pub(crate) fn open_managed_file(path: &Path) -> io::Result<File> {
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
        // 当天文件正在或即将被写入，即使独占配额也不在这里淘汰。
        if entry.date == today {
            continue;
        }
        let size = entry.path.metadata()?.len();
        std::fs::remove_file(entry.path)?;
        total = total.saturating_sub(size);
    }
    Ok(())
}

/// 把旧目录里的受管日志文件一次性迁入新目录，返回迁入的文件数。
///
/// 只处理严格命名的日志文件；新目录已有同名文件时保留两份，不覆盖也不合并。必须在写入器打开新目录之前调用。
/// 同一文件系统内用重命名，跨文件系统时复制后删除源文件；任一文件失败即返回错误，已迁入的文件保留在新目录，
/// 重复调用会继续迁移剩余文件。
pub fn migrate_managed_log_files(from: &Path, to: &Path) -> io::Result<usize> {
    if from == to {
        return Ok(0);
    }
    let files = match managed_files(from) {
        Ok(files) => files,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    if files.is_empty() {
        return Ok(0);
    }
    std::fs::create_dir_all(to)?;
    let mut moved = 0;
    for entry in files {
        let Some(name) = entry.path.file_name() else {
            continue;
        };
        let destination = to.join(name);
        if destination.try_exists()? {
            continue;
        }
        if std::fs::rename(&entry.path, &destination).is_err() {
            std::fs::copy(&entry.path, &destination)?;
            std::fs::remove_file(&entry.path)?;
        }
        moved += 1;
    }
    // 旧目录已空时顺带移除；非空（宿主放了其他文件）则保持不动。
    let _ = std::fs::remove_dir(from);
    Ok(moved)
}

pub fn managed_log_files(directory: &Path) -> io::Result<Vec<PathBuf>> {
    managed_files(directory).map(|files| files.into_iter().map(|entry| entry.path).collect())
}

fn file_name(date: NaiveDate) -> String {
    managed_log_file_name(date)
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
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

    fn managed_file(directory: &Path, date: &str, bytes: usize) -> PathBuf {
        let path = directory.join(format!("engine.{date}.jsonl"));
        std::fs::write(&path, vec![b'x'; bytes]).expect("managed file");
        path
    }

    #[test]
    fn full_quota_evicts_the_oldest_managed_file_for_a_new_record() {
        let directory = tempdir().expect("temp dir");
        let oldest = managed_file(directory.path(), "2000-01-01", 4);
        let newer = managed_file(directory.path(), "2000-01-02", 5);
        let unmanaged = directory.path().join("keep.txt");
        std::fs::write(&unmanaged, b"private host file").expect("unmanaged");
        let (mut writer, dropped) =
            BoundedDailyMakeWriter::new_with_limits(directory.path(), 36_500, 10)
                .expect("bounded writer");

        writer.write_all(b"12345").expect("new record");
        writer.flush().expect("flush");

        assert!(!oldest.exists());
        assert!(newer.exists());
        assert!(unmanaged.exists());
        assert_eq!(dropped.load(Ordering::Relaxed), 0);
        let today = directory.path().join(file_name(Utc::now().date_naive()));
        assert_eq!(std::fs::read(today).expect("today"), b"12345");
    }

    #[test]
    fn record_larger_than_the_whole_quota_is_dropped_without_evicting_anything() {
        let directory = tempdir().expect("temp dir");
        let old = managed_file(directory.path(), "2000-01-01", 4);
        let (mut writer, dropped) =
            BoundedDailyMakeWriter::new_with_limits(directory.path(), 36_500, 10)
                .expect("bounded writer");

        writer
            .write_all(&[b'y'; 11])
            .expect("dropped record is non-fatal");

        assert!(old.exists());
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn current_day_file_is_never_evicted_to_make_room() {
        let directory = tempdir().expect("temp dir");
        let (mut writer, dropped) =
            BoundedDailyMakeWriter::new_with_limits(directory.path(), 36_500, 10)
                .expect("bounded writer");

        writer.write_all(b"12345678").expect("first record");
        writer
            .write_all(b"abcd")
            .expect("dropped record is non-fatal");
        writer.flush().expect("flush");

        let today = directory.path().join(file_name(Utc::now().date_naive()));
        assert_eq!(std::fs::read(today).expect("today"), b"12345678");
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn future_dated_managed_files_can_be_evicted_after_a_clock_rollback() {
        let directory = tempdir().expect("temp dir");
        let future = managed_file(directory.path(), "2999-01-01", 8);
        let (mut writer, dropped) =
            BoundedDailyMakeWriter::new_with_limits(directory.path(), 36_500, 10)
                .expect("bounded writer");

        writer.write_all(b"12345").expect("new record");
        writer.flush().expect("flush");

        assert!(!future.exists());
        assert_eq!(dropped.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn cleanup_never_removes_the_current_day_file_even_when_it_alone_exceeds_the_cap() {
        let directory = tempdir().expect("temp dir");
        let today = NaiveDate::from_ymd_opt(2026, 9, 4).expect("date");
        let older = managed_file(directory.path(), "2026-09-03", 3);
        let current = managed_file(directory.path(), "2026-09-04", 20);

        cleanup(directory.path(), today, 7, 5).expect("cleanup");

        assert!(!older.exists());
        assert!(current.exists());
    }

    #[test]
    fn day_rollover_recreates_a_directory_removed_while_the_writer_was_open() {
        let root = tempdir().expect("temp dir");
        let directory = root.path().join("logs");
        std::fs::create_dir(&directory).expect("log directory");
        let yesterday = Utc::now()
            .date_naive()
            .checked_sub_days(Days::new(1))
            .expect("date");
        let mut state =
            WriterState::open(directory.clone(), yesterday, 7, 1_024).expect("yesterday file");
        std::fs::remove_dir_all(&directory).expect("remove directory");

        state
            .rotate_if_needed()
            .expect("rotation recreates the directory");
        state.ensure_current().expect("current file");

        state.file.write_all(b"after-rollover\n").expect("write");
        let today = directory.join(file_name(Utc::now().date_naive()));
        assert_eq!(std::fs::read(today).expect("today"), b"after-rollover\n");
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
            .try_send(FileMessage::Line(
                b"before-flush\n".to_vec(),
                LocalDiagnosticSource::Runtime as usize,
            ))
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
            transition: Mutex::new(()),
            sender,
            thread: Arc::new(Mutex::new(None)),
            closed: Arc::new(AtomicBool::new(false)),
            parked: Arc::new(AtomicBool::new(false)),
            submission: Arc::new(Mutex::new(())),
            transition_id: Arc::new(Mutex::new(Arc::new(()))),
            shutdown: Arc::new(FileShutdownCoordinator::default()),
            park_error: Arc::new(Mutex::new(None)),
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
        assert_eq!(
            worker.signal(FileMessage::Flush, Duration::from_millis(5), || {}),
            SignalResult::TimedOut
        );
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
        let counts = runtime.statistics();
        assert_eq!(counts[0].quota_dropped_count, 1);
        assert_eq!(counts[0].write_failed_count, 0);
        assert_eq!(counts[0].written_count, 1);
        assert!(counts[0].last_written_at_ms.is_some());
        assert!(counts[1].last_written_at_ms.is_none());

        assert!(runtime.shutdown(Duration::from_secs(1)));
        writer
            .write_all(b"after-shutdown")
            .expect("closed writer remains non-fatal");
        assert_eq!(runtime.dropped_records(), 2);
        assert_eq!(runtime.statistics()[0].queue_dropped_count, 1);
    }

    #[test]
    fn control_request_honors_deadline_when_the_queue_cannot_accept_it() {
        let (sender, receiver) = mpsc::sync_channel(0);
        let worker = Arc::new(LocalFileWorker {
            transition: Mutex::new(()),
            sender,
            thread: Arc::new(Mutex::new(None)),
            closed: Arc::new(AtomicBool::new(false)),
            parked: Arc::new(AtomicBool::new(false)),
            submission: Arc::new(Mutex::new(())),
            transition_id: Arc::new(Mutex::new(Arc::new(()))),
            shutdown: Arc::new(FileShutdownCoordinator::default()),
            park_error: Arc::new(Mutex::new(None)),
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
            transition: Mutex::new(()),
            sender,
            thread: Arc::new(Mutex::new(Some(std::thread::spawn(|| {})))),
            closed: Arc::new(AtomicBool::new(false)),
            parked: Arc::new(AtomicBool::new(false)),
            submission: Arc::new(Mutex::new(())),
            transition_id: Arc::new(Mutex::new(Arc::new(()))),
            shutdown: Arc::new(FileShutdownCoordinator::default()),
            park_error: Arc::new(Mutex::new(None)),
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
            transition: Mutex::new(()),
            sender,
            thread: Arc::new(Mutex::new(Some(std::thread::spawn(|| {})))),
            closed: Arc::new(AtomicBool::new(false)),
            parked: Arc::new(AtomicBool::new(false)),
            submission: Arc::new(Mutex::new(())),
            transition_id: Arc::new(Mutex::new(Arc::new(()))),
            shutdown: Arc::new(FileShutdownCoordinator::default()),
            park_error: Arc::new(Mutex::new(None)),
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
    fn concurrent_producers_never_lose_records_to_gate_contention() {
        const PRODUCERS: usize = 8;
        const PER_PRODUCER: usize = 400;
        let directory = tempdir().expect("temp dir");
        let runtime = LocalFileRuntime::new_with_limits(directory.path(), 7, 10_000_000)
            .expect("local file runtime");
        // 总量低于队列容量，任何丢弃都只能来自提交门争用。
        assert!(PRODUCERS * PER_PRODUCER < ASYNC_QUEUE_CAPACITY);
        let start = Arc::new(std::sync::Barrier::new(PRODUCERS));
        let producers: Vec<_> = (0..PRODUCERS)
            .map(|_| {
                let mut writer = runtime.writer();
                let start = Arc::clone(&start);
                std::thread::spawn(move || {
                    start.wait();
                    for _ in 0..PER_PRODUCER {
                        writer.write_all(b"record\n").expect("write");
                    }
                })
            })
            .collect();
        for producer in producers {
            producer.join().expect("producer");
        }
        assert!(runtime.flush(Duration::from_secs(5)));

        assert_eq!(runtime.dropped_records(), 0);
        let files = managed_log_files(directory.path()).expect("files");
        let written = std::fs::read_to_string(&files[0])
            .expect("file")
            .lines()
            .count();
        assert_eq!(written, PRODUCERS * PER_PRODUCER);
    }
}
