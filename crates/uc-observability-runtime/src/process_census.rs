//! 进程资源周期记录与增长告警（Engine #214 的诊断）。
//!
//! 守护进程的匿名堆曾在 20 多小时内增长到 17 GiB，而日志里没有任何一条能说明是哪类内存在涨。
//! 这里按固定周期把进程自己的资源计数写进模块日志：匿名/文件/共享驻留、交换、线程数、文件描述符、
//! `smaps_rollup` 的匿名 PSS，以及 glibc 的堆内活跃/空闲字节，用来区分“存活对象在涨”和“分配器没有归还”。
//! 只有计数和字节数；没有地址、标识、路径或内容。读不到的值直接省略，不写成 0。
//!
//! 增长告警只比较同一进程内的匿名驻留：窗口内相对窗口最低点增长超过阈值时写一条 WARN，
//! 冷却期内不重复。它只写记录，不停止、限速或重启任何东西。
//!
//! 其他运行期事实（tokio 任务数、网络连接与路径数）由各自的负责人在同一周期写自己的记录，
//! 按时间戳与本记录对照；这里不跨层读取它们。

// 只有 Linux 与 Android 有读取来源；其他平台只编译判定逻辑（供测试），来源相关项不会被使用。
#![cfg_attr(not(any(target_os = "linux", target_os = "android")), allow(dead_code))]

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{Builder, JoinHandle};
use std::time::{Duration, Instant};

#[cfg(feature = "test-process-release")]
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

#[cfg(feature = "test-process-release")]
pub(crate) static PROBE_CENSUS_ENABLED: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "test-process-release")]
pub(crate) static PROBE_CENSUS_READS: AtomicU64 = AtomicU64::new(0);

#[cfg(feature = "test-process-release")]
struct ProbeSource;

#[cfg(feature = "test-process-release")]
impl ProcessSource for ProbeSource {
    fn read(&mut self) -> ProcessReading {
        PROBE_CENSUS_READS.fetch_add(1, Ordering::SeqCst);
        ProcessReading::default()
    }
}

use uc_observability_contract::{uc_info, uc_warn};

/// 资源记录周期。
#[cfg(any(target_os = "linux", target_os = "android"))]
const CADENCE: Duration = Duration::from_secs(300);
/// 启动后的首条记录延后一小段时间：启动瞬间的数值没有诊断价值，短生命周期的进程也不会被周期记录打扰。
#[cfg(any(target_os = "linux", target_os = "android"))]
const INITIAL_DELAY: Duration = Duration::from_secs(30);
/// 增长告警比较的窗口（秒）。
const GROWTH_WINDOW_SECS: u64 = 60 * 60;
/// 窗口内相对最低点的匿名驻留增长阈值（KiB）：512 MiB。
const GROWTH_THRESHOLD_KB: u64 = 512 * 1024;
/// 两次告警之间的最短间隔（秒）。
const ALERT_COOLDOWN_SECS: u64 = 60 * 60;
/// 窗口内保留的样本上限；周期固定时窗口内只有 13 个，上限只防止异常调度下无界增长。
const MAX_WINDOW_SAMPLES: usize = 64;

/// 一次读取到的进程资源；读不到的项为 `None`。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ProcessReading {
    pub(crate) rss_anon_kb: Option<u64>,
    pub(crate) rss_file_kb: Option<u64>,
    pub(crate) rss_shmem_kb: Option<u64>,
    pub(crate) vm_swap_kb: Option<u64>,
    pub(crate) vm_size_kb: Option<u64>,
    pub(crate) threads: Option<u64>,
    pub(crate) fd_total: Option<u64>,
    pub(crate) pss_anon_kb: Option<u64>,
    pub(crate) heap_in_use_kb: Option<u64>,
    pub(crate) heap_free_kb: Option<u64>,
    pub(crate) heap_mmap_kb: Option<u64>,
}

