//! 同一 profile 的宿主进程先后或重叠启动时，启动存储检查对 SQLite 锁争用与真实损坏的分类。
//!
//! 每个场景都用独立宿主进程打开同一个合成 profile：第一个进程创建 Space 后关闭，第二个进程
//! 启动时执行首次升级恢复检查（journal 仍处于 Verified）。场景结束后把启动结果与脱敏诊断写入
//! `UC_STARTUP_OVERLAP_ARTIFACT_DIR`（默认 `target/startup-overlap`），供前后对比与复核。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use diesel::connection::SimpleConnection as _;
use diesel::{Connection as _, SqliteConnection};
use serde_json::{json, Value};
use tempfile::TempDir;

#[path = "common/process.rs"]
#[allow(dead_code)]
mod process;
use process::{mount_rendezvous, ProfileHost};

const JOURNAL: &str = "profile-storage-upgrade/.journal-v1";

/// 前一个进程正在关闭（连接池释放、WAL 检查点）时启动下一个进程；启动不得报告资料损坏。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn startup_overlapping_a_closing_host_is_never_reported_as_corrupt() {
    let rendezvous = mount_rendezvous().await;
    let iterations = env_number("UC_STARTUP_OVERLAP_ITERATIONS", 12);
    let delays = [0_u64, 10, 25, 50, 100, 200];
    let mut records = Vec::new();
    for iteration in 0..iterations {
        let root = TempDir::new().expect("profile root");
        let (mut first, storage) = create_space(root.path(), &rendezvous.uri());
        let delay = delays[iteration as usize % delays.len()];
        let shutdown_sent = Instant::now();
        first.send(json!({ "command": "shutdown" }));
        std::thread::sleep(Duration::from_millis(delay));
        let (second, ready) = ProfileHost::launch(root.path(), &rendezvous.uri(), Some(storage));
        let started_after_ms = shutdown_sent.elapsed().as_millis();
        let first_reply = first.reply();
        let first_exit = first.wait_exit();
        let second_exit = stop(second, &ready);
        records.push(json!({
            "iteration": iteration,
            "delay_ms": delay,
            "second_started_after_shutdown_ms": started_after_ms,
            "first_shutdown_ok": first_reply.get("ok").is_some(),
            "first_exit_success": first_exit.success(),
            "second": startup_result(&ready),
            "second_exit_success": second_exit,
            "upgrade_failures": upgrade_failures(root.path()),
        }));
    }
    let corrupt = records
        .iter()
        .filter(|record| record["second"]["failure"] == "corrupt_data")
        .count();
    write_artifact(
        "overlap",
        json!({ "iterations": iterations, "corrupt": corrupt, "records": records }),
    );
    assert_eq!(corrupt, 0, "overlapping startup reported corrupt data");
}

/// 另一连接短暂独占运行库时，启动等待锁释放后继续，而不是立即失败。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn startup_waits_for_a_briefly_held_runtime_database() {
    let rendezvous = mount_rendezvous().await;
    let root = TempDir::new().expect("profile root");
    let storage = create_stopped_space(root.path(), &rendezvous.uri());
    let holder = hold_exclusive(&control_database(root.path()));
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(1_500));
        drop(holder);
    });
    let started = Instant::now();
    let (second, ready) = ProfileHost::launch(root.path(), &rendezvous.uri(), Some(storage));
    let startup_ms = started.elapsed().as_millis();
    release.join().expect("release holder");
    let failures = upgrade_failures(root.path());
    let exit = stop(second, &ready);
    write_artifact(
        "brief-lock",
        json!({
            "held_ms": 1_500,
            "startup_ms": startup_ms,
            "second": startup_result(&ready),
            "second_exit_success": exit,
            "upgrade_failures": failures,
        }),
    );
    assert_eq!(ready["ready"], true, "startup did not wait for the lock");
}

