//! 移动端后台暂停的文件交接验收：暂停成功后，本进程不得再持有 Engine 自有的文件锁或 SQLite 连接，
//! 恢复后资料与网络日志照常工作。iOS 在进程挂起时若发现共享容器内仍有被持有的锁，会以
//! `0xdead10cc` 终止应用；这里在桌面进程上用真实的绑定入口、真实目录和独立进程复现同一检查。
//!
//! 检查方式刻意避开假结果：
//! - 本进程打开的文件只按已有描述符枚举（不新开 SQLite 文件）；关闭任何指向同一文件的描述符会让本进程的
//!   POSIX 锁全部失效，探测本身就会制造“已释放”。
//! - SQLite 的 POSIX 锁由子进程用 `F_GETLK` 查询；flock 类租约在本进程用独立描述符试锁即可。
//! - 另一进程以超额预算回收网络日志目录，另一进程的 Engine 在暂停期间写入资料，恢复后由本进程读回。
//!
//! 独立测试二进制：进程级观测每个进程只能安装一次。工件写入 `UC_TEST_ARTIFACTS_DIR`
//! （默认 `target/test-artifacts/suspension-releases-files`）。只在能枚举描述符路径的 macOS 与 Linux 上编译。
#![cfg(any(target_os = "macos", target_os = "linux"))]

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use uc_engine_uniffi::{
    configure_network_log, install_process_observability, prepare_local_diagnostic_export,
    prepare_network_log_export, BindingClipboardSnapshot, BindingConfig,
    BindingDeploymentEnvironment, BindingEngineState, BindingError, BindingErrorCategory,
    BindingFileMetadata, BindingHost, BindingNetworkLogConfig, BindingNetworkProcessRole,
    BindingObservabilityConfig, HostBindingError, MobileEngine,
};

const PASSPHRASE: &str = "correct horse battery staple";
const PROFILE_ID: &str = "suspension-releases-files";
const SHUTDOWN_DEADLINE_MS: u64 = 30_000;
const SUSPEND_DEADLINE_MS: u64 = 30_000;
const CHILD_MODE: &str = "UC_SUSPENSION_CHILD";
const CHILD_ROOT: &str = "UC_SUSPENSION_ROOT";
const CHILD_ENTRY: &str = "child_process_entry";
const PARENT_TEXT_BEFORE: &str = "parent text before suspension";
const PARENT_TEXT_AFTER: &str = "parent text after resume";
const CHILD_TEXT: &str = "child text written while the parent was suspended";

/// 宿主目录与可跨进程共享的安全存储：值以文件保存，子进程用同一根目录即可读到本进程写入的密钥。
struct FileHost {
    root: PathBuf,
    writes: Mutex<HashMap<String, File>>,
}

impl FileHost {
    fn new(root: &Path) -> Self {
        Self {
            root: root.to_owned(),
            writes: Mutex::new(HashMap::new()),
        }
    }

    fn private_data(&self) -> PathBuf {
        self.root.join("data")
    }

    fn secure_path(&self, key: &str) -> PathBuf {
        let name: String = key.bytes().map(|byte| format!("{byte:02x}")).collect();
        self.root.join("secure").join(name)
    }

    fn export_path(&self, handle: &str) -> PathBuf {
        self.root.join("exports").join(handle)
    }
}

impl BindingHost for FileHost {
    fn private_data_directory(&self) -> Result<String, HostBindingError> {
        Ok(self.private_data().to_string_lossy().into_owned())
    }

    fn cache_directory(&self) -> Result<String, HostBindingError> {
        Ok(self.root.join("cache").to_string_lossy().into_owned())
    }

    fn temporary_directory(&self) -> Result<String, HostBindingError> {
        Ok(self.root.join("temporary").to_string_lossy().into_owned())
    }

