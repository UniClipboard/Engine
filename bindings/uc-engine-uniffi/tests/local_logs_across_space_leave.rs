//! 移动端 Engine 日志放在私有数据目录下的 `logs`，不在缓存目录内：离开空间会清理整个缓存，
//! 进程级日志写入器持有当天文件直到进程退出，日志若在缓存里就会被一并删除，
//! 之后的记录写进已删除的文件，诊断导出报告已写入却找不到任何文件。
//! 旧版本把日志放在 `<cache>/logs`；安装观测运行期时一次性迁入新目录，旧记录不丢。
//! 独立测试二进制：进程级 observability 每个进程只能安装一次。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use uc_engine_uniffi::{
    install_process_observability, prepare_local_diagnostic_export, BindingClipboardSnapshot,
    BindingConfig, BindingDeploymentEnvironment, BindingFileMetadata, BindingHost,
    BindingObservabilityConfig, BindingObservabilitySetupStatus, BindingObservabilitySignalResult,
    HostBindingError, MobileEngine,
};

const PASSPHRASE: &str = "correct horse battery staple";
const SHUTDOWN_DEADLINE_MS: u64 = 30_000;
/// 迁入的旧布局日志里的记录数；不是本次运行写入的，不计入 `written_count`。
const LEGACY_RECORDS: u64 = 1;

/// 公历日期（UTC），仅用于生成受管日志文件名。
fn utc_date(time: std::time::SystemTime) -> String {
    let days = time
        .duration_since(std::time::UNIX_EPOCH)
        .expect("after epoch")
        .as_secs()
        / 86_400;
    // Howard Hinnant 的 civil_from_days。
    let z = i64::try_from(days).expect("days") + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

struct DirectoryHost {
    private_data: PathBuf,
    cache: PathBuf,
    temporary: PathBuf,
    values: Mutex<HashMap<String, Vec<u8>>>,
}

impl DirectoryHost {
    fn new(root: &Path) -> Self {
        Self {
            private_data: root.join("data"),
            cache: root.join("cache"),
            temporary: root.join("temporary"),
            values: Mutex::new(HashMap::new()),
        }
    }

    fn values(&self) -> std::sync::MutexGuard<'_, HashMap<String, Vec<u8>>> {
        self.values
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl BindingHost for DirectoryHost {
    fn private_data_directory(&self) -> Result<String, HostBindingError> {
        Ok(self.private_data.to_string_lossy().into_owned())
    }

    fn cache_directory(&self) -> Result<String, HostBindingError> {
        Ok(self.cache.to_string_lossy().into_owned())
    }

    fn temporary_directory(&self) -> Result<String, HostBindingError> {
        Ok(self.temporary.to_string_lossy().into_owned())
    }

    fn secure_storage_get(&self, key: String) -> Result<Option<Vec<u8>>, HostBindingError> {
        Ok(self.values().get(&key).cloned())
    }

    fn secure_storage_set(&self, key: String, value: Vec<u8>) -> Result<(), HostBindingError> {
        self.values().insert(key, value);
        Ok(())
    }

    fn secure_storage_delete(&self, key: String) -> Result<(), HostBindingError> {
        self.values().remove(&key);
        Ok(())
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
        _handle: String,
        _offset: u64,
        _bytes: Vec<u8>,
    ) -> Result<(), HostBindingError> {
        Err(HostBindingError::InvalidHandle)
    }

    fn file_finish_write(&self, _handle: String) -> Result<(), HostBindingError> {
        Err(HostBindingError::InvalidHandle)
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

/// 与移动端导出相同：先由 Engine 刷新并报告写入计数，再列出日志目录里的受管文件。
fn export_written_and_on_disk(logs: &Path) -> (u64, u64) {
    let report = prepare_local_diagnostic_export(1_000).expect("diagnostic export report");
    assert_eq!(report.flush, BindingObservabilitySignalResult::Completed);
    let written = report.files.iter().map(|file| file.written_count).sum();
    let on_disk = fs::read_dir(logs)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    name.starts_with("engine.") && name.ends_with(".jsonl")
                })
                .map(|entry| {
                    fs::read_to_string(entry.path())
                        .expect("managed log file")
                        .lines()
                        .filter(|line| !line.is_empty())
                        .count() as u64
                })
                .sum()
        })
        .unwrap_or(0);
    (written, on_disk)
}

#[test]
fn leaving_a_space_keeps_the_active_engine_log_exportable() {
    let root = tempfile::tempdir().expect("temporary host root");
    let host = Arc::new(DirectoryHost::new(root.path()));
    let logs = host.private_data.join("logs");
    assert!(
        !logs.starts_with(&host.cache),
        "the log directory must not live inside the cache"
    );
    // 旧布局遗留的日志：安装时迁入新目录，缓存里不再保留。
    let legacy = host.cache.join("logs");
    fs::create_dir_all(&legacy).expect("legacy log directory");
    // 迁入后仍在保留期内，所以用 UTC 昨天的日期命名。
    let yesterday = utc_date(std::time::SystemTime::now() - std::time::Duration::from_secs(86_400));
    let legacy_name = format!("engine.{yesterday}.jsonl");
    fs::write(legacy.join(&legacy_name), b"{\"legacy\":true}\n").expect("legacy log file");
    let setup = install_process_observability(
        BindingObservabilityConfig {
            service_version: "1.2.3".to_owned(),
            environment: BindingDeploymentEnvironment::Test,
            app_channel: "test".to_owned(),
            remote_diagnostics_enabled: false,
            collector: None,
        },
        host.clone(),
    )
    .expect("observability install");
    assert_eq!(setup.local_file, BindingObservabilitySetupStatus::Ready);
    assert_eq!(
        fs::read(logs.join(&legacy_name)).expect("migrated legacy log"),
        b"{\"legacy\":true}\n"
    );
    assert!(
        !legacy.join(&legacy_name).exists(),
        "the legacy copy is moved, not duplicated"
    );

    let engine = MobileEngine::start(
        BindingConfig {
            app_version: "1.2.3".to_owned(),
            profile_id: "local-logs-across-leave".to_owned(),
        },
        host.clone(),
    )
    .expect("engine start");
    engine
        .create_space(Some("before leave".to_owned()), PASSPHRASE.to_owned())
        .expect("create space");
    let (written_before, on_disk_before) = export_written_and_on_disk(&logs);
    assert!(written_before > 0, "the engine must have written records");
    assert_eq!(on_disk_before, written_before + LEGACY_RECORDS);

    engine.leave_space().expect("leave space");
    engine
        .create_space(Some("after leave".to_owned()), PASSPHRASE.to_owned())
        .expect("the same instance can create a space after leaving");

    let (written_after, on_disk_after) = export_written_and_on_disk(&logs);
    assert!(
        written_after > written_before,
        "records must keep being written after leaving"
    );
    assert_eq!(
        on_disk_after,
        written_after + LEGACY_RECORDS,
        "every record the engine reports as written must still be exportable after leaving"
    );

    engine
        .shutdown(SHUTDOWN_DEADLINE_MS)
        .expect("engine shutdown");
}
