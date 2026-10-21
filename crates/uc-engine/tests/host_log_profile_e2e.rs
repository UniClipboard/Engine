//! 宿主声明的日志档位：经公开 `Engine::start`、`QueryDiagnostics` 与 `ExportDiagnosticLogs`，用真实
//! 进程观测运行时、真实设置文件与诊断导出包验证 Engine 只原样报告宿主声明的档位。
//!
//! 全部场景在同一个测试中顺序执行，因为进程观测运行时每个进程只安装一次。旧版设置取自升级矩阵的
//! Desktop 0.19.4 资料，再改写 `general.debug_mode`。设置 `UC_TEST_ARTIFACTS_DIR` 时，把每一步的
//! 诊断状态、导出清单、设置文件哈希与键、拒绝结果写到 `<dir>/host-log-profile/result.json`。
//! 验收脚本以 `RUST_LOG=trace UC_LOG_PROFILE=debug` 运行本测试，证明 Engine 不读取宿主环境。

#![cfg(unix)]

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uc_engine::error_codes::HOST_LOG_PROFILE_INVALID_CODE;
use uc_engine::observability::{
    DeploymentEnvironment, LocalCaptureMode, LocalLogConfig, ObservabilityConfig,
    ObservabilityResource, OperatingSystem, ProcessObservabilityHandle,
    ProcessObservabilityRuntime,
};
use uc_engine::{
    DiagnosticsStatusSummary, Engine, EngineConfig, EngineErrorCategory, ExportDiagnosticLogsInput,
    HostCapabilities, HostCapabilityError, HostCapabilityErrorCategory, HostClipboard,
    HostClipboardSnapshot, HostDirectories, HostFileAccess, HostFileHandle, HostFileMetadata,
    HostSecureStorage, Operation, OperationResult, SettingsPatch, SettingsUpdateOutcome,
    SyncSettingsPatch,
};

const LEGACY_SETTINGS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/upgrade-matrix/fixtures/desktop-0.19.4/profile/settings.json"
);