impl ProcessReading {
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// 资源来源，测试用假实现替换。
pub(crate) trait ProcessSource: Send + 'static {
    fn read(&mut self) -> ProcessReading;
}

/// 从 `/proc/self` 与 glibc 读取；仅在 Linux 与 Android 上有实现。
#[cfg(any(target_os = "linux", target_os = "android"))]
pub(crate) struct ProcSource {
    root: std::path::PathBuf,
}

#[cfg(any(target_os = "linux", target_os = "android"))]
impl ProcSource {
    pub(crate) fn new() -> Self {
        Self {
            root: std::path::PathBuf::from("/proc/self"),
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
impl ProcessSource for ProcSource {
    fn read(&mut self) -> ProcessReading {
        let mut reading = ProcessReading::default();
        if let Ok(status) = std::fs::read_to_string(self.root.join("status")) {
            apply_status(&status, &mut reading);
        }
        if let Ok(rollup) = std::fs::read_to_string(self.root.join("smaps_rollup")) {
            reading.pss_anon_kb = kb_field(&rollup, "Pss_Anon:");
        }
        reading.fd_total = std::fs::read_dir(self.root.join("fd"))
            .ok()
            // 目录句柄本身也占一个描述符，读到的条目数已经包含它。
            .map(|entries| entries.count() as u64);
        apply_allocator(&mut reading);
        reading
    }
}

/// 取形如 `Key:\t  1234 kB` 的行的数值。
fn kb_field(text: &str, key: &str) -> Option<u64> {
    text.lines()
        .find_map(|line| line.strip_prefix(key))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|value| value.parse().ok())
}

fn apply_status(status: &str, reading: &mut ProcessReading) {
    reading.rss_anon_kb = kb_field(status, "RssAnon:");
    reading.rss_file_kb = kb_field(status, "RssFile:");
    reading.rss_shmem_kb = kb_field(status, "RssShmem:");
    reading.vm_swap_kb = kb_field(status, "VmSwap:");
    reading.vm_size_kb = kb_field(status, "VmSize:");
    reading.threads = status
        .lines()
        .find_map(|line| line.strip_prefix("Threads:"))
        .and_then(|rest| rest.trim().parse().ok());
}

/// glibc 的 `mallinfo2` 结果（`struct mallinfo2`，全部为 `size_t`）。
#[cfg(all(target_os = "linux", target_env = "gnu"))]
#[repr(C)]
struct Mallinfo2 {
    arena: usize,
    ordblks: usize,
    smblks: usize,
    hblks: usize,
    hblkhd: usize,
    usmblks: usize,
    fsmblks: usize,
    uordblks: usize,
    fordblks: usize,
    keepcost: usize,
}

/// 经 `dlsym` 在运行期解析 `mallinfo2`（glibc 2.33 起提供）。直接链接会让二进制在更旧的发行版上无法加载；
/// 旧的 `mallinfo` 用 32 位整数，超过 4 GiB 即回绕，不可用。解析不到时三个堆字段都省略。
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn apply_allocator(reading: &mut ProcessReading) {
    type Mallinfo2Fn = unsafe extern "C" fn() -> Mallinfo2;
    // SAFETY: `RTLD_DEFAULT` 与以 NUL 结尾的符号名都是有效参数；`dlsym` 只读符号表。
    let symbol = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"mallinfo2".as_ptr()) };
    if symbol.is_null() {
        return;
    }
    // SAFETY: glibc 的 `mallinfo2` 无参数、返回 `struct mallinfo2`，与 `Mallinfo2` 布局一致；
    // 符号非空，且函数只读取 malloc 内部计数，可从任意线程调用。
    let info = unsafe {
        let function: Mallinfo2Fn = std::mem::transmute::<*mut libc::c_void, Mallinfo2Fn>(symbol);
        function()
    };
    reading.heap_in_use_kb = Some(info.uordblks as u64 / 1024);
    reading.heap_free_kb = Some(info.fordblks as u64 / 1024);
    reading.heap_mmap_kb = Some(info.hblkhd as u64 / 1024);
}