    fn secure_storage_get(&self, key: String) -> Result<Option<Vec<u8>>, HostBindingError> {
        match fs::read(self.secure_path(&key)) {
            Ok(value) => Ok(Some(value)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(HostBindingError::Io),
        }
    }

    fn secure_storage_set(&self, key: String, value: Vec<u8>) -> Result<(), HostBindingError> {
        let path = self.secure_path(&key);
        fs::create_dir_all(path.parent().ok_or(HostBindingError::Io)?)
            .map_err(|_| HostBindingError::Io)?;
        fs::write(path, value).map_err(|_| HostBindingError::Io)
    }

    fn secure_storage_delete(&self, key: String) -> Result<(), HostBindingError> {
        match fs::remove_file(self.secure_path(&key)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(HostBindingError::Io),
        }
    }

    fn file_metadata(&self, _handle: String) -> Result<BindingFileMetadata, HostBindingError> {
        Err(HostBindingError::InvalidHandle)
    }

    fn file_read_chunk(
        &self,
        _handle: String,
        _offset: u64,
        _max_bytes: u32,
    ) -> Result<Vec<u8>, HostBindingError> {
        Err(HostBindingError::InvalidHandle)
    }

    fn file_write_chunk(
        &self,
        handle: String,
        offset: u64,
        bytes: Vec<u8>,
    ) -> Result<(), HostBindingError> {
        let mut writes = self
            .writes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !writes.contains_key(&handle) {
            let path = self.export_path(&handle);
            fs::create_dir_all(path.parent().ok_or(HostBindingError::Io)?)
                .map_err(|_| HostBindingError::Io)?;
            let file = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(path)
                .map_err(|_| HostBindingError::Io)?;
            writes.insert(handle.clone(), file);
        }
        let file = writes.get_mut(&handle).ok_or(HostBindingError::Io)?;
        file.seek(SeekFrom::Start(offset))
            .map_err(|_| HostBindingError::Io)?;
        file.write_all(&bytes).map_err(|_| HostBindingError::Io)
    }

    fn file_finish_write(&self, handle: String) -> Result<(), HostBindingError> {
        self.writes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&handle);
        Ok(())
    }

    fn clipboard_read(&self) -> Result<BindingClipboardSnapshot, HostBindingError> {
        Ok(BindingClipboardSnapshot {
            observed_at_ms: 0,
            representations: Vec::new(),
        })
    }

    fn clipboard_write(&self, _snapshot: BindingClipboardSnapshot) -> Result<(), HostBindingError> {
        Ok(())
    }
}

fn config() -> BindingConfig {
    BindingConfig {
        app_version: "1.2.3".to_owned(),
        profile_id: PROFILE_ID.to_owned(),
    }
}

fn observability() -> BindingObservabilityConfig {
    BindingObservabilityConfig {
        service_version: "1.2.3".to_owned(),
        environment: BindingDeploymentEnvironment::Test,
        app_channel: "test".to_owned(),
        remote_diagnostics_enabled: false,
        collector: None,
    }
}

fn network_root(host: &FileHost) -> PathBuf {
    host.private_data()
        .join("logs")
        .join("network")
        .join("mobile_app")
}

/// 可能被加锁的文件：SQLite 主库及其 WAL/共享内存、redb、租约与锁文件，以及网络日志目录中的全部文件。
/// Engine 的日文件在进程内持续打开但从不加锁，不在此列。
fn lock_capable(path: &Path, network: &Path) -> bool {
    if path.starts_with(network) {
        return true;
    }
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    name.ends_with(".sqlite")
        || name.ends_with("-wal")
        || name.ends_with("-shm")
        || name.ends_with(".db")
        || name.ends_with(".lock")
        || name.ends_with(".lease")
        || name == ".lease"
}

/// 本进程当前打开、位于测试根目录内且可能被加锁的文件；只读取已有描述符，不新开任何文件。
fn open_lock_capable_files(root: &Path, network: &Path) -> Vec<PathBuf> {
    open_paths()
        .into_iter()
        .filter(|path| path.starts_with(root) && lock_capable(path, network))
        .collect()
}

#[cfg(target_os = "macos")]
fn open_paths() -> Vec<PathBuf> {
    use std::ffi::CStr;
    use std::os::unix::ffi::OsStrExt;
    let limit = unsafe { libc::getdtablesize() };
    let mut paths = Vec::new();
    for fd in 0..limit {
        let mut buffer = [0 as libc::c_char; libc::PATH_MAX as usize];
        // SAFETY: F_GETPATH 写入不超过 MAXPATHLEN 字节的以 NUL 结尾路径；无效描述符只返回错误。
        if unsafe { libc::fcntl(fd, libc::F_GETPATH, buffer.as_mut_ptr()) } == 0 {
            let path = unsafe { CStr::from_ptr(buffer.as_ptr()) };
            paths.push(PathBuf::from(std::ffi::OsStr::from_bytes(path.to_bytes())));
        }
    }
    paths
}

