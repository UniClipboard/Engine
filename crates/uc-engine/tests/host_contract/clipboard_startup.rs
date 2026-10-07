//! 隔离进程的真实 Engine/SQLite/宿主导入启动恢复验收；不读取全局剪贴板或系统钥匙串。

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use diesel::connection::SimpleConnection;
use diesel::Connection;
use serde_json::json;
use tokio::process::Command;
use uc_engine::observability::{
    DeploymentEnvironment, LocalLogConfig, ObservabilityConfig, ObservabilityResource,
    OperatingSystem, ProcessObservabilityRuntime,
};
use uc_engine::{
    ClipboardRestoreMode, CreateSpaceInput, Engine, EngineConfig, ExportDiagnosticLogsInput,
    HostCapabilities, HostCapabilityError, HostCapabilityErrorCategory, HostClipboard,
    HostClipboardRepresentation, HostClipboardSnapshot, HostDirectories, HostFileAccess,
    HostFileHandle, HostFileMetadata, Operation, OperationResult, RestoreClipboardInput,
    SecretString, SendTextInput,
};
use uc_testkit::{FailureKind, Scenario, ScenarioBudget, ScenarioConfig, ScenarioFailure};

use super::{startup::runtime_database, MemorySecureStorage};

const CHILD_ENV: &str = "UC_CLIPBOARD_STARTUP_CHILD";
const PRIVATE_SENTINEL: &str = "private-clipboard-startup-sentinel";

#[derive(Clone, Copy, Debug)]
enum Fault {
    None,
    Clipboard,
    Metadata,
    Open,
    Missing,
    Middle,
    EmptyChunk,
    SecondRep,
    Destination,
    Directory,
}

#[derive(Clone)]
struct Clipboard {
    snapshot: Arc<Mutex<HostClipboardSnapshot>>,
    reads: Arc<AtomicUsize>,
    fault: Arc<Mutex<Fault>>,
    import_root: PathBuf,
}

impl Clipboard {
    fn new(root: &Path) -> Self {
        Self {
            snapshot: Arc::new(Mutex::new(HostClipboardSnapshot {
                observed_at_ms: 1,
                representations: Vec::new(),
            })),
            reads: Arc::new(AtomicUsize::new(0)),
            fault: Arc::new(Mutex::new(Fault::None)),
            import_root: root.join("temporary/clipboard-imports"),
        }
    }

    fn files(&self, count: usize) {
        self.snapshot.lock().unwrap().representations = (0..count)
            .map(|index| HostClipboardRepresentation::File {
                format: "file".into(),
                handle: HostFileHandle::new(index.to_string()),
                display_name: PRIVATE_SENTINEL.into(),
                mime_type: Some("application/octet-stream".into()),
                size_bytes: 128 * 1024,
            })
            .collect();
        self.snapshot
            .lock()
            .unwrap()
            .representations
            .push(HostClipboardRepresentation::Inline {
                format: "files".into(),
                mime_type: Some("text/uri-list".into()),
                bytes: b"file:///isolated-synthetic-source".to_vec(),
            });
    }

    fn text(&self) {
        self.snapshot.lock().unwrap().representations = vec![HostClipboardRepresentation::Inline {
            format: "text".into(),
            mime_type: Some("text/plain".into()),
            bytes: PRIVATE_SENTINEL.as_bytes().to_vec(),
        }];
    }
}

impl HostClipboard for Clipboard {
    fn read(&self) -> Result<HostClipboardSnapshot, HostCapabilityError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        if matches!(*self.fault.lock().unwrap(), Fault::Clipboard) {
            return Err(host_error(io::ErrorKind::PermissionDenied));
        }
        if matches!(*self.fault.lock().unwrap(), Fault::Directory) {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&self.import_root, std::fs::Permissions::from_mode(0o500))
                    .unwrap();
            }
        }
        Ok(self.snapshot.lock().unwrap().clone())
    }

    fn write(&self, snapshot: HostClipboardSnapshot) -> Result<(), HostCapabilityError> {
        *self.snapshot.lock().unwrap() = snapshot;
        Ok(())
    }
}

fn host_error(kind: io::ErrorKind) -> HostCapabilityError {
    let error =
        HostCapabilityError::new(HostCapabilityErrorCategory::Io, "isolated host read failed")
            .with_source(Box::new(io::Error::new(kind, PRIVATE_SENTINEL)));
    assert_eq!(
        std::error::Error::source(&error)
            .unwrap()
            .downcast_ref::<io::Error>()
            .unwrap()
            .kind(),
        kind
    );
    assert!(!format!("{error:?}").contains(PRIVATE_SENTINEL));
    error
}

