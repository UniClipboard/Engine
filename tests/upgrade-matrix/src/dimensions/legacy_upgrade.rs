//! D1 旧版快照升级：导入早于 Engine 的 Desktop 资料快照，由当前源码打开、核对、重启并继续使用。
//!
//! 可选的提交中断：首次启动期间外部持有 Space 转换激活租约，使单设备重建在提交边界失败，
//! 留下与竞争失败相同的持久状态；释放后重启必须收敛，且后续重启保持不变。
//!
//! 可选的非法宿主偏好：首次启动前把资料副本中已迁出 Engine 的宿主产品偏好改为非法值或错误类型，
//! 会话仍须正常建立。每个单元最后都经公开操作保存一次设置，并核对落盘文件只含 Engine 拥有的设置。

use std::fs::{self, File, OpenOptions};
use std::path::PathBuf;

use serde_json::{Value, json};
use uc_testkit::{FailureKind, ScenarioFailure};

use crate::{
    catalog::{Interruption, LegacyAnchor, Point},
    cell::CellRun,
    device::{Device, Launch, failure, history_contains},
    dimensions::{require_preserved, upgrade::continue_using},
    fixture::{Content, content_digest, digest, install_legacy_profile},
};

/// Engine 在应用数据根目录下用于串行化 Space 转换激活的租约文件。只有本单元依赖这个内部名称；
/// 名称变化会使首次启动意外成功，单元以 `interruption-not-effective` 显式失败。
const ACTIVATION_LEASE: &str = ".space-transition-activation.lease";

/// Engine 保存后 `settings.json` 允许出现的顶层键与 `general` 键；其余键都是已迁出 Engine 的宿主产品偏好。
const ENGINE_SETTINGS_KEYS: [&str; 9] = [
    "schema_version",
    "general",
    "sync",
    "retention_policy",
    "security",
    "pairing",
    "file_sync",
    "network",
    "mobile_sync",
];
const ENGINE_GENERAL_KEYS: [&str; 1] = ["device_name"];

/// 注入的非法宿主产品偏好：路径与写入值。`general.theme = "neon"` 与 Desktop 任务 4 的 B3 输入相同，
/// 其余覆盖非法枚举与错误 JSON 类型；`general.debug_mode` 已迁出 Engine，错误类型同样不得阻止启动。
/// 不改动任何仍归 Engine 的字段。
fn invalid_preferences() -> [(&'static str, &'static str, Value); 10] {
    [
        ("general", "theme", json!("neon")),
        ("general", "startup_mode", json!("turbo")),
        ("general", "update_channel", json!("nightly")),
        ("general", "language", json!(42)),
        ("general", "usage_analytics_enabled", json!("yes")),
        ("general", "theme_overrides_light", json!("not-a-map")),
        ("general", "debug_mode", json!("yes")),
        ("quick_panel", "position", json!("nowhere")),
        ("quick_panel", "double_tap_modifier", json!("hyper")),
        ("keyboard_shortcuts", "global.toggleQuickPanel", json!(42)),
    ]
}

pub(crate) async fn run(
    run: &mut CellRun,
    from: &LegacyAnchor,
    to: &Point,
    interruption: Option<Interruption>,
) -> Result<(), ScenarioFailure> {
    let mut device = Device::new(run, "a", "Device A")?;
    let snapshot_settings = {
        let _stage = run.stage("import-legacy");
        let storage = install_legacy_profile(from, device.root())?;
        device.preload_secure_storage(storage)?;
        read_settings(&device)?.1
    };
    let interrupted = match interruption {
        Some(Interruption::CommitInterrupted) => {
            Some(interrupt_first_upgrade(run, &mut device, to).await?)
        }
        Some(Interruption::InvalidPreferences) => {
            let _stage = run.stage("invalid-preferences");
            inject_invalid_preferences(run, &device)?;
            None
        }
        None => None,
    };
    {
        let _stage = run.stage("upgrade");
        let startup = device.start(run, to).await?;
        run.fact(&format!("startup-{}-{}", device.label, to.id), startup);
    }
    let upgraded = {
        let _stage = run.stage("verify");
        verify_legacy_state(run, &mut device, from).await?
    };
    {
        let _stage = run.stage("restart");
        if interruption == Some(Interruption::InvalidPreferences) {
            // 首次会话未保存设置时，重启仍带着非法偏好启动，即已运行过 Engine 的资料再次遇到同一输入。
            let still_present = invalid_preferences_present(&device)?;
            run.fact("invalid-preferences-before-restart", json!(still_present));
        }
        device.stop(run).await?;
        device.start(run, to).await?;
        let restarted = device.unlocked_observation().await?;
        require_preserved(run, &upgraded, &restarted, "state-changed-by-restart")?;
    }
    {
        let _stage = run.stage("continue");
        continue_using(&mut device, "a-1").await?;
    }
    {
        let _stage = run.stage("settings-save");
        verify_saved_settings(run, &mut device, from, &snapshot_settings).await?;
    }
    {
        let _stage = run.stage("cleanup");
        device.stop(run).await?;
    }
    // 暂时性租约冲突不能报告为永久失败；放在收敛核对之后，旧行为下先暴露更关键的无法收敛。
    if let Some(interrupted) = interrupted {
        if interrupted["retryable"] != true {
            run.restore_failed_stage("interrupted-upgrade");
            return Err(failure(
                FailureKind::ProductInvariant,
                "interrupted-start-not-retryable",
            ));
        }
    }
    Ok(())
}