#[tokio::test(flavor = "multi_thread")]
async fn engine_reports_only_the_host_declared_log_profile() {
    let root = tempfile::tempdir().unwrap();
    let logs = root.path().join("logs");
    let outbox = root.path().join("host-files");
    std::fs::create_dir_all(&outbox).unwrap();
    let runtime = ProcessObservabilityRuntime::install(
        ObservabilityConfig::new(
            ObservabilityResource::new(
                "2.0.0",
                DeploymentEnvironment::Test,
                OperatingSystem::Macos,
                "test",
            )
            .unwrap(),
        )
        .with_local_logs(LocalLogConfig::new(logs.clone())),
    )
    .unwrap()
    .handle();
    let mut steps = Vec::new();

    // 旧资料一：`debug_mode` 为非法类型，宿主声明 `debug`。
    let invalid_legacy = Profile::new(root.path().join("invalid-legacy"), &logs, &outbox);
    invalid_legacy.write_legacy_settings(json!("yes"));
    let before = invalid_legacy.settings_facts();
    let engine = invalid_legacy.start(Some("debug")).await;
    let mut step = observe(&engine, &runtime, &outbox, "invalid-legacy-debug").await;
    assert_eq!(step.profile, Some("debug".to_string()));
    assert_eq!(
        step.capture_mode, "standard",
        "the label must not start a detailed capture"
    );
    assert_eq!(
        step.engine_module_debug_records, 0,
        "the label must not raise Engine module logs"
    );
    assert_eq!(
        query_device_name(&engine).await,
        Some("LegacyFixture".to_string())
    );
    save_settings(&engine).await;
    let saved = invalid_legacy.settings_facts();
    assert_eq!(saved["general_keys"], json!(["device_name"]));
    assert_eq!(saved["has_debug_mode"], json!(false));
    step.extra = json!({ "settings_before": before, "settings_after_save": saved });
    steps.push(step);
    engine.shutdown(Duration::from_secs(15)).await.unwrap();

    // 同一资料改用其他档位重启：报告随本次声明变化，不读取任何保存值。
    let engine = invalid_legacy.start(Some("prod")).await;
    let step = observe(&engine, &runtime, &outbox, "invalid-legacy-restart-prod").await;
    assert_eq!(step.profile, Some("prod".to_string()));
    steps.push(step);
    engine.shutdown(Duration::from_secs(15)).await.unwrap();

    // 再次重启且不声明：报告缺失，不回退到 Engine 自身构建类型或环境变量。
    let engine = invalid_legacy.start(None).await;
    let step = observe(
        &engine,
        &runtime,
        &outbox,
        "invalid-legacy-restart-undeclared",
    )
    .await;
    assert_eq!(step.profile, None);
    steps.push(step);
    engine.shutdown(Duration::from_secs(15)).await.unwrap();

    // 旧资料二：`debug_mode = true` 不再影响报告；保存后该键消失；之后按声明报告。
    let enabled_legacy = Profile::new(root.path().join("enabled-legacy"), &logs, &outbox);
    enabled_legacy.write_legacy_settings(json!(true));
    let before = enabled_legacy.settings_facts();
    let engine = enabled_legacy.start(None).await;
    let mut step = observe(&engine, &runtime, &outbox, "enabled-legacy-undeclared").await;
    assert_eq!(
        step.profile, None,
        "a stored debug_mode must not become a profile"
    );
    save_settings(&engine).await;
    let saved = enabled_legacy.settings_facts();
    assert_eq!(saved["has_debug_mode"], json!(false));
    step.extra = json!({ "settings_before": before, "settings_after_save": saved });
    steps.push(step);
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
    let engine = enabled_legacy.start(Some("debug_clipboard")).await;
    let step = observe(
        &engine,
        &runtime,
        &outbox,
        "enabled-legacy-restart-declared",
    )
    .await;
    assert_eq!(step.profile, Some("debug_clipboard".to_string()));
    steps.push(step);
    engine.shutdown(Duration::from_secs(15)).await.unwrap();

    // 非法名称在任何资料读写之前拒绝：新资料不创建数据根，既有资料的设置字节不变。
    let existing_settings = std::fs::read(invalid_legacy.settings_path()).unwrap();
    let mut rejected = Vec::new();
    for label in [
        String::new(),
        "Debug".to_string(),
        "debug mode".to_string(),
        "../debug".to_string(),
        "debug.json".to_string(),
        "调试".to_string(),
        "a".repeat(33),
    ] {
        let fresh = Profile::new(root.path().join("rejected"), &logs, &outbox);
        let fresh_error = fresh.try_start(Some(&label)).await.err().expect("rejected");
        let existing_error = invalid_legacy
            .try_start(Some(&label))
            .await
            .err()
            .expect("rejected");
        for error in [&fresh_error, &existing_error] {
            assert_eq!(error.code(), HOST_LOG_PROFILE_INVALID_CODE);
            assert_eq!(error.category(), EngineErrorCategory::InvalidInput);
            assert!(!error.is_retryable());
        }
        assert!(
            !fresh.private.exists(),
            "rejection must precede profile I/O"
        );
        assert_eq!(
            std::fs::read(invalid_legacy.settings_path()).unwrap(),
            existing_settings
        );
        rejected.push(json!({
            "label_len": label.len(),
            "code": fresh_error.code(),
            "category": format!("{:?}", fresh_error.category()),
            "retryable": fresh_error.is_retryable(),
            "fresh_data_root_created": fresh.private.exists(),
            "existing_settings_unchanged": true,
        }));
    }
    let local_logs = read_logs(&logs);
    assert!(
        !local_logs.contains("debug mode") && !local_logs.contains("调试"),
        "rejected labels must not reach local logs"
    );

    runtime.shutdown(Duration::from_secs(2));
    write_artifact(json!({
        "environment": {
            "RUST_LOG": std::env::var("RUST_LOG").ok(),
            "UC_LOG_PROFILE": std::env::var("UC_LOG_PROFILE").ok(),
        },
        "legacy_settings_source": "tests/upgrade-matrix/fixtures/desktop-0.19.4/profile/settings.json",
        "steps": steps.iter().map(Step::to_json).collect::<Vec<_>>(),
        "rejected_labels": rejected,
    }));
}

/// 一次运行中的观察：公开诊断状态、导出包清单与 Engine 模块日志等级，以及观测运行时采集模式。
struct Step {
    name: &'static str,
    profile: Option<String>,
    manifest: Value,
    capture_mode: &'static str,
    engine_module_debug_records: usize,
    engine_module_records: usize,
    extra: Value,
}

impl Step {
    fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "query_diagnostics_effective_log_profile": self.profile,
            "export_manifest": self.manifest,
            "capture_mode": self.capture_mode,
            "engine_module_records": self.engine_module_records,
            "engine_module_debug_records": self.engine_module_debug_records,
            "extra": self.extra,
        })
    }
}