/// 锁持有超过等待上限时，启动以可重试的存储不可用失败，journal 不前进；锁释放后重启成功。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn startup_reports_a_long_held_runtime_database_as_retryable_and_recovers() {
    let rendezvous = mount_rendezvous().await;
    let root = TempDir::new().expect("profile root");
    let storage = create_stopped_space(root.path(), &rendezvous.uri());
    let journal_before = std::fs::read(root.path().join("private").join(JOURNAL))
        .expect("verified journal remains after Space creation");
    let holder = hold_exclusive(&control_database(root.path()));
    let (blocked, blocked_ready) =
        ProfileHost::launch(root.path(), &rendezvous.uri(), Some(storage.clone()));
    let blocked_exit = stop(blocked, &blocked_ready);
    let journal_after_failure = std::fs::read(root.path().join("private").join(JOURNAL)).ok();
    let blocked_failures = upgrade_failures(root.path());
    drop(holder);
    let storage = blocked_ready
        .get("secure_storage")
        .cloned()
        .unwrap_or(storage);
    let (recovered, recovered_ready) =
        ProfileHost::launch(root.path(), &rendezvous.uri(), Some(storage));
    let recovered_exit = stop(recovered, &recovered_ready);
    let journal_after_recovery = root.path().join("private").join(JOURNAL).exists();
    write_artifact(
        "long-lock-recovery",
        json!({
            "blocked": startup_result(&blocked_ready),
            "blocked_exit_success": blocked_exit,
            "journal_unchanged_after_failure":
                journal_after_failure.as_deref() == Some(journal_before.as_slice()),
            "upgrade_failures": blocked_failures,
            "recovered": startup_result(&recovered_ready),
            "recovered_exit_success": recovered_exit,
            "journal_present_after_recovery": journal_after_recovery,
        }),
    );
    assert_eq!(
        blocked_ready["ready"], false,
        "the held lock must block startup"
    );
    assert_eq!(blocked_ready["startup"]["failure"], "storage_unavailable");
    assert_eq!(blocked_ready["startup"]["failure_retryable"], true);
    assert_eq!(
        journal_after_failure.as_deref(),
        Some(journal_before.as_slice()),
        "a failed verification must not advance the upgrade journal"
    );
    assert_eq!(
        recovered_ready["ready"], true,
        "restart after release must succeed"
    );
    assert!(
        !journal_after_recovery,
        "recovery completes the upgrade journal"
    );
}

/// 已提升的运行库真实损坏时，启动仍以不可重试的资料损坏拒绝。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn startup_still_rejects_a_corrupt_runtime_database() {
    let rendezvous = mount_rendezvous().await;
    let mut results = Vec::new();
    for (case, corruption) in [
        ("not_a_database", Corruption::Header),
        ("malformed_page", Corruption::Page),
    ] {
        let root = TempDir::new().expect("profile root");
        let storage = create_stopped_space(root.path(), &rendezvous.uri());
        corrupt(&control_database(root.path()), corruption);
        let (second, ready) = ProfileHost::launch(root.path(), &rendezvous.uri(), Some(storage));
        let exit = stop(second, &ready);
        results.push(json!({
            "case": case,
            "second": startup_result(&ready),
            "second_exit_success": exit,
            "upgrade_failures": upgrade_failures(root.path()),
        }));
    }
    write_artifact("corruption", json!({ "cases": results }));
    for result in &results {
        assert_eq!(result["second"]["ready"], false, "{result}");
        assert_eq!(result["second"]["failure"], "corrupt_data", "{result}");
        assert_eq!(result["second"]["failure_retryable"], false, "{result}");
    }
}

/// 创建 Space 并导出安全存储，第一个进程保持运行。
fn create_space(root: &Path, rendezvous: &str) -> (ProfileHost, Value) {
    let (mut host, ready) = ProfileHost::launch(root, rendezvous, None);
    assert_eq!(ready["ready"], true, "first host did not start: {ready}");
    host.request(json!({ "command": "create", "name": "Overlap" }));
    let storage = host.request(json!({ "command": "secure_storage" }));
    (host, storage)
}

/// 创建 Space 后完整关闭第一个进程，只交回安全存储。
fn create_stopped_space(root: &Path, rendezvous: &str) -> Value {
    let (mut host, storage) = create_space(root, rendezvous);
    host.request(json!({ "command": "shutdown" }));
    assert!(host.wait_exit().success(), "first host exit");
    storage
}

/// 已就绪的宿主正常关闭；启动失败的宿主自行退出。返回进程是否以成功状态退出。
fn stop(mut host: ProfileHost, ready: &Value) -> bool {
    if ready["ready"] == true {
        host.request(json!({ "command": "shutdown" }));
    }
    host.wait_exit().success()
}