#[cfg(all(
    any(target_os = "linux", target_os = "android"),
    not(all(target_os = "linux", target_env = "gnu"))
))]
fn apply_allocator(_reading: &mut ProcessReading) {}

/// 增长告警的判定结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GrowthAlert {
    pub(crate) growth_kb: u64,
    pub(crate) window_secs: u64,
}

/// 窗口内相对最低点的增长判定；只依赖传入的时间与数值，便于用合成序列测试。
pub(crate) struct GrowthWatch {
    samples: VecDeque<(u64, u64)>,
    last_alert_secs: Option<u64>,
}

impl GrowthWatch {
    pub(crate) fn new() -> Self {
        Self {
            samples: VecDeque::new(),
            last_alert_secs: None,
        }
    }

    /// 记录 `now_secs` 时刻的匿名驻留（KiB），超过阈值且不在冷却期内时返回告警。
    pub(crate) fn observe(&mut self, now_secs: u64, anon_kb: u64) -> Option<GrowthAlert> {
        while self
            .samples
            .front()
            .is_some_and(|(at, _)| now_secs.saturating_sub(*at) > GROWTH_WINDOW_SECS)
        {
            self.samples.pop_front();
        }
        self.samples.push_back((now_secs, anon_kb));
        while self.samples.len() > MAX_WINDOW_SAMPLES {
            self.samples.pop_front();
        }
        let lowest = self.samples.iter().map(|(_, kb)| *kb).min()?;
        let growth_kb = anon_kb.saturating_sub(lowest);
        if growth_kb < GROWTH_THRESHOLD_KB {
            return None;
        }
        if self
            .last_alert_secs
            .is_some_and(|last| now_secs.saturating_sub(last) < ALERT_COOLDOWN_SECS)
        {
            return None;
        }
        self.last_alert_secs = Some(now_secs);
        Some(GrowthAlert {
            growth_kb,
            window_secs: GROWTH_WINDOW_SECS,
        })
    }
}

struct StopSignal {
    stopped: Mutex<bool>,
    wake: Condvar,
}

impl StopSignal {
    /// 等待一个周期；被停止时提前返回 `true`。
    fn wait(&self, cadence: Duration) -> bool {
        let guard = self.stopped.lock().unwrap_or_else(PoisonError::into_inner);
        let (guard, _) = self
            .wake
            .wait_timeout_while(guard, cadence, |stopped| !*stopped)
            .unwrap_or_else(PoisonError::into_inner);
        *guard
    }

    fn stop(&self) {
        *self.stopped.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.wake.notify_all();
    }
}

/// 周期记录线程的所有者；关闭时停止并等待线程退出。
pub(crate) struct ProcessCensus {
    signal: Arc<StopSignal>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl ProcessCensus {
    /// 按平台选择来源并启动；不支持的平台不启动线程，也不写记录。
    pub(crate) fn start() -> Option<Self> {
        #[cfg(feature = "test-process-release")]
        if PROBE_CENSUS_ENABLED.load(Ordering::SeqCst) {
            return Self::start_with(ProbeSource, Duration::ZERO, Duration::from_millis(10));
        }
        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            Self::start_with(ProcSource::new(), INITIAL_DELAY, CADENCE)
        }
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        {
            None
        }
    }

    pub(crate) fn start_with(
        source: impl ProcessSource,
        initial_delay: Duration,
        cadence: Duration,
    ) -> Option<Self> {
        let signal = Arc::new(StopSignal {
            stopped: Mutex::new(false),
            wake: Condvar::new(),
        });
        let worker_signal = Arc::clone(&signal);
        let thread = Builder::new()
            .name("uc-process-census".to_owned())
            .spawn(move || run(source, initial_delay, cadence, &worker_signal))
            .ok()?;
        Some(Self {
            signal,
            thread: Mutex::new(Some(thread)),
        })
    }

