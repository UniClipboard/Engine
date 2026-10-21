//! 宿主声明的附加配置文件：经公开 `Engine::start` 与操作，用真实文件验证配置导出、导入恢复、
//! 恢复出厂与升级备份的完整行为。
//!
//! 每个场景使用独立临时数据根和合成文件。设置 `UC_TEST_ARTIFACTS_DIR` 时，各场景把输入、配置包、
//! 暂存目录和结果目录的清单与 sha256 写到 `<dir>/host-configuration-files/<场景>/inventory.txt`。

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sha2::{Digest, Sha256};
use uc_engine::error_codes::{EXPORT_CONFIG_FAILED_CODE, HOST_CONFIGURATION_FILES_INVALID_CODE};
use uc_engine::{
    ConfigExportOutcome, ConfigImportStageOutcome, CreateSpaceInput, Engine, EngineConfig,
    EngineErrorCategory, ExportConfigInput, HostCapabilities, HostCapabilityError,
    HostCapabilityErrorCategory, HostClipboard, HostClipboardSnapshot, HostConfigurationFile,
    HostConfigurationFileReset, HostDirectories, HostFileAccess, HostFileHandle, HostFileMetadata,
    HostSecureStorage, Operation, OperationResult, SecretString, StageConfigImportInput,
};
use uc_infra_profile::security::{ProfileArchiveReceipt, ProfileBackupArchive};

const PASSPHRASE: &str = "synthetic-host-files-passphrase";
const LEGACY_PASSPHRASE: &str = "synthetic-legacy-passphrase";

const PRODUCT: &str = "desktop-settings.json";
const NOTIFIED: &str = "last_notified_update.json";
const SKIPPED: &str = "skipped_version.json";
const THROTTLE: &str = "update_prompt_throttle.json";
const TELEMETRY: &str = "desktop-telemetry.json";
const UNLISTED: &str = "unlisted-host-note.json";
const ENGINE_RESET_FILES: [&str; 3] = [NOTIFIED, SKIPPED, THROTTLE];

/// 与 Desktop task10 计划一致的合成声明：产品文件由宿主在重置成功后处理，更新状态文件由 Engine 删除。
fn desktop_declaration() -> Vec<HostConfigurationFile> {
    let mut files = vec![HostConfigurationFile::new(
        PRODUCT,
        HostConfigurationFileReset::ByHost,
    )];
    files.extend(
        ENGINE_RESET_FILES
            .iter()
            .map(|name| HostConfigurationFile::new(*name, HostConfigurationFileReset::ByEngine)),
    );
    files
}