async fn observe(
    engine: &Engine,
    runtime: &ProcessObservabilityHandle,
    outbox: &Path,
    name: &'static str,
) -> Step {
    let OperationResult::DiagnosticsStatus(DiagnosticsStatusSummary {
        rejected_config_import,
        effective_log_profile,
    }) = engine.execute(Operation::QueryDiagnostics).await.unwrap()
    else {
        panic!("unexpected diagnostics result");
    };
    assert_eq!(rejected_config_import, None, "no pending import exists");
    let capture_mode = match runtime.query_local_diagnostic_status().capture.mode {
        LocalCaptureMode::Standard => "standard",
        LocalCaptureMode::Detailed => "detailed",
    };
    let handle = format!("{name}.zip");
    assert!(matches!(
        engine
            .execute(Operation::ExportDiagnosticLogs(ExportDiagnosticLogsInput {
                since_hours: Some(1),
                destination: HostFileHandle::new(handle.clone()),
            }))
            .await
            .unwrap(),
        OperationResult::DiagnosticLogsExported(_)
    ));
    let archive = std::fs::read(outbox.join(&handle)).unwrap();
    let (manifest, module_records) = read_archive(&archive);
    let object = manifest.as_object().expect("manifest object");
    assert!(!object.contains_key("debugMode"), "{manifest}");
    assert_eq!(
        object.get("effectiveLogProfile"),
        Some(&json!(effective_log_profile)),
        "status and manifest must report the same declaration"
    );
    let engine_module_debug_records = module_records
        .iter()
        .filter(|record| record["level"] == "DEBUG")
        .count();
    Step {
        name,
        profile: effective_log_profile,
        manifest,
        capture_mode,
        engine_module_debug_records,
        engine_module_records: module_records.len(),
        extra: Value::Null,
    }
}

/// 返回导出包的清单与其中全部 Engine 模块日志记录。
fn read_archive(bytes: &[u8]) -> (Value, Vec<Value>) {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut manifest = Value::Null;
    let mut records = Vec::new();
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).unwrap();
        let name = file.name().expect("valid archive entry name").to_string();
        let mut text = String::new();
        file.read_to_string(&mut text).unwrap();
        if name == "manifest.json" {
            manifest = serde_json::from_str(&text).unwrap();
        } else if name.ends_with(".jsonl") {
            records.extend(
                text.lines()
                    .map(|line| serde_json::from_str::<Value>(line).unwrap())
                    .filter(|record| record["source"] == "engine_module"),
            );
        }
    }
    (manifest, records)
}

async fn query_device_name(engine: &Engine) -> Option<String> {
    let OperationResult::Settings(settings) =
        engine.execute(Operation::QuerySettings).await.unwrap()
    else {
        panic!("unexpected settings result");
    };
    settings.general.device_name
}

/// 经公开操作保存一次 Engine 设置，触发设置文件重写。
async fn save_settings(engine: &Engine) {
    let result = engine
        .execute(Operation::UpdateSettings(Box::new(SettingsPatch {
            sync: Some(SyncSettingsPatch {
                sync_on_restore: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        })))
        .await
        .unwrap();
    assert!(matches!(
        result,
        OperationResult::SettingsUpdated(SettingsUpdateOutcome::Updated(_))
    ));
}

fn read_logs(directory: &Path) -> String {
    let mut text = String::new();
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            text.push_str(&String::from_utf8_lossy(&std::fs::read(path).unwrap()));
        }
    }
    text
}

fn write_artifact(result: Value) {
    let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") else {
        return;
    };
    let directory = PathBuf::from(directory).join("host-log-profile");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("result.json"),
        serde_json::to_vec_pretty(&result).unwrap(),
    )
    .unwrap();
}

/// 一个隔离安装：数据根位于 `<root>/private`，全部安装共用同一个日志目录与宿主文件目录。
struct Profile {
    root: PathBuf,
    private: PathBuf,
    logs: PathBuf,
    outbox: PathBuf,
    storage: MemorySecureStorage,
}

impl Profile {
    fn new(root: PathBuf, logs: &Path, outbox: &Path) -> Self {
        Self {
            private: root.join("private"),
            root,
            logs: logs.to_path_buf(),
            outbox: outbox.to_path_buf(),
            storage: MemorySecureStorage::default(),
        }
    }

    fn host(&self) -> HostCapabilities {
        HostCapabilities::new(
            HostDirectories::new(
                self.private.clone(),
                self.root.join("cache"),
                self.root.join("temporary"),
                self.logs.clone(),
            ),
            Box::new(self.storage.clone()),
            Box::new(EmptyClipboard),
            Box::new(DirectoryFiles(self.outbox.clone())),
        )
    }

