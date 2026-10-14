//! 移动端把 Engine 日志放在宿主缓存目录下（`<cache>/logs`），离开空间会清理缓存。
//! 进程级日志写入器只在安装时打开当天文件，因此离开空间不能删掉正在写入的日志：
//! 否则之后的记录写进已删除的文件，诊断导出报告已写入却找不到任何文件。
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
    let logs = host.cache.join("logs");
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
    assert_eq!(on_disk_before, written_before);

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
        on_disk_after, written_after,
        "every record the engine reports as written must still be exportable after leaving"
    );

    engine
        .shutdown(SHUTDOWN_DEADLINE_MS)
        .expect("engine shutdown");
}
