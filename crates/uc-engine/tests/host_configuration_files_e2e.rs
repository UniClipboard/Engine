//! 宿主声明的附加配置文件：经公开 `Engine::start` 与操作，用真实文件验证配置导出、导入恢复、
//! 恢复出厂与升级备份的完整行为。
//!
//! 每个场景使用独立临时数据根和合成文件。设置 `UC_TEST_ARTIFACTS_DIR` 时，各场景把输入、配置包、
//! 暂存目录和结果目录的清单与 sha256 写到 `<dir>/host-configuration-files/<场景>/inventory.txt`。

#![cfg(unix)]

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
    EngineErrorCategory, ExportConfigInput, HistoryEntryInput, HostCapabilities,
    HostCapabilityError, HostCapabilityErrorCategory, HostClipboard, HostClipboardSnapshot,
    HostConfigurationFile, HostConfigurationFileReset, HostDirectories, HostFileAccess,
    HostFileHandle, HostFileMetadata, HostSecureStorage, Operation, OperationResult, SecretString,
    SendTextInput, StageConfigImportInput, UnlockSpaceInput,
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
async fn new_install_import_starts_on_first_and_subsequent_boot() {
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
    let source_space = assert_ready(&engine, &mut report).await;
    let content = "synthetic encrypted blob content ".repeat(800);
    let source_entry = send_text(&engine, &content).await;
    let exported = engine
        .execute(Operation::ExportConfig(ExportConfigInput {
            destination: HostFileHandle::new("round-trip.ucbundle"),
        }))
        .await
        .unwrap();
    assert_eq!(
        exported,
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
    let mut blob_inventory = Vec::new();
    collect(
        &staged.join("runtime-blobs"),
        &staged.join("runtime-blobs"),
        &mut blob_inventory,
    );
    assert!(
        blob_inventory
            .iter()
            .any(|entry| !entry.starts_with("dir ")),
        "large encrypted source content must travel as blobs"
    );
    let staged_host_members = names_in(&staged.join("ui-state"));
    assert_eq!(
        staged_host_members,
        vec![PRODUCT, NOTIFIED, SKIPPED, THROTTLE],
        "only declared files travel; telemetry and unlisted files stay outside the bundle"
    );

    // 下一次启动在打开资料前应用暂存导入。
    let engine = target.start(desktop_declaration()).await;
    assert_eq!(assert_ready(&engine, &mut report).await, source_space);
    assert_history(&engine, &source_entry, &content).await;
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
    let engine = target.start(desktop_declaration()).await;
    assert_eq!(assert_ready(&engine, &mut report).await, source_space);
    assert_history(&engine, &source_entry, &content).await;
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
    assert_ready(&engine, &mut report).await;
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
async fn same_install_import_retries_partial_copy_and_keeps_missing_members() {
    let mut report = Report::new("same-install-partial-copy");
    let root = tempfile::tempdir().unwrap();
    let outbox = root.path().join("host-files");
    std::fs::create_dir_all(&outbox).unwrap();
    let profile = Profile::new(root.path().join("profile"), &outbox);
    let expected = source_files();
    for (name, bytes) in &expected {
        profile.write(name, bytes);
    }
    let engine = profile.start(desktop_declaration()).await;
    create_space(&engine).await;
    // 配置包缺少该文件，导入必须保留导出后写入的本机值。
    std::fs::remove_file(profile.private.join(THROTTLE)).unwrap();
    engine
        .execute(Operation::ExportConfig(ExportConfigInput {
            destination: HostFileHandle::new("same-install.ucbundle"),
        }))
        .await
        .unwrap();
    report.file(
        "original same-install bundle",
        &outbox.join("same-install.ucbundle"),
    );
    for name in [PRODUCT, NOTIFIED, SKIPPED, THROTTLE] {
        profile.write(name, b"modified after export");
    }
    stage(&engine, "same-install.ucbundle", PASSPHRASE).await;
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
    report.tree(
        "staging before apply",
        &profile.private.join("import-staging"),
    );
    // 前两个成员恢复后，最后一个存在的声明成员因目录阻挡而复制失败。
    std::fs::remove_file(profile.private.join(SKIPPED)).unwrap();
    std::fs::create_dir(profile.private.join(SKIPPED)).unwrap();
    std::fs::write(
        profile.private.join(SKIPPED).join("keep"),
        b"do not delete directories",
    )
    .unwrap();
    let outside_temporary = root.path().join("outside-temporary.json");
    std::fs::write(&outside_temporary, b"temporary link must not be followed").unwrap();
    std::os::unix::fs::symlink(
        &outside_temporary,
        profile.private.join(format!(".{PRODUCT}.import-tmp")),
    )
    .unwrap();
    let error = match profile.try_start(desktop_declaration()).await {
        Ok(_) => panic!("directory destination must fail apply"),
        Err(error) => error,
    };
    report.line(format!("obstructed apply code: {}", error.code()));
    assert!(profile.private.join("pending-import.json").exists());
    assert!(profile.private.join("import-staging").exists());
    assert_eq!(profile.read(PRODUCT), expected[PRODUCT]);
    assert_eq!(
        std::fs::read(&outside_temporary).unwrap(),
        b"temporary link must not be followed"
    );
    assert_eq!(profile.read(NOTIFIED), expected[NOTIFIED]);
    assert!(profile.private.join(SKIPPED).join("keep").exists());
    report.tree("partial restore", &profile.private);
    std::fs::remove_file(profile.private.join(SKIPPED).join("keep")).unwrap();
    std::fs::remove_dir(profile.private.join(SKIPPED)).unwrap();
    let outside = root.path().join("outside.json");
    std::fs::write(&outside, b"outside survives restore").unwrap();
    std::os::unix::fs::symlink(&outside, profile.private.join(SKIPPED)).unwrap();
    let engine = profile.start(desktop_declaration()).await;
    let setup = engine.execute(Operation::QuerySetupState).await.unwrap();
    assert!(
        matches!(&setup, OperationResult::SetupState(state) if state.has_completed && state.space_id.is_some() && !state.re_pairing_required)
    );
    let encryption = engine
        .execute(Operation::QueryEncryptionState)
        .await
        .unwrap();
    assert!(
        matches!(&encryption, OperationResult::EncryptionState(state) if state.initialized && state.session_ready)
    );
    report.line(format!("same-install setup: {setup:?}"));
    report.line(format!("same-install encryption: {encryption:?}"));
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
    for name in [PRODUCT, NOTIFIED, SKIPPED] {
        assert_eq!(profile.read(name), expected[name]);
    }
    assert_eq!(profile.read(THROTTLE), b"modified after export");
    for name in [TELEMETRY, UNLISTED] {
        assert_eq!(profile.read(name), expected[name]);
    }
    assert_eq!(std::fs::read(outside).unwrap(), b"outside survives restore");
    assert!(!profile.private.join("pending-import.json").exists());
    assert!(!profile.private.join("import-staging").exists());
    report.tree("successful retry", &profile.private);
    report.line(
        "same original profile and same original in-memory secure storage; no identity or key copy"
            .into(),
    );
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
    std::fs::remove_file(source.private.join(PRODUCT)).unwrap();
    std::fs::create_dir(source.private.join(PRODUCT)).unwrap();
    let directory_error = engine
        .execute(Operation::ExportConfig(ExportConfigInput {
            destination: HostFileHandle::new("directory.ucbundle"),
        }))
        .await
        .unwrap_err();
    assert_eq!(directory_error.code(), EXPORT_CONFIG_FAILED_CODE);
    assert!(!outbox.join("directory.ucbundle").exists());
    std::fs::remove_dir(source.private.join(PRODUCT)).unwrap();
    source.write(PRODUCT, &vec![b'x'; 1024 * 1024 + 1]);
    let oversized_error = engine
        .execute(Operation::ExportConfig(ExportConfigInput {
            destination: HostFileHandle::new("oversized.ucbundle"),
        }))
        .await
        .unwrap_err();
    assert_eq!(oversized_error.code(), EXPORT_CONFIG_FAILED_CODE);
    assert!(!outbox.join("oversized.ucbundle").exists());
    report.line(format!(
        "symlink={}, directory={}, oversized={}",
        refused.code(),
        directory_error.code(),
        oversized_error.code()
    ));
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
    std::fs::remove_file(profile.private.join(SKIPPED)).unwrap();
    std::fs::create_dir(profile.private.join(SKIPPED)).unwrap();
    std::fs::write(
        profile.private.join(SKIPPED).join("keep"),
        b"keep directory contents",
    )
    .unwrap();
    let directory_error = engine
        .execute(Operation::FactoryResetSpace)
        .await
        .unwrap_err();
    report.line(format!(
        "directory reset error code: {}",
        directory_error.code()
    ));
    assert!(profile.private.join(SKIPPED).join("keep").exists());
    for name in [PRODUCT, TELEMETRY, UNLISTED] {
        assert_eq!(profile.read(name), files[name]);
    }
    std::fs::remove_file(profile.private.join(SKIPPED).join("keep")).unwrap();
    std::fs::remove_dir(profile.private.join(SKIPPED)).unwrap();
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
    report.tree("verified upgrade backup archive and receipt", &backup_dir);
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
async fn existing_install_import_recovers_partial_adoption_and_retains_usable_backup() {
    let mut report = Report::new("existing-install-adoption");
    let root = tempfile::tempdir().unwrap();
    let outbox = root.path().join("outbox");
    std::fs::create_dir_all(&outbox).unwrap();
    let source = Profile::new(root.path().join("source"), &outbox);
    let engine = source.start(desktop_declaration()).await;
    create_space(&engine).await;
    let source_space = assert_ready(&engine, &mut report).await;
    let source_content = "synthetic cross-install encrypted blob ".repeat(800);
    let source_entry = send_text(&engine, &source_content).await;
    for (name, bytes) in source_files() {
        source.write(name, &bytes);
    }
    engine
        .execute(Operation::ExportConfig(ExportConfigInput {
            destination: HostFileHandle::new("existing.ucbundle"),
        }))
        .await
        .unwrap();
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
    report.file("complete source bundle", &outbox.join("existing.ucbundle"));

    let target = Profile::new(root.path().join("target"), &outbox);
    const TARGET_PASSPHRASE: &str = "different-synthetic-target-passphrase";
    let engine = target.start_version("1.2.3", desktop_declaration()).await;
    create_space_with_passphrase(&engine, TARGET_PASSPHRASE).await;
    let target_space = assert_ready(&engine, &mut report).await;
    assert_ne!(source_space, target_space);
    let target_entry = send_text(&engine, "target history before import").await;
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
    // 版本变化先通过真实启动建立旧目标的升级备份。
    let engine = target.start(desktop_declaration()).await;
    stage(&engine, "existing.ucbundle", PASSPHRASE).await;
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
    target.write(TELEMETRY, b"target telemetry survives");
    target.write(UNLISTED, b"target unrelated root survives");
    std::fs::write(
        target.private.join("vault/unrelated-sentinel"),
        b"target vault sentinel",
    )
    .unwrap();
    std::fs::create_dir_all(target.private.join("file-cache")).unwrap();
    std::fs::write(
        target.private.join("file-cache/unrelated-sentinel"),
        b"target cached bytes",
    )
    .unwrap();
    std::fs::create_dir_all(target.root.join("cache/spool")).unwrap();
    std::fs::write(
        target.root.join("cache/spool/unrelated-sentinel"),
        b"target pending bytes",
    )
    .unwrap();
    let old_vault = std::fs::read(target.private.join("vault/profile-secrets-v1")).unwrap();
    report.tree("target before adoption", &target.private);
    report.tree(
        "target backup before adoption",
        &target.root.join("private-upgrade-backups"),
    );

    // 首次失败发生于安全存储写入，原件和核验归档已经保留。
    target.storage.fail_sets.store(true, Ordering::SeqCst);
    assert!(target.try_start(desktop_declaration()).await.is_err());
    target.storage.fail_sets.store(false, Ordering::SeqCst);
    assert!(target.private.join("pending-import.json").exists());
    let recovery = target.root.join(".uc-config-import-recovery");
    let transaction = only_child(&only_child(&recovery));
    let receipt_bytes = std::fs::read(transaction.join("receipt.json")).unwrap();
    report.tree("after secure-storage write failure", &recovery);
    // 第二次失败发生于 KEK 已写入，但 keyslot 文件尚未安装。
    *target.storage.block_after_kek.lock().unwrap() =
        Some(target.private.join("vault/keyslot.json"));
    assert!(target.try_start(desktop_declaration()).await.is_err());
    assert!(target.private.join("pending-import.json").exists());
    assert!(target.private.join(".config-import-adoption-v1").exists());
    assert!(target.private.join("import-staging").exists());
    assert_eq!(
        receipt_bytes,
        std::fs::read(transaction.join("receipt.json")).unwrap()
    );
    report.tree("partial adoption before retry", &target.private);
    std::fs::remove_dir(target.private.join("vault/keyslot.json")).unwrap();

    let engine = target.start(desktop_declaration()).await;
    assert_eq!(assert_ready(&engine, &mut report).await, source_space);
    assert_history(&engine, &source_entry, &source_content).await;
    let new_entry = send_text(&engine, "imported installation can encrypt new content").await;
    assert_history(
        &engine,
        &new_entry,
        "imported installation can encrypt new content",
    )
    .await;
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
    let engine = target.start(desktop_declaration()).await;
    assert_eq!(assert_ready(&engine, &mut report).await, source_space);
    assert_history(&engine, &source_entry, &source_content).await;
    let new_entry = send_text(&engine, "imported installation can encrypt new content").await;
    assert_history(
        &engine,
        &new_entry,
        "imported installation can encrypt new content",
    )
    .await;
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
    assert_eq!(
        receipt_bytes,
        std::fs::read(transaction.join("receipt.json")).unwrap()
    );
    assert_eq!(target.read(TELEMETRY), b"target telemetry survives");
    assert_eq!(target.read(UNLISTED), b"target unrelated root survives");
    let originals = transaction.join("originals");
    assert_eq!(
        old_vault,
        std::fs::read(originals.join("vault/profile-secrets-v1")).unwrap()
    );
    assert_eq!(
        std::fs::read(originals.join("vault/unrelated-sentinel")).unwrap(),
        b"target vault sentinel"
    );
    assert_eq!(
        std::fs::read(originals.join("file-cache/unrelated-sentinel")).unwrap(),
        b"target cached bytes"
    );
    assert_eq!(
        std::fs::read(originals.join("spool/unrelated-sentinel")).unwrap(),
        b"target pending bytes"
    );
    assert!(originals.join("upgrade-backups/current").is_file());
    for name in [PRODUCT, NOTIFIED, SKIPPED, THROTTLE] {
        assert_eq!(target.read(name), source_files()[name]);
    }
    for name in [
        "pending-import.json",
        "import-staging",
        ".config-import-adoption-v1",
    ] {
        assert!(!target.private.join(name).exists());
    }
    report.tree("retained target state and verified backup", &recovery);
    report.tree("imported target", &target.private);

    // 从核验归档恢复到另一个隔离安装，再用旧目标口令走公开恢复；不复制任何安全存储。
    let restored = Profile::new(root.path().join("restored-original"), &outbox);
    let receipt: ProfileArchiveReceipt = serde_json::from_slice(&receipt_bytes).unwrap();
    let old_backup: serde_json::Value =
        serde_json::from_slice(&std::fs::read(originals.join("upgrade-backups/current")).unwrap())
            .unwrap();
    let old_receipt: ProfileArchiveReceipt =
        serde_json::from_value(old_backup["receipt"].clone()).unwrap();
    ProfileBackupArchive::new(originals.join("upgrade-backups"))
        .verify(&old_receipt)
        .unwrap();
    std::fs::create_dir_all(&restored.root).unwrap();
    ProfileBackupArchive::new(transaction.join("archive"))
        .restore_to_new_directory(&receipt, &restored.private)
        .unwrap();
    let engine = restored.start(desktop_declaration()).await;
    engine
        .execute(Operation::UnlockSpace(UnlockSpaceInput {
            passphrase: SecretString::new(TARGET_PASSPHRASE),
        }))
        .await
        .unwrap();
    assert_eq!(assert_ready(&engine, &mut report).await, target_space);
    assert_history(&engine, &target_entry, "target history before import").await;
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
    report.tree(
        "restored original target after public passphrase recovery",
        &restored.private,
    );
    report.finish();
}

fn only_child(path: &Path) -> PathBuf {
    let mut entries = std::fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().path());
    let child = entries.next().unwrap();
    assert!(entries.next().is_none());
    child
}

#[tokio::test(flavor = "multi_thread")]
async fn legacy_bundle_is_rejected_before_target_mutation() {
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
    create_space(&engine).await;
    target.write(PRODUCT, b"target product survives legacy rejection");
    let before = engine.execute(Operation::QuerySetupState).await.unwrap();
    let keys_before = target.storage.values.lock().unwrap().clone();
    let vault_before = std::fs::read(target.private.join("vault/profile-secrets-v1")).unwrap();
    let staged = engine
        .execute(Operation::StageConfigImport(StageConfigImportInput {
            source: HostFileHandle::new("legacy.ucbundle"),
            password: SecretString::new(LEGACY_PASSPHRASE),
        }))
        .await
        .unwrap();
    assert!(
        matches!(&staged, OperationResult::ConfigImportStaged(ConfigImportStageOutcome::Incompatible { reason }) if reason.contains("re-export"))
    );
    report.line(format!("public legacy rejection: {staged:?}"));
    assert_eq!(
        before,
        engine.execute(Operation::QuerySetupState).await.unwrap()
    );
    assert_eq!(keys_before, *target.storage.values.lock().unwrap());
    assert_eq!(
        vault_before,
        std::fs::read(target.private.join("vault/profile-secrets-v1")).unwrap()
    );
    assert_eq!(
        target.read(PRODUCT),
        b"target product survives legacy rejection"
    );
    for name in [
        "pending-import.json",
        "import-staging",
        ".config-import-adoption-v1",
    ] {
        assert!(!target.private.join(name).exists());
    }
    assert!(!target.root.join(".uc-config-import-recovery").exists());
    assert_ready(&engine, &mut report).await;
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
    let engine = target.start(desktop_declaration()).await;
    assert_ready(&engine, &mut report).await;
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
    report.tree("target after rejected legacy bundle", &target.private);
    report.finish();
}

/// 升级前暂存的旧包或不完整包不得阻断目标资料的后续启动。
#[tokio::test(flavor = "multi_thread")]
async fn rejected_pending_import_keeps_target_usable_across_restarts() {
    let mut report = Report::new("rejected-pending-import");
    let root = tempfile::tempdir().unwrap();
    let outbox = root.path().join("outbox");
    std::fs::create_dir_all(&outbox).unwrap();
    for schema in [1, 2] {
        let target = Profile::new(root.path().join(format!("target-{schema}")), &outbox);
        target.write(PRODUCT, b"target configuration");
        let engine = target.start(desktop_declaration()).await;
        create_space(&engine).await;
        let space = assert_ready(&engine, &mut report).await;
        let content = "synthetic retained history";
        let entry = send_text(&engine, content).await;
        engine.shutdown(Duration::from_secs(15)).await.unwrap();
        target.write(
            "pending-import.json",
            &serde_json::to_vec(&serde_json::json!({
                "schema_ver": schema, "staging_dir": "import-staging",
                "has_kek": true, "staged_at_unix_ms": 0
            }))
            .unwrap(),
        );
        let staging = target.private.join("import-staging/db");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("uniclipboard.db"), b"incomplete source").unwrap();
        for _ in 0..2 {
            let engine = target.start(desktop_declaration()).await;
            assert_eq!(assert_ready(&engine, &mut report).await, space);
            assert_history(&engine, &entry, content).await;
            engine.shutdown(Duration::from_secs(15)).await.unwrap();
        }
        assert!(!target.private.join("pending-import.json").exists());
        assert!(!target.private.join("import-staging").exists());
        assert!(!target.private.join(".config-import-adoption-v1").exists());
        assert_eq!(target.read(PRODUCT), b"target configuration");
        report.tree("retained target", &target.private);
    }
    report.finish();
}

/// 超出累计上限的稀疏密文文件应在读取内容前拒绝，且不覆盖导出目标。
#[tokio::test(flavor = "multi_thread")]
async fn cumulative_blob_export_limit_preserves_destination() {
    let mut report = Report::new("cumulative-export-limit");
    let root = tempfile::tempdir().unwrap();
    let outbox = root.path().join("outbox");
    std::fs::create_dir_all(&outbox).unwrap();
    let source = Profile::new(root.path().join("source"), &outbox);
    let engine = source.start(desktop_declaration()).await;
    create_space(&engine).await;
    let generation = std::fs::read_dir(source.private.join("profile-data-generations"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let blobs = generation.join("v3-payloads/blobs");
    std::fs::create_dir_all(&blobs).unwrap();
    for name in ["synthetic-sparse-a", "synthetic-sparse-b"] {
        std::fs::File::create(blobs.join(name))
            .unwrap()
            .set_len(1024 * 1024 * 1024)
            .unwrap();
    }
    let destination = outbox.join("bounded.ucbundle");
    std::fs::write(&destination, b"existing destination").unwrap();
    let error = tokio::time::timeout(
        Duration::from_secs(15),
        engine.execute(Operation::ExportConfig(ExportConfigInput {
            destination: HostFileHandle::new("bounded.ucbundle"),
        })),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert_eq!(error.code(), EXPORT_CONFIG_FAILED_CODE);
    assert_eq!(
        std::fs::read(&destination).unwrap(),
        b"existing destination"
    );
    assert!(!outbox.join("bounded.ucbundle.tmp").exists());
    report.line(format!(
        "two sparse 1 GiB members rejected: {}",
        error.code()
    ));
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
    report.finish();
}

async fn send_text(engine: &Engine, text: &str) -> String {
    let result = engine
        .execute(Operation::SendText(SendTextInput {
            text: text.into(),
            target_devices: Vec::new(),
        }))
        .await
        .unwrap();
    let OperationResult::EntrySent(sent) = result else {
        panic!("expected saved entry")
    };
    sent.entry_id
}

async fn assert_history(engine: &Engine, entry_id: &str, expected: &str) {
    let result = engine
        .execute(Operation::GetHistoryEntry(HistoryEntryInput {
            entry_id: entry_id.into(),
        }))
        .await
        .unwrap();
    let OperationResult::HistoryEntry(entry) = result else {
        panic!("expected history entry")
    };
    assert_eq!(entry.content, expected);
}

async fn assert_ready(engine: &Engine, report: &mut Report) -> String {
    let setup = engine.execute(Operation::QuerySetupState).await.unwrap();
    assert!(
        matches!(&setup, OperationResult::SetupState(state) if state.has_completed && state.space_id.is_some() && !state.re_pairing_required)
    );
    let encryption = engine
        .execute(Operation::QueryEncryptionState)
        .await
        .unwrap();
    assert!(
        matches!(&encryption, OperationResult::EncryptionState(state) if state.initialized && state.session_ready)
    );
    report.line(format!("public setup: {setup:?}"));
    report.line(format!("public encryption: {encryption:?}"));
    let OperationResult::SetupState(state) = setup else {
        unreachable!()
    };
    state.space_id.unwrap()
}

async fn create_space(engine: &Engine) {
    create_space_with_passphrase(engine, PASSPHRASE).await;
}

async fn create_space_with_passphrase(engine: &Engine, passphrase: &str) {
    engine
        .execute(Operation::CreateSpace(CreateSpaceInput {
            device_name: Some("Synthetic Host Files".into()),
            passphrase: SecretString::new(passphrase),
            passphrase_confirmation: SecretString::new(passphrase),
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

    async fn try_start(
        &self,
        declaration: Vec<HostConfigurationFile>,
    ) -> Result<Engine, uc_engine::EngineError> {
        Engine::start(
            EngineConfig::new("2.0.0").with_host_configuration_files(declaration),
            self.host(),
        )
        .await
        .map(|(engine, _)| engine)
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
    fail_sets: Arc<AtomicBool>,
    block_after_kek: Arc<Mutex<Option<PathBuf>>>,
}

impl HostSecureStorage for MemorySecureStorage {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, HostCapabilityError> {
        Ok(self.values.lock().unwrap().get(key).cloned())
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<(), HostCapabilityError> {
        if self.fail_sets.load(Ordering::SeqCst) {
            return Err(HostCapabilityError::new(
                HostCapabilityErrorCategory::Unavailable,
                "synthetic secure-storage write failure",
            ));
        }
        self.values
            .lock()
            .unwrap()
            .insert(key.to_owned(), value.to_vec());
        if key.starts_with("kek:v1:") {
            if let Some(path) = self.block_after_kek.lock().unwrap().take() {
                std::fs::create_dir_all(path).unwrap();
            }
        }
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
        let _ = tracing_subscriber::fmt()
            .with_env_filter("uc_engine=info,uc_infra_profile=info")
            .with_ansi(false)
            .try_init();
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
        if let Some(directory) = self.artifact_directory() {
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(directory.join(path.file_name().unwrap()), &bytes).unwrap();
        }
        self.line(format!(
            "\n[{label}]\n{}  {} bytes  {}",
            sha256(&bytes),
            bytes.len(),
            path.file_name().unwrap().to_string_lossy()
        ));
    }

    fn tree(&mut self, label: &str, root: &Path) {
        self.line(format!("\n[{label}]\nsource: {}", root.display()));
        if let Some(directory) = self.artifact_directory() {
            let snapshot = directory.join(format!("snapshot-{}", self.text.lines().count()));
            std::fs::create_dir_all(&snapshot).unwrap();
            copy_synthetic_tree(root, &snapshot);
            self.line(format!("synthetic snapshot: {}", snapshot.display()));
        }
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

    fn artifact_directory(&self) -> Option<PathBuf> {
        std::env::var_os("UC_TEST_ARTIFACTS_DIR").map(|directory| {
            PathBuf::from(directory)
                .join("host-configuration-files")
                .join(self.scenario)
        })
    }

    fn finish(&self) {
        let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") else {
            return;
        };
        let directory = PathBuf::from(directory)
            .join("host-configuration-files")
            .join(self.scenario);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("inventory.txt"), &self.text).unwrap();
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

impl Drop for Report {
    fn drop(&mut self) {
        self.finish();
    }
}

fn copy_synthetic_tree(source: &Path, destination: &Path) {
    let Ok(entries) = std::fs::read_dir(source) else {
        return;
    };
    for entry in entries {
        let entry = entry.unwrap();
        let kind = entry.file_type().unwrap();
        let target = destination.join(entry.file_name());
        if kind.is_dir() {
            std::fs::create_dir_all(&target).unwrap();
            copy_synthetic_tree(&entry.path(), &target);
        } else if kind.is_file() {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}