/// 首次启动期间持有激活租约，要求启动在提交边界失败，并记录公开错误码与可重试性。
async fn interrupt_first_upgrade(
    run: &mut CellRun,
    device: &mut Device,
    to: &Point,
) -> Result<Value, ScenarioFailure> {
    let _stage = run.stage("interrupted-upgrade");
    let lease = hold_activation_lease(device)?;
    let launched = device.launch(run, to).await;
    drop(lease);
    match launched? {
        Launch::Refused { reply } => {
            let summary = json!({
                "error": reply["error"],
                "code": reply["code"],
                "retryable": reply["retryable"],
                "startup": reply["startup"],
            });
            run.fact("interrupted-start", summary.clone());
            // 干扰必须落在资料升级完成之后的会话恢复（单设备重建），否则测不到提交边界。
            if reply["startup"]["upgrade_completed"] != true {
                return Err(failure(
                    FailureKind::FixtureInvalid,
                    "interruption-before-session-recovery",
                ));
            }
            if reply["error"] != "start_failed" {
                return Err(failure(
                    FailureKind::ProductInvariant,
                    "interrupted-start-unexpected-refusal",
                ));
            }
            Ok(summary)
        }
        Launch::Ready { .. } => {
            device.stop(run).await?;
            Err(failure(
                FailureKind::FixtureInvalid,
                "interruption-not-effective",
            ))
        }
    }
}

fn hold_activation_lease(device: &Device) -> Result<File, ScenarioFailure> {
    let path = device.root().join("private").join(ACTIVATION_LEASE);
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|_| failure(FailureKind::FixtureInvalid, "activation-lease-open"))?;
    // 共享锁足以让 Engine 的排他获取失败；Windows 上排他锁还会阻止资料升级备份读取该文件，
    // 使失败提前到备份阶段而不是重建提交边界。
    file.try_lock_shared()
        .map_err(|_| failure(FailureKind::FixtureInvalid, "activation-lease-held"))?;
    Ok(file)
}

/// 升级后必须完整保留快照中的历史与设备名，并成为只含本机的单设备 Space。
async fn verify_legacy_state(
    run: &mut CellRun,
    device: &mut Device,
    from: &LegacyAnchor,
) -> Result<Value, ScenarioFailure> {
    let observed = device.unlocked_observation().await?;
    run.fact(
        &format!("upgrade-status-{}-{}", device.label, from.id),
        observed["upgrade"].clone(),
    );
    let entries = observed["history"].as_array().map_or(0, Vec::len);
    let preserved = from
        .history
        .iter()
        .all(|text| history_contains(&observed, &content_digest(&Content::Text(text.clone()))));
    run.fact(
        "legacy-history",
        json!({ "expected": from.history.len(), "observed": entries, "all_present": preserved }),
    );
    if !preserved || entries != from.history.len() {
        return Err(failure(
            FailureKind::ProductInvariant,
            "legacy-history-not-preserved",
        ));
    }
    if observed["local_device"]["name"] != from.device_name.as_str() {
        return Err(failure(
            FailureKind::ProductInvariant,
            "legacy-device-name-not-preserved",
        ));
    }
    if observed["devices"] != json!([{ "name": from.device_name, "local": true }]) {
        return Err(failure(
            FailureKind::ProductInvariant,
            "upgraded-space-not-single-device",
        ));
    }
    Ok(observed)
}

fn settings_path(device: &Device) -> PathBuf {
    device.root().join("private").join("settings.json")
}