#[derive(Clone)]
struct Files {
    fault: Arc<Mutex<Fault>>,
    import_root: PathBuf,
    export: PathBuf,
}

impl HostFileAccess for Files {
    fn metadata(&self, _: &HostFileHandle) -> Result<HostFileMetadata, HostCapabilityError> {
        match *self.fault.lock().unwrap() {
            Fault::Metadata => return Err(host_error(io::ErrorKind::PermissionDenied)),
            Fault::Destination => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    for directory in std::fs::read_dir(&self.import_root).unwrap() {
                        std::fs::set_permissions(
                            directory.unwrap().path(),
                            std::fs::Permissions::from_mode(0o500),
                        )
                        .unwrap();
                    }
                }
            }
            _ => {}
        }
        Ok(HostFileMetadata {
            display_name: PRIVATE_SENTINEL.into(),
            size_bytes: 128 * 1024,
            mime_type: None,
        })
    }

    fn read_chunk(
        &self,
        handle: &HostFileHandle,
        offset: u64,
        max: u32,
    ) -> Result<Vec<u8>, HostCapabilityError> {
        match *self.fault.lock().unwrap() {
            Fault::Open => return Err(host_error(io::ErrorKind::PermissionDenied)),
            Fault::Missing => return Err(host_error(io::ErrorKind::NotFound)),
            Fault::Middle if offset > 0 => return Err(host_error(io::ErrorKind::UnexpectedEof)),
            Fault::EmptyChunk => return Ok(Vec::new()),
            Fault::SecondRep if handle.as_str() == "1" => {
                return Err(host_error(io::ErrorKind::NotFound))
            }
            _ => {}
        }
        Ok(vec![42; max.min((128 * 1024 - offset) as u32) as usize])
    }

    fn write_chunk(
        &self,
        _: &HostFileHandle,
        offset: u64,
        bytes: &[u8],
    ) -> Result<(), HostCapabilityError> {
        use std::io::{Seek, SeekFrom, Write};
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&self.export)
            .unwrap();
        file.seek(SeekFrom::Start(offset)).unwrap();
        file.write_all(bytes).unwrap();
        Ok(())
    }

    fn finish_write(&self, _: &HostFileHandle) -> Result<(), HostCapabilityError> {
        Ok(())
    }
}

fn host(root: &Path, storage: MemorySecureStorage, clipboard: Clipboard) -> HostCapabilities {
    HostCapabilities::new(
        HostDirectories::new(
            root.join("private"),
            root.join("cache"),
            root.join("temporary"),
            root.join("logs"),
        ),
        Box::new(storage),
        Box::new(clipboard.clone()),
        Box::new(Files {
            fault: clipboard.fault,
            import_root: root.join("temporary/clipboard-imports"),
            export: root.join("diagnostics.zip"),
        }),
    )
}

async fn start(root: &Path, storage: &MemorySecureStorage, clipboard: &Clipboard) -> Engine {
    Engine::start(
        EngineConfig::new("2.0.0"),
        host(root, storage.clone(), clipboard.clone()),
    )
    .await
    .unwrap()
    .0
}

async fn activate(engine: &Engine) {
    let OperationResult::EntrySent(saved) = engine
        .execute(Operation::SendText(SendTextInput {
            text: PRIVATE_SENTINEL.into(),
            target_devices: Vec::new(),
        }))
        .await
        .unwrap()
    else {
        panic!("expected stored entry")
    };
    engine
        .execute(Operation::RestoreClipboard(RestoreClipboardInput {
            entry_id: saved.entry_id,
            mode: ClipboardRestoreMode::Standard,
        }))
        .await
        .unwrap();
    assert!(matches!(
        engine
            .execute(Operation::QueryActiveClipboard)
            .await
            .unwrap(),
        OperationResult::ActiveClipboard(Some(_))
    ));
}

fn sql(root: &Path, command: &str) {
    let path = runtime_database(
        root,
        "profile-data-generations",
        "v3-payloads/profile.sqlite",
    );
    diesel::sqlite::SqliteConnection::establish(path.to_str().unwrap())
        .unwrap()
        .batch_execute(command)
        .unwrap();
}

fn imports(root: &Path) -> Vec<PathBuf> {
    let path = root.join("temporary/clipboard-imports");
    if !path.exists() {
        return vec![];
    }
    std::fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "由独立进程验收启动，避免全局观测运行时与其他测试串扰"]