#[cfg(target_os = "linux")]
fn open_paths() -> Vec<PathBuf> {
    fs::read_dir("/proc/self/fd")
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter_map(|entry| fs::read_link(entry.path()).ok())
                .collect()
        })
        .unwrap_or_default()
}

fn files_under(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_owned()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => pending.push(path),
                Ok(kind) if kind.is_file() => files.push(path),
                _ => {}
            }
        }
    }
    files.sort();
    files
}

fn is_sqlite(path: &Path) -> bool {
    path.to_string_lossy().contains(".sqlite")
}

/// flock 类锁：以独立描述符试锁；被本进程其他描述符持有同样返回忙。SQLite 文件不在本进程打开。
fn held_flocks(root: &Path, network: &Path) -> Vec<PathBuf> {
    files_under(root)
        .into_iter()
        .filter(|path| lock_capable(path, network) && !is_sqlite(path))
        .filter(|path| {
            let Ok(file) = OpenOptions::new().read(true).write(true).open(path) else {
                return false;
            };
            file.try_lock().is_err()
        })
        .collect()
}

/// 由子进程查询 SQLite 文件上的 POSIX 锁（任何字节范围的读/写锁），返回持锁文件的相对路径。
fn held_sqlite_locks(root: &Path) -> Vec<String> {
    let output = child(root, "posix-probe");
    String::from_utf8_lossy(&output)
        .lines()
        .filter_map(|line| marked(line, "LOCKED "))
        .collect()
}

fn posix_lock_owner(file: &File, start: i64, length: i64) -> Option<libc::pid_t> {
    use std::os::fd::AsRawFd;
    let mut probe: libc::flock = unsafe { std::mem::zeroed() };
    probe.l_type = libc::F_WRLCK as libc::c_short;
    probe.l_whence = libc::SEEK_SET as libc::c_short;
    probe.l_start = start;
    probe.l_len = length;
    // SAFETY: F_GETLK 只读写传入的 flock 结构；不取得任何锁。
    let result = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETLK, &mut probe) };
    (result == 0 && probe.l_type != libc::F_UNLCK as libc::c_short).then_some(probe.l_pid)
}

/// libtest 会在子进程第一行输出前打印 `test <name> ... `，标记可能不在行首。
fn marked(line: &str, marker: &str) -> Option<String> {
    line.split_once(marker)
        .map(|(_, value)| value.trim().to_owned())
}