    fn config(label: Option<&str>) -> EngineConfig {
        let config = EngineConfig::new("2.0.0");
        match label {
            Some(label) => config.with_host_log_profile(label),
            None => config,
        }
    }

    async fn start(&self, label: Option<&str>) -> Engine {
        self.try_start(label).await.unwrap()
    }

    async fn try_start(&self, label: Option<&str>) -> Result<Engine, uc_engine::EngineError> {
        Engine::start(Self::config(label), self.host())
            .await
            .map(|(engine, _events)| engine)
    }

    fn settings_path(&self) -> PathBuf {
        self.private.join("settings.json")
    }

    fn write_legacy_settings(&self, debug_mode: Value) {
        let mut settings: Value =
            serde_json::from_slice(&std::fs::read(LEGACY_SETTINGS).unwrap()).unwrap();
        settings["general"]["debug_mode"] = debug_mode;
        std::fs::create_dir_all(&self.private).unwrap();
        std::fs::write(
            self.settings_path(),
            serde_json::to_vec_pretty(&settings).unwrap(),
        )
        .unwrap();
    }

    fn settings_facts(&self) -> Value {
        let bytes = std::fs::read(self.settings_path()).unwrap();
        let settings: Value = serde_json::from_slice(&bytes).unwrap();
        let general = settings["general"].as_object().unwrap();
        json!({
            "sha256": sha256(&bytes),
            "general_keys": general.keys().collect::<Vec<_>>(),
            "has_debug_mode": general.contains_key("debug_mode"),
            "debug_mode": general.get("debug_mode"),
        })
    }
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Clone, Default)]
struct MemorySecureStorage {
    values: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>>,
}

impl HostSecureStorage for MemorySecureStorage {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, HostCapabilityError> {
        Ok(self.values.lock().unwrap().get(key).cloned())
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<(), HostCapabilityError> {
        self.values
            .lock()
            .unwrap()
            .insert(key.to_owned(), value.to_vec());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), HostCapabilityError> {
        self.values.lock().unwrap().remove(key);
        Ok(())
    }
}

struct EmptyClipboard;

impl HostClipboard for EmptyClipboard {
    fn read(&self) -> Result<HostClipboardSnapshot, HostCapabilityError> {
        Ok(HostClipboardSnapshot {
            observed_at_ms: 1,
            representations: Vec::new(),
        })
    }

    fn write(&self, _snapshot: HostClipboardSnapshot) -> Result<(), HostCapabilityError> {
        Ok(())
    }
}

/// 句柄即 `outbox` 下的文件名，模拟宿主的文件选择结果。
struct DirectoryFiles(PathBuf);

fn host_io_error(error: std::io::Error) -> HostCapabilityError {
    HostCapabilityError::new(HostCapabilityErrorCategory::Unavailable, error.to_string())
}

impl HostFileAccess for DirectoryFiles {
    fn metadata(&self, handle: &HostFileHandle) -> Result<HostFileMetadata, HostCapabilityError> {
        let metadata = std::fs::metadata(self.0.join(handle.as_str())).map_err(host_io_error)?;
        Ok(HostFileMetadata {
            display_name: handle.as_str().into(),
            size_bytes: metadata.len(),
            mime_type: None,
        })
    }

    fn read_chunk(
        &self,
        handle: &HostFileHandle,
        offset: u64,
        max_bytes: u32,
    ) -> Result<Vec<u8>, HostCapabilityError> {
        let mut file = std::fs::File::open(self.0.join(handle.as_str())).map_err(host_io_error)?;
        file.seek(SeekFrom::Start(offset)).map_err(host_io_error)?;
        let mut bytes = vec![0; max_bytes as usize];
        let read = file.read(&mut bytes).map_err(host_io_error)?;
        bytes.truncate(read);
        Ok(bytes)
    }

    fn write_chunk(
        &self,
        handle: &HostFileHandle,
        offset: u64,
        bytes: &[u8],
    ) -> Result<(), HostCapabilityError> {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(self.0.join(handle.as_str()))
            .map_err(host_io_error)?;
        file.seek(SeekFrom::Start(offset)).map_err(host_io_error)?;
        file.write_all(bytes).map_err(host_io_error)
    }

    fn finish_write(&self, _handle: &HostFileHandle) -> Result<(), HostCapabilityError> {
        Ok(())
    }
}