async fn daemon_child() {
    let root = PathBuf::from(std::env::var(CHILD_ENV).unwrap());
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
        .with_local_logs(LocalLogConfig::new(root.join("logs"))),
    )
    .unwrap()
    .handle();
    let storage = MemorySecureStorage::default();
    let clipboard = Clipboard::new(&root);
    let engine = start(&root, &storage, &clipboard).await;
    engine
        .execute(Operation::CreateSpace(CreateSpaceInput {
            device_name: Some("isolated clipboard acceptance".into()),
            passphrase: SecretString::new("isolated-test-passphrase"),
            passphrase_confirmation: SecretString::new("isolated-test-passphrase"),
        }))
        .await
        .unwrap();
    activate(&engine).await;
    engine.shutdown_until_complete().await.unwrap();
    drop(engine);

    let mut results = Vec::new();
    for fault in [
        Fault::Open,
        Fault::Missing,
        Fault::Middle,
        Fault::Clipboard,
        Fault::Metadata,
        Fault::EmptyChunk,
        Fault::SecondRep,
        Fault::Destination,
        Fault::Directory,
    ] {
        if cfg!(not(unix)) && matches!(fault, Fault::Destination | Fault::Directory) {
            continue;
        }
        *clipboard.fault.lock().unwrap() = fault;
        clipboard.files(2);
        let before = imports(&root);
        let engine = start(&root, &storage, &clipboard).await;
        assert!(matches!(
            engine
                .execute(Operation::QueryActiveClipboard)
                .await
                .unwrap(),
            OperationResult::ActiveClipboard(None)
        ));
        assert_eq!(
            imports(&root),
            before,
            "failed import leaked files: {fault:?}"
        );
        #[cfg(unix)]
        if matches!(fault, Fault::Directory) {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                &clipboard.import_root,
                std::fs::Permissions::from_mode(0o700),
            )
            .unwrap();
        }
        *clipboard.fault.lock().unwrap() = Fault::None;
        clipboard.text();
        assert!(matches!(
            engine
                .execute(Operation::CaptureCurrentClipboard)
                .await
                .unwrap(),
            OperationResult::ClipboardCaptured { entry_id: Some(_) }
        ));
        activate(&engine).await;
        engine.shutdown_until_complete().await.unwrap();
        drop(engine);
        results.push(json!({ "fault": format!("{fault:?}"), "startup": "ready", "register": "cleared", "import_cleanup": "passed", "subsequent_capture": "passed" }));
    }

    // 真实 SQLite trigger 拒绝 DELETE，证明不能吞掉持久化失败。
    sql(&root, "CREATE TRIGGER reject_active_reset BEFORE DELETE ON active_clipboard_register BEGIN SELECT RAISE(ABORT, 'injected reset failure'); END;");
    *clipboard.fault.lock().unwrap() = Fault::Open;
    clipboard.files(1);
    let failed = Engine::start(
        EngineConfig::new("2.0.0"),
        host(&root, storage.clone(), clipboard.clone()),
    )
    .await;
    assert!(failed.is_err(), "DB reset failure must stop startup");
    let error = failed.err().unwrap();
    assert_eq!(error.code(), 1101);
    assert!(!serde_json::to_string(&error)
        .unwrap()
        .contains(PRIVATE_SENTINEL));
    sql(&root, "DROP TRIGGER reject_active_reset;");
    let engine = start(&root, &storage, &clipboard).await;
    assert!(matches!(
        engine
            .execute(Operation::QueryActiveClipboard)
            .await
            .unwrap(),
        OperationResult::ActiveClipboard(None)
    ));
    engine.shutdown_until_complete().await.unwrap();
    drop(engine);

    let reads = clipboard.reads.load(Ordering::SeqCst);
    let engine = start(&root, &storage, &clipboard).await;
    assert_eq!(
        clipboard.reads.load(Ordering::SeqCst),
        reads,
        "empty register must skip OS read at startup"
    );
    *clipboard.fault.lock().unwrap() = Fault::None;
    clipboard.files(1);
    assert!(matches!(
        engine
            .execute(Operation::CaptureCurrentClipboard)
            .await
            .unwrap(),
        OperationResult::ClipboardCaptured { entry_id: Some(_) }
    ));
    let successful_imports = imports(&root);
    assert!(!successful_imports.is_empty());
    *clipboard.fault.lock().unwrap() = Fault::SecondRep;
    clipboard.files(2);
    assert!(engine
        .execute(Operation::CaptureCurrentClipboard)
        .await
        .is_err());
    assert_eq!(
        imports(&root),
        successful_imports,
        "failed call deleted a successful call's files or leaked its own"
    );
    *clipboard.fault.lock().unwrap() = Fault::None;
    clipboard.text();
    activate(&engine).await;
    engine.shutdown_until_complete().await.unwrap();
    drop(engine);
    let engine = start(&root, &storage, &clipboard).await;
    assert!(
        matches!(
            engine
                .execute(Operation::QueryActiveClipboard)
                .await
                .unwrap(),
            OperationResult::ActiveClipboard(Some(_))
        ),
        "normal text match must retain register"
    );
    engine
        .execute(Operation::ExportDiagnosticLogs(ExportDiagnosticLogsInput {
            since_hours: Some(24),
            destination: HostFileHandle::new("export"),
        }))
        .await
        .unwrap();
    engine.shutdown_until_complete().await.unwrap();
    runtime.shutdown(Duration::from_secs(2));
    std::fs::write(root.join("matrix.json"), serde_json::to_vec_pretty(&json!({ "pid": std::process::id(), "cases": results, "reset_failure_code": 1101, "empty_register_skips_read": true, "normal_file_and_text": true, "boundary": "real Engine process and SQLite with isolated injected host; no Finder/TCC or Desktop daemon proof" })).unwrap()).unwrap();
}