    pub(crate) fn stop(&self) {
        self.signal.stop();
        let thread = self
            .thread
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(thread) = thread {
            // 线程只在周期等待与一次读取之间，不持有外部资源；panic 已由线程自己结束，无需上报。
            let _ = thread.join();
        }
    }
}

fn run(
    mut source: impl ProcessSource,
    initial_delay: Duration,
    cadence: Duration,
    signal: &StopSignal,
) {
    let started = Instant::now();
    let mut watch = GrowthWatch::new();
    if signal.wait(initial_delay) {
        return;
    }
    loop {
        let reading = source.read();
        if reading.is_empty() {
            uc_info!(
                supported = false,
                "process resource census could not read any value"
            );
        } else {
            emit_census(&reading);
            if let Some(anon_kb) = reading.rss_anon_kb {
                if let Some(alert) = watch.observe(started.elapsed().as_secs(), anon_kb) {
                    emit_alert(&alert, &reading);
                }
            }
        }
        if signal.wait(cadence) {
            return;
        }
    }
}

fn emit_census(reading: &ProcessReading) {
    uc_info!(
        rss_anon_kb = reading.rss_anon_kb,
        rss_file_kb = reading.rss_file_kb,
        rss_shmem_kb = reading.rss_shmem_kb,
        vm_swap_kb = reading.vm_swap_kb,
        vm_size_kb = reading.vm_size_kb,
        threads = reading.threads,
        fd_total = reading.fd_total,
        pss_anon_kb = reading.pss_anon_kb,
        heap_in_use_kb = reading.heap_in_use_kb,
        heap_free_kb = reading.heap_free_kb,
        heap_mmap_kb = reading.heap_mmap_kb,
        "process resource census"
    );
}