fn source_files() -> BTreeMap<&'static str, Vec<u8>> {
    BTreeMap::from([
        (
            PRODUCT,
            br#"{"version":1,"general":{"trayIcon":"cat","theme":"dark"}}"#.to_vec(),
        ),
        (NOTIFIED, br#"{"version":"9.9.1-synthetic"}"#.to_vec()),
        (SKIPPED, br#"{"version":"9.9.0-synthetic"}"#.to_vec()),
        (
            THROTTLE,
            br#"{"lastPromptAt":"2026-10-21T00:00:00Z"}"#.to_vec(),
        ),
        (TELEMETRY, b"false".to_vec()),
        (UNLISTED, br#"{"source":"synthetic-unlisted"}"#.to_vec()),
    ])
}

#[tokio::test(flavor = "multi_thread")]
async fn declared_files_return_to_their_host_paths_after_export_and_import() {
    let mut report = Report::new("round-trip");
    let root = tempfile::tempdir().unwrap();
    let outbox = root.path().join("host-files");
    std::fs::create_dir_all(&outbox).unwrap();

    // 来源：声明文件、遥测与未声明文件都在数据根下。
    let source = Profile::new(root.path().join("source"), &outbox);
    for (name, bytes) in source_files() {
        source.write(name, &bytes);
    }
    report.tree("source data root (before export)", &source.private);
    let engine = source.start(desktop_declaration()).await;
    create_space(&engine).await;
    assert_eq!(
        engine
            .execute(Operation::ExportConfig(ExportConfigInput {
                destination: HostFileHandle::new("round-trip.ucbundle"),
            }))
            .await
            .unwrap(),
        OperationResult::ConfigExport(ConfigExportOutcome::Exported)
    );
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
    report.file("bundle", &outbox.join("round-trip.ucbundle"));

    // 目标一：完整声明。目标已有不同的遥测与未声明文件，`skipped_version.json` 是指向数据根外的链接。
    let target = Profile::new(root.path().join("target"), &outbox);
    target.write(TELEMETRY, b"true");
    target.write(UNLISTED, br#"{"target":"keep"}"#);
    let outside = root.path().join("outside-target.json");
    std::fs::write(&outside, b"outside must stay untouched").unwrap();
    std::fs::create_dir_all(&target.private).unwrap();
    std::os::unix::fs::symlink(&outside, target.private.join(SKIPPED)).unwrap();
    report.tree("target data root (before stage)", &target.private);

    let engine = target.start(desktop_declaration()).await;
    stage(&engine, "round-trip.ucbundle", PASSPHRASE).await;
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
    let staged = target.private.join("import-staging");
    report.tree("target import-staging (decrypted members)", &staged);
    let staged_host_members = names_in(&staged.join("ui-state"));
    assert_eq!(
        staged_host_members,
        vec![PRODUCT, NOTIFIED, SKIPPED, THROTTLE],
        "only declared files travel; telemetry and unlisted files stay outside the bundle"
    );

    // 下一次启动在打开资料前应用暂存导入。
    let engine = target.start(desktop_declaration()).await;
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
    report.tree("target data root (after apply)", &target.private);
    let expected = source_files();
    for name in [PRODUCT, NOTIFIED, SKIPPED, THROTTLE] {
        let path = target.private.join(name);
        assert!(
            std::fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_file(),
            "{name} must be a regular file at the host path"
        );
        assert_eq!(std::fs::read(&path).unwrap(), expected[name], "{name}");
    }
    assert_eq!(target.read(TELEMETRY), b"true");
    assert_eq!(target.read(UNLISTED), br#"{"target":"keep"}"#);
    assert_eq!(
        std::fs::read(&outside).unwrap(),
        b"outside must stay untouched"
    );
    assert!(!target.private.join("ui-state").exists());
    assert!(!staged.exists());
    assert!(!target.private.join("pending-import.json").exists());

    // 目标二：只声明产品文件。配置包里其他成员不得写入数据根。
    let narrow = Profile::new(root.path().join("narrow-target"), &outbox);
    let product_only = vec![HostConfigurationFile::new(
        PRODUCT,
        HostConfigurationFileReset::ByHost,
    )];
    let engine = narrow.start(product_only.clone()).await;
    stage(&engine, "round-trip.ucbundle", PASSPHRASE).await;
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
    let engine = narrow.start(product_only).await;
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
    report.tree("narrow target data root (after apply)", &narrow.private);
    assert_eq!(narrow.read(PRODUCT), expected[PRODUCT]);
    for name in ENGINE_RESET_FILES {
        assert!(!narrow.private.join(name).exists(), "{name} is undeclared");
    }
    assert!(!narrow.private.join("ui-state").exists());
    report.finish();
}

#[tokio::test(flavor = "multi_thread")]
async fn export_skips_missing_declared_files_and_refuses_symlinks() {
    let mut report = Report::new("export-edges");
    let root = tempfile::tempdir().unwrap();
    let outbox = root.path().join("host-files");
    std::fs::create_dir_all(&outbox).unwrap();
    let source = Profile::new(root.path().join("source"), &outbox);
    let files = source_files();
    source.write(PRODUCT, &files[PRODUCT]);
    source.write(NOTIFIED, &files[NOTIFIED]);
    let engine = source.start(desktop_declaration()).await;
    create_space(&engine).await;

    assert_eq!(
        engine
            .execute(Operation::ExportConfig(ExportConfigInput {
                destination: HostFileHandle::new("missing.ucbundle"),
            }))
            .await
            .unwrap(),
        OperationResult::ConfigExport(ConfigExportOutcome::Exported)
    );
    report.file(
        "bundle with two declared files missing",
        &outbox.join("missing.ucbundle"),
    );

    let outside = root.path().join("outside-source.json");
    std::fs::write(&outside, b"never bundle me").unwrap();
    std::fs::remove_file(source.private.join(PRODUCT)).unwrap();
    std::os::unix::fs::symlink(&outside, source.private.join(PRODUCT)).unwrap();
    let refused = engine
        .execute(Operation::ExportConfig(ExportConfigInput {
            destination: HostFileHandle::new("symlink.ucbundle"),
        }))
        .await
        .unwrap_err();
    assert_eq!(refused.code(), EXPORT_CONFIG_FAILED_CODE);
    assert!(!outbox.join("symlink.ucbundle").exists());
    engine.shutdown(Duration::from_secs(15)).await.unwrap();

    let target = Profile::new(root.path().join("target"), &outbox);
    let engine = target.start(desktop_declaration()).await;
    stage(&engine, "missing.ucbundle", PASSPHRASE).await;
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
    report.tree(
        "target import-staging (decrypted members)",
        &target.private.join("import-staging"),
    );
    assert_eq!(
        names_in(&target.private.join("import-staging/ui-state")),
        vec![PRODUCT, NOTIFIED]
    );
    report.finish();
}

#[tokio::test(flavor = "multi_thread")]
async fn factory_reset_deletes_only_engine_owned_host_files_after_keys_are_wiped() {
    let mut report = Report::new("factory-reset");
    let root = tempfile::tempdir().unwrap();
    let outbox = root.path().join("host-files");
    std::fs::create_dir_all(&outbox).unwrap();
    let profile = Profile::new(root.path().join("profile"), &outbox);
    let files = source_files();
    let engine = profile.start(desktop_declaration()).await;
    create_space(&engine).await;
    for (name, bytes) in &files {
        profile.write(name, bytes);
    }
    // Engine 删除的声明文件若是链接，只解除链接本身。
    let outside = root.path().join("outside-reset.json");
    std::fs::write(&outside, b"outside survives reset").unwrap();
    std::fs::remove_file(profile.private.join(NOTIFIED)).unwrap();
    std::os::unix::fs::symlink(&outside, profile.private.join(NOTIFIED)).unwrap();
    report.tree("data root (before reset)", &profile.private);

    // 密钥清除失败：重置未成功，所有宿主文件原样保留。
    profile.storage.fail_deletes.store(true, Ordering::SeqCst);
    let failed = engine
        .execute(Operation::FactoryResetSpace)
        .await
        .unwrap_err();
    report.line(format!("failed reset error code: {}", failed.code()));
    report.tree("data root (after failed reset)", &profile.private);
    for name in [PRODUCT, SKIPPED, THROTTLE, TELEMETRY, UNLISTED] {
        assert_eq!(profile.read(name), files[name], "{name} after failed reset");
    }
    assert!(std::fs::symlink_metadata(profile.private.join(NOTIFIED)).is_ok());

    // 同一操作续做成功：Engine 负责的文件被删除，产品文件、遥测与未声明文件保持原字节。
    profile.storage.fail_deletes.store(false, Ordering::SeqCst);
    assert_eq!(
        engine.execute(Operation::FactoryResetSpace).await.unwrap(),
        OperationResult::SpaceFactoryReset
    );
    report.tree("data root (after successful reset)", &profile.private);
    for name in ENGINE_RESET_FILES {
        assert!(
            std::fs::symlink_metadata(profile.private.join(name)).is_err(),
            "{name} is reset by Engine"
        );
    }
    assert_eq!(std::fs::read(&outside).unwrap(), b"outside survives reset");
    for name in [PRODUCT, TELEMETRY, UNLISTED] {
        assert_eq!(profile.read(name), files[name], "{name} after reset");
    }
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
    report.finish();
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_declarations_stop_start_before_touching_the_data_root() {
    let mut report = Report::new("invalid-declarations");
    let root = tempfile::tempdir().unwrap();
    let outbox = root.path().join("host-files");
    std::fs::create_dir_all(&outbox).unwrap();
    let long_name = format!("{}.json", "a".repeat(60));
    let by_engine =
        |name: &str| HostConfigurationFile::new(name, HostConfigurationFileReset::ByEngine);
    let cases: Vec<(&str, Vec<HostConfigurationFile>)> = vec![
        ("parent traversal", vec![by_engine("../escape.json")]),
        ("separator", vec![by_engine("nested/escape.json")]),
        ("absolute", vec![by_engine("/tmp/escape.json")]),
        ("leading dot", vec![by_engine(".hidden.json")]),
        ("uppercase", vec![by_engine("Settings.JSON")]),
        ("not json", vec![by_engine("notes.txt")]),
        ("empty", vec![by_engine("")]),
        ("too long", vec![by_engine(&long_name)]),
        ("engine settings", vec![by_engine("settings.json")]),
        (
            "engine import marker",
            vec![by_engine("pending-import.json")],
        ),
        (
            "duplicate",
            vec![
                HostConfigurationFile::new(PRODUCT, HostConfigurationFileReset::ByHost),
                by_engine(PRODUCT),
            ],
        ),
    ];
    for (index, (label, declaration)) in cases.into_iter().enumerate() {
        let profile = Profile::new(root.path().join(format!("case-{index}")), &outbox);
        let error = match Engine::start(
            EngineConfig::new("2.0.0").with_host_configuration_files(declaration),
            profile.host(),
        )
        .await
        {
            Ok(_) => panic!("{label}: invalid declaration must stop start"),
            Err(error) => error,
        };
        report.line(format!(
            "{label}: code {} category {:?} data root exists {}",
            error.code(),
            error.category(),
            profile.private.exists()
        ));
        assert_eq!(
            error.code(),
            HOST_CONFIGURATION_FILES_INVALID_CODE,
            "{label}"
        );
        assert_eq!(
            error.category(),
            EngineErrorCategory::InvalidInput,
            "{label}"
        );
        assert!(!error.is_retryable(), "{label}");
        assert!(!profile.private.exists(), "{label}: no data written");
    }
    report.finish();
}

#[tokio::test(flavor = "multi_thread")]
async fn upgrade_backup_keeps_the_whole_data_root_including_declared_files() {
    let mut report = Report::new("upgrade-backup");
    let root = tempfile::tempdir().unwrap();
    let outbox = root.path().join("host-files");
    std::fs::create_dir_all(&outbox).unwrap();
    let profile = Profile::new(root.path().join("profile"), &outbox);
    let files = source_files();
    let engine = profile.start_version("1.2.3", desktop_declaration()).await;
    create_space(&engine).await;
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
    for (name, bytes) in &files {
        profile.write(name, bytes);
    }
    report.tree("data root (before upgrade)", &profile.private);

    let engine = profile.start_version("1.2.4", desktop_declaration()).await;
    engine.shutdown(Duration::from_secs(15)).await.unwrap();

    // 公开接口没有还原操作：此处用 Infra 归档还原到隔离目录，核对 Engine 启动时捕获的备份内容。
    let backup_root = root.path().join("profile/private-upgrade-backups");
    let backup_dir = std::fs::read_dir(&backup_root)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(backup_dir.join("current")).unwrap()).unwrap();
    let receipt: ProfileArchiveReceipt = serde_json::from_value(record["receipt"].clone()).unwrap();
    let restored = root.path().join("restored");
    ProfileBackupArchive::new(backup_dir)
        .restore_to_new_directory(&receipt, &restored)
        .unwrap();
    report.tree("restored backup (isolated directory)", &restored);
    for (name, bytes) in &files {
        assert_eq!(
            std::fs::read(restored.join(name)).unwrap(),
            *bytes,
            "{name}: the backup is a whole data-root copy, declared or not"
        );
    }
    report.finish();
}

#[tokio::test(flavor = "multi_thread")]
async fn bundle_exported_before_declarations_restores_to_host_paths() {
    let mut report = Report::new("legacy-bundle");
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/config-bundle-0c57e1ab");
    let root = tempfile::tempdir().unwrap();
    let outbox = root.path().join("host-files");
    std::fs::create_dir_all(&outbox).unwrap();
    std::fs::copy(
        fixture.join("bundle.ucbundle"),
        outbox.join("legacy.ucbundle"),
    )
    .unwrap();
    report.file("fixture bundle", &outbox.join("legacy.ucbundle"));
    assert_eq!(
        sha256(&std::fs::read(outbox.join("legacy.ucbundle")).unwrap()),
        "6590e08f366d95de9bf66d7e9ce87c2999f70699b963600f24213b31c8b6d105"
    );

    let target = Profile::new(root.path().join("target"), &outbox);
    let engine = target.start(desktop_declaration()).await;
    stage(&engine, "legacy.ucbundle", LEGACY_PASSPHRASE).await;
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
    report.tree(
        "target import-staging (decrypted members)",
        &target.private.join("import-staging"),
    );
    let engine = target.start(desktop_declaration()).await;
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
    report.tree("target data root (after apply)", &target.private);

    for name in [NOTIFIED, SKIPPED] {
        assert_eq!(
            target.read(name),
            std::fs::read(fixture.join(format!("source-{name}"))).unwrap(),
            "{name} restored from the old ui-state member"
        );
    }
    // 旧 Engine 从不导出这些文件：缺失是预期结果，不是恢复成功。
    for name in [THROTTLE, PRODUCT, TELEMETRY, UNLISTED] {
        assert!(
            !target.private.join(name).exists(),
            "{name} was never exported"
        );
    }
    assert!(!target.private.join("ui-state").exists());
    report.line(format!(
        "expected absent (never exported by 0c57e1ab): {THROTTLE}, {PRODUCT}, {TELEMETRY}, {UNLISTED}"
    ));
    report.finish();
}

async fn create_space(engine: &Engine) {
    engine
        .execute(Operation::CreateSpace(CreateSpaceInput {
            device_name: Some("Synthetic Host Files".into()),
            passphrase: SecretString::new(PASSPHRASE),
            passphrase_confirmation: SecretString::new(PASSPHRASE),
        }))
        .await
        .unwrap();
}

async fn stage(engine: &Engine, handle: &str, passphrase: &str) {
    let staged = engine
        .execute(Operation::StageConfigImport(StageConfigImportInput {
            source: HostFileHandle::new(handle),
            password: SecretString::new(passphrase),
        }))
        .await
        .unwrap();
    assert!(matches!(
        staged,
        OperationResult::ConfigImportStaged(ConfigImportStageOutcome::Staged { .. })
    ));
}

/// 一个隔离安装：数据根位于 `<root>/private`，宿主文件句柄映射到共享的 `outbox` 目录。
struct Profile {
    root: PathBuf,
    private: PathBuf,
    outbox: PathBuf,
    storage: MemorySecureStorage,
}

impl Profile {
    fn new(root: PathBuf, outbox: &Path) -> Self {
        Self {
            private: root.join("private"),
            root,
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
                self.root.join("logs"),
            ),
            Box::new(self.storage.clone()),
            Box::new(EmptyClipboard),
            Box::new(DirectoryFiles(self.outbox.clone())),
        )
    }

    async fn start(&self, declaration: Vec<HostConfigurationFile>) -> Engine {
        self.start_version("2.0.0", declaration).await
    }

    async fn start_version(
        &self,
        version: &str,
        declaration: Vec<HostConfigurationFile>,
    ) -> Engine {
        let (engine, _events) = Engine::start(
            EngineConfig::new(version).with_host_configuration_files(declaration),
            self.host(),
        )
        .await
        .unwrap();
        engine
    }

    fn write(&self, name: &str, bytes: &[u8]) {
        std::fs::create_dir_all(&self.private).unwrap();
        std::fs::write(self.private.join(name), bytes).unwrap();
    }

    fn read(&self, name: &str) -> Vec<u8> {
        std::fs::read(self.private.join(name)).unwrap()
    }
}

#[derive(Clone, Default)]
struct MemorySecureStorage {
    values: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    fail_deletes: Arc<AtomicBool>,
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
        if self.fail_deletes.load(Ordering::SeqCst) {
            return Err(HostCapabilityError::new(
                HostCapabilityErrorCategory::Unavailable,
                "secure storage delete failure injected by reset test",
            ));
        }
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

fn names_in(directory: &Path) -> Vec<&'static str> {
    let mut names: Vec<String> = match std::fs::read_dir(directory) {
        Ok(entries) => entries
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect(),
        Err(_) => Vec::new(),
    };
    names.sort();
    let order = [PRODUCT, NOTIFIED, SKIPPED, THROTTLE, TELEMETRY, UNLISTED];
    let mut known: Vec<&'static str> = order
        .into_iter()
        .filter(|name| names.iter().any(|found| found == name))
        .collect();
    assert_eq!(known.len(), names.len(), "unexpected members: {names:?}");
    known.sort_by_key(|name| order.iter().position(|candidate| candidate == name));
    known
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

/// 场景证据：目录树（相对路径、类型、sha256）与关键结果，写入 `UC_TEST_ARTIFACTS_DIR`。
struct Report {
    scenario: &'static str,
    text: String,
}

impl Report {
    fn new(scenario: &'static str) -> Self {
        Self {
            scenario,
            text: format!("scenario: {scenario}\n"),
        }
    }

    fn line(&mut self, line: String) {
        self.text.push_str(&line);
        self.text.push('\n');
    }

    fn file(&mut self, label: &str, path: &Path) {
        let bytes = std::fs::read(path).unwrap();
        self.line(format!(
            "\n[{label}]\n{}  {} bytes  {}",
            sha256(&bytes),
            bytes.len(),
            path.file_name().unwrap().to_string_lossy()
        ));
    }

    fn tree(&mut self, label: &str, root: &Path) {
        self.line(format!("\n[{label}]"));
        let mut entries = Vec::new();
        collect(root, root, &mut entries);
        entries.sort();
        if entries.is_empty() {
            self.line("(absent or empty)".to_owned());
        }
        for entry in entries {
            self.line(entry);
        }
    }

    fn finish(self) {
        let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") else {
            return;
        };
        let directory = PathBuf::from(directory)
            .join("host-configuration-files")
            .join(self.scenario);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("inventory.txt"), self.text).unwrap();
    }
}

/// 数据库、日志等运行文件内容随运行变化，只记录宿主文件与暂存成员的哈希，其余记录类型与大小。
fn collect(root: &Path, directory: &Path, entries: &mut Vec<String>) {
    let Ok(read) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in read {
        let entry = entry.unwrap();
        let path = entry.path();
        let relative = path.strip_prefix(root).unwrap().display().to_string();
        let metadata = std::fs::symlink_metadata(&path).unwrap();
        if metadata.file_type().is_symlink() {
            entries.push(format!("symlink  {relative}"));
        } else if metadata.is_dir() {
            entries.push(format!("dir      {relative}/"));
            collect(root, &path, entries);
        } else {
            let bytes = std::fs::read(&path).unwrap();
            entries.push(format!(
                "{}  {:>8}  {relative}",
                sha256(&bytes),
                bytes.len()
            ));
        }
    }
}