#[tokio::test]
async fn clipboard_startup_recovery_process() {
    let artifact_root = std::env::var_os("UC_TEST_ARTIFACTS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-artifacts")
        });
    let mut scenario = Scenario::start(ScenarioConfig::new("clipboard-startup-recovery", 1886, ScenarioBudget::new(Duration::from_secs(180)), "just cargo test -p uc-engine --test host_contract clipboard_startup_recovery_process --locked -- --nocapture", artifact_root)).unwrap();
    let profile = scenario.temp_dir("isolated-profile").unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "clipboard_startup::daemon_child",
            "--ignored",
            "--nocapture",
        ])
        .env(CHILD_ENV, profile.path());
    let status = scenario
        .run_child_process(
            "isolated-engine-daemon",
            &mut command,
            Duration::from_secs(170),
        )
        .await;
    let artifacts = ["matrix.json", "diagnostics.zip"]
        .map(|name| (name, std::fs::read(profile.path().join(name)).ok()));
    drop(profile);
    let result = match status {
        Ok(status) if status.success() => Ok(()),
        _ => Err(ScenarioFailure::new(
            FailureKind::ProductInvariant,
            "clipboard-startup-child-failed",
        )),
    };
    if result.is_ok() {
        verify_archive(artifacts[1].1.as_ref().unwrap());
    }
    let completion = scenario.finish(result).unwrap();
    for (name, bytes) in artifacts {
        std::fs::write(completion.artifact_dir().join(name), bytes.unwrap()).unwrap();
    }
}

fn verify_archive(bytes: &[u8]) {
    use std::io::Read;
    let mut archive = zip::ZipArchive::new(io::Cursor::new(bytes)).unwrap();
    let mut records = Vec::new();
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).unwrap();
        if file
            .name()
            .rsplit('/')
            .next()
            .unwrap()
            .starts_with("engine.")
            && file.name().ends_with(".jsonl")
        {
            let mut log = String::new();
            file.read_to_string(&mut log).unwrap();
            assert!(!log.contains(PRIVATE_SENTINEL));
            records.extend(
                log.lines()
                    .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap()),
            );
        }
    }
    let reads = records
        .iter()
        .filter(|record| record["fields"]["error_kind"] == "active_clipboard_os_read")
        .collect::<Vec<_>>();
    assert!(
        !reads.is_empty(),
        "standard archive omitted OS read failures"
    );
    for kind in ["PermissionDenied", "NotFound", "UnexpectedEof"] {
        assert!(
            reads
                .iter()
                .any(|record| record["error.chain"].to_string().contains(kind)),
            "source kind missing: {kind}"
        );
    }
    assert!(reads
        .iter()
        .all(|record| record["capture_mode"] == "standard"));
    assert!(records.iter().any(|record| record["fields"]["error_kind"]
        == "active_clipboard_register_reset"
        && record["fields"]["error_class"] == "storage"));
    assert!(records.iter().any(
        |record| record["fields"]["context"] == "clipboard background"
            && record["error.chain"].is_array()
    ));
}