fn read_settings(device: &Device) -> Result<(Vec<u8>, Value), ScenarioFailure> {
    let bytes = fs::read(settings_path(device))
        .map_err(|_| failure(FailureKind::ProductInvariant, "settings-file-unreadable"))?;
    let value = serde_json::from_slice(&bytes)
        .map_err(|_| failure(FailureKind::ProductInvariant, "settings-file-invalid"))?;
    Ok((bytes, value))
}

/// 只改资料副本：记录快照原文件与改写后文件的哈希，以及改写的路径。
fn inject_invalid_preferences(run: &mut CellRun, device: &Device) -> Result<(), ScenarioFailure> {
    let (source, mut settings) = read_settings(device)?;
    let mut paths = Vec::new();
    for (section, key, value) in invalid_preferences() {
        let Some(section_value) = settings.get_mut(section).and_then(Value::as_object_mut) else {
            return Err(failure(
                FailureKind::FixtureInvalid,
                "invalid-preferences-section-missing",
            ));
        };
        section_value.insert(key.to_owned(), value);
        paths.push(format!("{section}.{key}"));
    }
    let derived = serde_json::to_vec_pretty(&settings)
        .map_err(|_| failure(FailureKind::FixtureInvalid, "invalid-preferences-encode"))?;
    fs::write(settings_path(device), &derived)
        .map_err(|_| failure(FailureKind::FixtureInvalid, "invalid-preferences-write"))?;
    run.fact(
        "invalid-preferences",
        json!({
            "source_sha256": digest(&source),
            "derived_sha256": digest(&derived),
            "paths": paths,
        }),
    );
    Ok(())
}

fn invalid_preferences_present(device: &Device) -> Result<bool, ScenarioFailure> {
    let (_, settings) = read_settings(device)?;
    Ok(invalid_preferences()
        .iter()
        .all(|(section, key, value)| settings[*section].get(*key) == Some(value)))
}

/// 经公开操作保存一次设置，再核对落盘文件：只剩 Engine 拥有的键，快照中的 Engine 设置原样保留，
/// 旧版 `sync.auto_sync` 已写为 `sync.auto_sync_enabled`。
async fn verify_saved_settings(
    run: &mut CellRun,
    device: &mut Device,
    from: &LegacyAnchor,
    snapshot: &Value,
) -> Result<(), ScenarioFailure> {
    let updated = device
        .call(
            "sync_preference",
            json!({ "sync_on_restore": true }),
            "settings-save-failed",
        )
        .await?;
    if updated.get("Rejected").is_some() {
        return Err(failure(
            FailureKind::ProductInvariant,
            "settings-save-rejected",
        ));
    }
    let (bytes, saved) = read_settings(device)?;
    let keys = |value: &Value| -> Vec<String> {
        value
            .as_object()
            .map(|object| object.keys().cloned().collect())
            .unwrap_or_default()
    };
    let top_level = keys(&saved);
    let general = keys(&saved["general"]);
    run.fact(
        "saved-settings",
        json!({ "sha256": digest(&bytes), "keys": top_level, "general_keys": general }),
    );
    let only_engine_keys = top_level
        .iter()
        .all(|key| ENGINE_SETTINGS_KEYS.contains(&key.as_str()))
        && general
            .iter()
            .all(|key| ENGINE_GENERAL_KEYS.contains(&key.as_str()));
    if !only_engine_keys {
        return Err(failure(
            FailureKind::ProductInvariant,
            "product-settings-written",
        ));
    }
    let engine_sections_kept = [
        "retention_policy",
        "security",
        "pairing",
        "file_sync",
        "network",
        "mobile_sync",
    ]
    .iter()
    .all(|section| saved[*section] == snapshot[*section]);
    let sync_kept = saved["sync"]["auto_sync_enabled"] == snapshot["sync"]["auto_sync"]
        && saved["sync"]["sync_frequency"] == snapshot["sync"]["sync_frequency"]
        && saved["sync"]["content_types"] == snapshot["sync"]["content_types"]
        && saved["sync"]["sync_on_restore"] == true;
    let general_kept = saved["general"]["device_name"] == from.device_name.as_str();
    if !(engine_sections_kept && sync_kept && general_kept) {
        run.fact(
            "engine-settings-kept",
            json!({ "sections": engine_sections_kept, "sync": sync_kept, "general": general_kept }),
        );
        return Err(failure(
            FailureKind::ProductInvariant,
            "engine-settings-not-preserved",
        ));
    }
    Ok(())
}