fn startup_result(ready: &Value) -> Value {
    json!({
        "ready": ready["ready"],
        "code": ready["code"],
        "engine_error_retryable": ready["retryable"],
        "failure": ready["startup"]["failure"],
        "failure_retryable": ready["startup"]["failure_retryable"],
        "state": ready["startup"]["state"],
    })
}

/// 读取共享日志目录中的升级失败诊断，只保留固定分类字段。
fn upgrade_failures(root: &Path) -> Vec<Value> {
    let Ok(entries) = std::fs::read_dir(root.join("logs")) else {
        return Vec::new();
    };
    let mut failures = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let text = std::fs::read_to_string(entry.path()).unwrap_or_default();
        for row in text
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|row| row["message"] == "profile storage upgrade step failed")
        {
            let fields = &row["fields"];
            failures.push(json!({
                "run_id": row["run_id"],
                "error_kind": fields["error_kind"],
                "sqlite_result": fields["sqlite_result"],
                "upgrade_phase": fields["upgrade_phase"],
                "upgrade_action": fields["upgrade_action"],
                "io_error_kind": fields["io_error_kind"],
                "io_error_code": fields["io_error_code"],
            }));
        }
    }
    failures
}

fn control_database(root: &Path) -> PathBuf {
    find_file(&root.join("private"), "control.sqlite").expect("promoted control database")
}

fn find_file(directory: &Path, name: &str) -> Option<PathBuf> {
    for entry in std::fs::read_dir(directory).ok()?.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_file(&path, name) {
                return Some(found);
            }
        } else if path.file_name().is_some_and(|file| file == name) {
            return Some(path);
        }
    }
    None
}

/// 以独占锁模式持有数据库：WAL 读者在锁释放前得到 SQLITE_BUSY，与另一进程关闭最后一个连接时
/// 的检查点独占窗口相同，只是持续时间可控。
fn hold_exclusive(database: &Path) -> SqliteConnection {
    let mut connection =
        SqliteConnection::establish(database.to_str().expect("database path")).expect("holder");
    connection
        .batch_execute("PRAGMA locking_mode = EXCLUSIVE; BEGIN EXCLUSIVE; COMMIT;")
        .expect("hold exclusive database lock");
    connection
}

#[derive(Clone, Copy)]
enum Corruption {
    /// 覆盖文件头：SQLite 报告 file is not a database。
    Header,
    /// 保留文件头、破坏第二页之后的内容：完整性检查失败。
    Page,
}

fn corrupt(database: &Path, corruption: Corruption) {
    checkpoint(database);
    let mut bytes = std::fs::read(database).expect("read control database");
    match corruption {
        Corruption::Header => bytes[..16].copy_from_slice(b"not a database!\0"),
        Corruption::Page => {
            let page = u16::from_be_bytes([bytes[16], bytes[17]]) as usize;
            let page = if page == 1 { 65_536 } else { page };
            assert!(bytes.len() > page * 2, "control database has a second page");
            for byte in &mut bytes[page..page * 2] {
                *byte = 0xA5;
            }
        }
    }
    std::fs::write(database, bytes).expect("write corrupted control database");
}

/// 先把 WAL 合并回主库并删除 WAL，确保损坏写入的是 SQLite 实际读取的页面。
fn checkpoint(database: &Path) {
    let mut connection =
        SqliteConnection::establish(database.to_str().expect("database path")).expect("open");
    connection
        .batch_execute("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode = DELETE;")
        .expect("checkpoint control database");
}

fn env_number(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn write_artifact(scenario: &str, mut body: Value) {
    let directory = std::env::var_os("UC_STARTUP_OVERLAP_ARTIFACT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../target/startup-overlap")
        });
    std::fs::create_dir_all(&directory).expect("artifact directory");
    body["scenario"] = json!(scenario);
    body["platform"] = json!(std::env::consts::OS);
    body["source_commit"] = json!(std::env::var("UC_SOURCE_COMMIT").ok());
    let path = directory.join(format!("{scenario}.json"));
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&body).expect("artifact JSON"),
    )
    .expect("write artifact");
}