fn emit_alert(alert: &GrowthAlert, reading: &ProcessReading) {
    uc_warn!(
        growth_kb = alert.growth_kb,
        window_secs = alert.window_secs,
        rss_anon_kb = reading.rss_anon_kb,
        rss_file_kb = reading.rss_file_kb,
        vm_swap_kb = reading.vm_swap_kb,
        threads = reading.threads,
        fd_total = reading.fd_total,
        pss_anon_kb = reading.pss_anon_kb,
        heap_in_use_kb = reading.heap_in_use_kb,
        heap_free_kb = reading.heap_free_kb,
        heap_mmap_kb = reading.heap_mmap_kb,
        "process anonymous memory grew beyond the alert threshold"
    );
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    const STATUS: &str = "Name:\tuniclipd\nVmSize:\t35046296 kB\nVmRSS:\t127928 kB\nRssAnon:\t84228 kB\nRssFile:\t43700 kB\nRssShmem:\t0 kB\nVmSwap:\t12 kB\nThreads:\t51\n";

    #[test]
    fn status_values_are_parsed_and_missing_keys_stay_absent() {
        let mut reading = ProcessReading::default();
        apply_status(STATUS, &mut reading);
        assert_eq!(reading.rss_anon_kb, Some(84228));
        assert_eq!(reading.rss_file_kb, Some(43700));
        assert_eq!(reading.rss_shmem_kb, Some(0));
        assert_eq!(reading.vm_swap_kb, Some(12));
        assert_eq!(reading.vm_size_kb, Some(35046296));
        assert_eq!(reading.threads, Some(51));

        let mut partial = ProcessReading::default();
        apply_status("Name:\tx\nRssAnon:\tnot-a-number kB\n", &mut partial);
        assert!(
            partial.is_empty(),
            "unparsable or missing values are omitted, not zero"
        );
    }

    #[test]
    fn rollup_anonymous_pss_is_read() {
        let rollup = "Rss:  127928 kB\nPss:  124465 kB\nPss_Anon:  84228 kB\nPss_File: 100 kB\n";
        assert_eq!(kb_field(rollup, "Pss_Anon:"), Some(84228));
        assert_eq!(kb_field(rollup, "Missing:"), None);
    }

    fn drive(watch: &mut GrowthWatch, series: &[(u64, u64)]) -> Vec<Option<GrowthAlert>> {
        series
            .iter()
            .map(|(at, kb)| watch.observe(*at, *kb))
            .collect()
    }

    #[test]
    fn flat_and_slow_growth_never_alert() {
        let mut flat = GrowthWatch::new();
        let series: Vec<_> = (0..48).map(|i| (i * 300, 84_000)).collect();
        assert!(drive(&mut flat, &series).iter().all(Option::is_none));

        // 约 100 MiB/小时：每个窗口只增长约 100 MiB，低于阈值。
        let mut slow = GrowthWatch::new();
        let series: Vec<_> = (0..48).map(|i| (i * 300, 84_000 + i * 8_500)).collect();
        assert!(drive(&mut slow, &series).iter().all(Option::is_none));
    }

    #[test]
    fn reported_leak_rate_alerts_once_within_the_window_and_respects_cooldown() {
        // 约 740 MB/h：每 5 分钟约 63 MiB，1 小时约 750 MiB，窗口内越过 512 MiB（第 9 个样本），之后每个冷却期一次。
        let mut watch = GrowthWatch::new();
        let mut alerts = Vec::new();
        for i in 0..40u64 {
            if let Some(alert) = watch.observe(i * 300, 84_000 + i * 64_000) {
                alerts.push((i * 300, alert));
            }
        }
        assert_eq!(
            alerts.len(),
            3,
            "one per cooldown hour over 200 minutes: {alerts:?}"
        );
        assert!(alerts[0].1.growth_kb >= GROWTH_THRESHOLD_KB);
        assert_eq!(alerts[0].1.window_secs, GROWTH_WINDOW_SECS);
        assert!(alerts[1].0 - alerts[0].0 >= ALERT_COOLDOWN_SECS);
    }

    #[test]
    fn a_step_alerts_and_a_sawtooth_that_falls_back_does_not_repeat() {
        let mut watch = GrowthWatch::new();
        let mut series = vec![(0, 100_000), (300, 100_000), (600, 700_000)];
        // 回落后在窗口内再次升高但仍在冷却期内。
        series.extend([(900, 100_000), (1200, 700_000)]);
        let alerts = drive(&mut watch, &series);
        assert!(alerts[2].is_some());
        assert!(alerts[4].is_none(), "cooldown suppresses the second rise");
    }

    #[test]
    fn a_decrease_such_as_after_a_restart_never_alerts_and_old_samples_expire() {
        let mut watch = GrowthWatch::new();
        assert!(watch.observe(0, 900_000).is_none());
        assert!(watch.observe(300, 80_000).is_none());
        // 两小时后窗口已不含旧的高点；从低点缓慢爬升不触发。
        assert!(watch.observe(7_500, 80_000).is_none());
        assert!(watch.observe(7_800, 90_000).is_none());
    }

    #[test]
    fn the_window_never_holds_more_than_the_sample_cap() {
        let mut watch = GrowthWatch::new();
        for i in 0..1_000u64 {
            watch.observe(i, 1);
        }
        assert!(watch.samples.len() <= MAX_WINDOW_SAMPLES);
    }

    struct CountingSource(Arc<AtomicUsize>);

    impl ProcessSource for CountingSource {
        fn read(&mut self) -> ProcessReading {
            self.0.fetch_add(1, Ordering::SeqCst);
            ProcessReading {
                rss_anon_kb: Some(1),
                ..ProcessReading::default()
            }
        }
    }

    #[test]
    fn the_sampler_reads_on_its_cadence_and_stops_promptly() {
        let reads = Arc::new(AtomicUsize::new(0));
        let census = ProcessCensus::start_with(
            CountingSource(Arc::clone(&reads)),
            Duration::ZERO,
            Duration::from_millis(10),
        )
        .expect("thread starts");
        let deadline = Instant::now() + Duration::from_secs(5);
        while reads.load(Ordering::SeqCst) < 3 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(reads.load(Ordering::SeqCst) >= 3);

        let stopping = Instant::now();
        census.stop();
        assert!(stopping.elapsed() < Duration::from_secs(2));
        let after_stop = reads.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(
            reads.load(Ordering::SeqCst),
            after_stop,
            "no reads after stop"
        );
        census.stop();
    }

    #[test]
    fn a_long_initial_delay_does_not_delay_stop() {
        let reads = Arc::new(AtomicUsize::new(0));
        let census = ProcessCensus::start_with(
            CountingSource(Arc::clone(&reads)),
            Duration::from_secs(3600),
            Duration::from_secs(3600),
        )
        .expect("thread starts");
        let stopping = Instant::now();
        census.stop();
        assert!(stopping.elapsed() < Duration::from_secs(2));
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn the_real_source_reads_this_process() {
        let reading = ProcSource::new().read();
        assert!(reading.rss_anon_kb.is_some_and(|kb| kb > 0));
        assert!(reading.threads.is_some_and(|count| count >= 1));
        assert!(reading.fd_total.is_some_and(|count| count >= 3));
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        {
            // 测试环境的 glibc 必须提供 `mallinfo2`，否则堆字段静默缺失会被误认为来源正常。
            assert!(reading.heap_in_use_kb.is_some_and(|kb| kb > 0));
            assert!(reading.heap_free_kb.is_some());
            assert!(reading.heap_mmap_kb.is_some());
        }
    }

    #[derive(Default, Clone)]
    struct Captured(Arc<Mutex<Vec<(tracing::Level, Vec<(String, String)>)>>>);

    struct FieldVisitor(Vec<(String, String)>);

    impl tracing::field::Visit for FieldVisitor {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            self.0.push((field.name().to_owned(), format!("{value:?}")));
        }
    }

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Captured {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _: tracing_subscriber::layer::Context<'_, S>,
        ) {
            let mut visitor = FieldVisitor(Vec::new());
            event.record(&mut visitor);
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((*event.metadata().level(), visitor.0));
        }
    }

    #[test]
    fn records_carry_only_the_planned_numeric_fields_and_omit_unread_values() {
        use tracing_subscriber::layer::SubscriberExt;

        let captured = Captured::default();
        let subscriber = tracing_subscriber::registry().with(captured.clone());
        tracing::subscriber::with_default(subscriber, || {
            let reading = ProcessReading {
                rss_anon_kb: Some(84_228),
                threads: Some(51),
                heap_in_use_kb: Some(70_000),
                ..ProcessReading::default()
            };
            emit_census(&reading);
            emit_alert(
                &GrowthAlert {
                    growth_kb: 600_000,
                    window_secs: GROWTH_WINDOW_SECS,
                },
                &reading,
            );
        });
        let records = captured.0.lock().unwrap_or_else(PoisonError::into_inner);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].0, tracing::Level::INFO);
        assert_eq!(records[1].0, tracing::Level::WARN);
        let names = |index: usize| -> Vec<&str> {
            records[index]
                .1
                .iter()
                .map(|(name, _)| name.as_str())
                .collect()
        };
        assert_eq!(
            names(0),
            ["message", "rss_anon_kb", "threads", "heap_in_use_kb"]
        );
        assert_eq!(
            names(1),
            [
                "message",
                "growth_kb",
                "window_secs",
                "rss_anon_kb",
                "threads",
                "heap_in_use_kb"
            ]
        );
        assert!(records[1]
            .1
            .iter()
            .any(|(name, value)| name == "growth_kb" && value == "600000"));
    }
}