fn child(root: &Path, mode: &str) -> Vec<u8> {
    let output = Command::new(std::env::current_exe().expect("test binary"))
        .args(["--exact", CHILD_ENTRY, "--nocapture", "--test-threads=1"])
        .env(CHILD_MODE, mode)
        .env(CHILD_ROOT, root)
        .output()
        .expect("child process");
    assert!(
        output.status.success(),
        "child {mode} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

/// 子进程入口；父进程未设置模式时直接返回。
#[test]
fn child_process_entry() {
    let Ok(mode) = std::env::var(CHILD_MODE) else {
        return;
    };
    let root = PathBuf::from(std::env::var(CHILD_ROOT).expect("child root"));
    match mode.as_str() {
        "posix-probe" => {
            for path in files_under(&root)
                .into_iter()
                .filter(|path| is_sqlite(path))
            {
                let file = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&path)
                    .expect("sqlite file");
                // 整个文件范围：包含主库锁字节与共享内存的读写、检查点、恢复和 DMS 锁。
                if posix_lock_owner(&file, 0, 0).is_some() {
                    let relative = path.strip_prefix(&root).expect("under root");
                    println!("LOCKED {}", relative.display());
                }
            }
        }
        "cleaner" => {
            // 另一进程以极小预算安装网络日志：打开自己的运行目录时回收所有未被租约保护的运行。
            configure_network_log(BindingNetworkLogConfig {
                role: BindingNetworkProcessRole::MobileApp,
                record_bytes: 512,
                queue_records: 16,
                file_bytes: 512,
                run_bytes: 512,
                root_bytes: 512,
                retention_seconds: 1,
            })
            .expect("cleaner network log");
            let host = Arc::new(FileHost::new(&root));
            install_process_observability(observability(), host).expect("cleaner observability");
            let started = Instant::now();
            while started.elapsed() < Duration::from_secs(10) {
                if prepare_network_log_export(1_000)
                    .map(|report| !report.run_directory.is_empty())
                    .unwrap_or(false)
                {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        "handoff" => {
            // 宿主扩展的交接：父进程暂停后，另一进程的 Engine 取得同一资料并写入。
            let host = Arc::new(FileHost::new(&root));
            let engine = MobileEngine::start(config(), host).expect("child engine start");
            let recovery = engine
                .recover_session(true)
                .expect("child session recovery");
            assert!(recovery.unlocked, "child must unlock the shared profile");
            let report = engine
                .send_text(CHILD_TEXT.to_owned(), Vec::new())
                .expect("child write");
            println!("ENTRY {}", report.entry_id);
            engine
                .shutdown(SHUTDOWN_DEADLINE_MS)
                .expect("child engine shutdown");
        }
        other => panic!("unknown child mode {other}"),
    }
}

fn run_manifest(directory: &Path) -> String {
    fs::read_to_string(directory.join("run.json")).unwrap_or_default()
}

fn reopened(directory: &Path) -> Option<u64> {
    let manifest = run_manifest(directory);
    let rest = manifest.split("\"reopened\":").nth(1)?;
    rest.trim_start()
        .split(|character: char| !character.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

/// 每一步立即追加到工件文件：失败的运行同样留下到失败点为止的证据。
struct Evidence {
    file: File,
}

impl Evidence {
    fn create() -> Self {
        let directory = std::env::var_os("UC_TEST_ARTIFACTS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../target/test-artifacts/suspension-releases-files")
            });
        fs::create_dir_all(&directory).expect("artifact directory");
        let file = File::create(directory.join("evidence.tsv")).expect("artifact file");
        Self { file }
    }

    fn record(&mut self, step: &str, value: impl std::fmt::Display) {
        println!("[evidence] {step}: {value}");
        writeln!(self.file, "{step}\t{value}").expect("artifact write");
    }
}

fn names(paths: &[PathBuf], root: &Path) -> String {
    let relative: Vec<_> = paths
        .iter()
        .map(|path| {
            path.strip_prefix(root)
                .unwrap_or(path)
                .display()
                .to_string()
        })
        .collect();
    format!("{relative:?}")
}

struct Probe {
    open: Vec<PathBuf>,
    flocks: Vec<PathBuf>,
    sqlite: Vec<String>,
}

fn probe(root: &Path, network: &Path) -> Probe {
    Probe {
        open: open_lock_capable_files(root, network),
        flocks: held_flocks(root, network),
        sqlite: held_sqlite_locks(root),
    }
}

fn record_probe(evidence: &mut Evidence, label: &str, probe: &Probe, root: &Path) {
    evidence.record(&format!("{label}.open_files"), names(&probe.open, root));
    evidence.record(&format!("{label}.held_flocks"), names(&probe.flocks, root));
    evidence.record(
        &format!("{label}.sqlite_locks"),
        format!("{:?}", probe.sqlite),
    );
}

fn assert_released(probe: &Probe, root: &Path, label: &str) {
    assert!(
        probe.open.is_empty(),
        "{label}: lock-capable files still open: {}",
        names(&probe.open, root)
    );
    assert!(
        probe.flocks.is_empty(),
        "{label}: file locks still held: {}",
        names(&probe.flocks, root)
    );
    assert!(
        probe.sqlite.is_empty(),
        "{label}: SQLite locks still held: {:?}",
        probe.sqlite
    );
}

fn run_directories(network: &Path) -> BTreeMap<PathBuf, bool> {
    fs::read_dir(network)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                .map(|entry| (entry.path(), true))
                .collect()
        })
        .unwrap_or_default()
}

/// Engine 在发布暂停前写下的核对记录（只含计数）。返回每条记录的两个计数，按写入顺序。
fn suspend_file_records(host: &FileHost) -> Vec<String> {
    prepare_local_diagnostic_export(1_000).expect("local diagnostic flush");
    let mut records = Vec::new();
    for path in files_under(&host.private_data().join("logs")) {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if !(name.starts_with("engine.") && name.ends_with(".jsonl")) {
            continue;
        }
        for line in fs::read_to_string(&path).unwrap_or_default().lines() {
            if !line.contains("\"message\":\"engine suspended with local files released\"")
                && !line.contains("\"message\":\"engine suspended while local files remain open\"")
            {
                continue;
            }
            let fields = line
                .split("\"fields\":")
                .nth(1)
                .and_then(|rest| rest.split('}').next())
                .unwrap_or_default();
            records.push(format!("{fields}}}"));
        }
    }
    records
}

fn read_export(host: &FileHost, engine: &MobileEngine, entry_id: &str, handle: &str) -> String {
    engine
        .export_entry(entry_id.to_owned(), handle.to_owned())
        .expect("export entry");
    let mut text = String::new();
    File::open(host.export_path(handle))
        .expect("exported file")
        .read_to_string(&mut text)
        .expect("exported text");
    text
}

#[test]
fn suspended_engine_releases_every_owned_file_and_resumes() {
    if std::env::var_os(CHILD_MODE).is_some() {
        return;
    }
    let directory = tempfile::tempdir().expect("temporary host root");
    // macOS 的描述符路径是解析后的真实路径（/private/var/...）；以同一形式比较。
    let root = directory.path().canonicalize().expect("canonical root");
    let host = Arc::new(FileHost::new(&root));
    let network = network_root(&host);
    let mut evidence = Evidence::create();

    configure_network_log(BindingNetworkLogConfig {
        role: BindingNetworkProcessRole::MobileApp,
        record_bytes: 4_096,
        queue_records: 1_024,
        file_bytes: 1 << 20,
        run_bytes: 4 << 20,
        root_bytes: 64 << 20,
        retention_seconds: 7 * 24 * 3_600,
    })
    .expect("network log configuration");
    install_process_observability(observability(), host.clone()).expect("observability install");

    let started = Instant::now();
    let engine = MobileEngine::start(config(), host.clone()).expect("engine start");
    engine
        .create_space(Some("suspension".to_owned()), PASSPHRASE.to_owned())
        .expect("create space");
    let before = engine
        .send_text(PARENT_TEXT_BEFORE.to_owned(), Vec::new())
        .expect("write before suspension");
    let export = prepare_network_log_export(1_000).expect("network log export");
    let run = PathBuf::from(&export.run_directory);
    evidence.record("run_directory_exists_before", run.is_dir());

    // 探测本身必须能看见持有状态，否则下面的“已释放”没有意义。
    let running = probe(&root, &network);
    record_probe(&mut evidence, "running", &running, &root);
    assert!(
        !running.open.is_empty() && !running.flocks.is_empty() && !running.sqlite.is_empty(),
        "probes must observe the running engine's files"
    );

    // 启动后不久即进入后台：对应移动端刚启动就切走的情形。
    let since_start = started.elapsed();
    let suspend_started = Instant::now();
    engine
        .suspend_with_deadline(SUSPEND_DEADLINE_MS)
        .expect("first suspend");
    evidence.record("first_suspend.since_start_ms", since_start.as_millis());
    evidence.record(
        "first_suspend.duration_ms",
        suspend_started.elapsed().as_millis(),
    );
    assert_eq!(
        engine.lifecycle_state().expect("state"),
        BindingEngineState::Suspended
    );
    let suspended = probe(&root, &network);
    record_probe(&mut evidence, "first_suspend", &suspended, &root);
    assert_released(&suspended, &root, "first suspend");
    let records = suspend_file_records(&host);
    evidence.record("first_suspend.engine_record", format!("{records:?}"));
    assert_eq!(
        records,
        ["{\"open_database_files\":0,\"open_lease_files\":0}"],
        "the suspend record reports zero open database and lease files"
    );

    // 另一进程超额回收：暂停中的运行没有租约保护，可以被删除，恢复后由写入方重建。
    child(&root, "cleaner");
    let parent_run_after_cleanup = run.is_dir();
    evidence.record(
        "run_directory_exists_after_cleaner",
        parent_run_after_cleanup,
    );

    // 另一进程的 Engine 在暂停期间取得同一资料并写入。
    let handoff = child(&root, "handoff");
    let child_entry = String::from_utf8_lossy(&handoff)
        .lines()
        .find_map(|line| marked(line, "ENTRY "))
        .expect("child entry id");
    evidence.record("handoff.child_entry_written", true);

    let resume_started = Instant::now();
    engine.resume().expect("first resume");
    evidence.record(
        "first_resume.duration_ms",
        resume_started.elapsed().as_millis(),
    );
    assert_eq!(
        engine.lifecycle_state().expect("state"),
        BindingEngineState::Running
    );
    assert_eq!(
        read_export(&host, &engine, &child_entry, "child-entry"),
        CHILD_TEXT,
        "resume must read what another process wrote while suspended"
    );
    assert_eq!(
        read_export(&host, &engine, &before.entry_id, "parent-before"),
        PARENT_TEXT_BEFORE
    );
    let after = engine
        .send_text(PARENT_TEXT_AFTER.to_owned(), Vec::new())
        .expect("write after resume");
    engine.list_devices().expect("control database query");

    let export = prepare_network_log_export(1_000).expect("network log export after resume");
    let resumed_run = PathBuf::from(&export.run_directory);
    assert_eq!(resumed_run, run, "resume keeps the same run");
    evidence.record("first_resume.run_directory_exists", run.is_dir());
    evidence.record("first_resume.reopened", format!("{:?}", reopened(&run)));
    assert!(run.is_dir(), "the writer recreates its run after resume");
    assert_eq!(reopened(&run), Some(1), "first resume reopens the run once");
    assert!(
        held_flocks(&root, &network)
            .iter()
            .any(|path| path.starts_with(&run)),
        "the resumed writer holds its run lease again"
    );

    // 第二轮：重复暂停与恢复不能残留或叠加。
    engine
        .suspend_with_deadline(SUSPEND_DEADLINE_MS)
        .expect("second suspend");
    let second = probe(&root, &network);
    record_probe(&mut evidence, "second_suspend", &second, &root);
    assert_released(&second, &root, "second suspend");
    let records = suspend_file_records(&host);
    evidence.record("second_suspend.engine_record", format!("{records:?}"));
    assert_eq!(records.len(), 2, "one record per successful suspend");
    engine.resume().expect("second resume");
    assert_eq!(
        read_export(&host, &engine, &after.entry_id, "parent-after"),
        PARENT_TEXT_AFTER
    );
    prepare_network_log_export(1_000).expect("network log export after second resume");
    evidence.record("second_resume.reopened", format!("{:?}", reopened(&run)));
    assert_eq!(
        reopened(&run),
        Some(2),
        "second resume reopens the run again"
    );

    // 第三轮：恢复后网络刚重建即以极短期限暂停。期限只结束宿主等待；已接受的暂停在收尾后自行发布
    // Suspended，宿主只查询状态、不再发出第二次请求。
    let deadline_started = Instant::now();
    let short = engine.suspend_with_deadline(1);
    let host_wait = deadline_started.elapsed();
    let deadline_exceeded = matches!(
        short,
        Err(BindingError::Engine {
            category: BindingErrorCategory::DeadlineExceeded,
            ..
        })
    );
    assert!(
        short.is_ok() || deadline_exceeded,
        "a short deadline either completes or reports DeadlineExceeded: {short:?}"
    );
    evidence.record("deadline_suspend.deadline_exceeded", deadline_exceeded);
    evidence.record("deadline_suspend.host_wait_ms", host_wait.as_millis());
    while engine.lifecycle_state().expect("state") != BindingEngineState::Suspended {
        assert!(
            deadline_started.elapsed() < Duration::from_secs(20),
            "the accepted suspend must finish on its own"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    evidence.record(
        "deadline_suspend.suspended_after_ms",
        deadline_started.elapsed().as_millis(),
    );
    let third = probe(&root, &network);
    record_probe(&mut evidence, "deadline_suspend", &third, &root);
    assert_released(&third, &root, "suspend completed after the host deadline");
    assert_eq!(suspend_file_records(&host).len(), 3);
    engine.resume().expect("resume after deadline suspend");
    engine
        .send_text("after deadline".to_owned(), Vec::new())
        .expect("write after deadline suspend");
    evidence.record("run_directories", run_directories(&network).len());

    use uc_engine_uniffi::BindingResourceRelease;
    let receipt = engine.shutdown_with_outcome(SHUTDOWN_DEADLINE_MS);
    assert_eq!(receipt, BindingResourceRelease::Released { error: None });
    assert_eq!(engine.shutdown_with_outcome(SHUTDOWN_DEADLINE_MS), receipt);
    assert_eq!(engine.suspend_with_outcome(SUSPEND_DEADLINE_MS), receipt);
    let final_probe = probe(&root, &network);
    record_probe(&mut evidence, "final_shutdown", &final_probe, &root);
    assert_released(&final_probe, &root, "final shutdown receipt");
    evidence.record("release_receipt.repeated", "released");
    evidence.record("outcome", "passed");
}
