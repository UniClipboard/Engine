//! D1 旧版快照升级：导入早于 Engine 的 Desktop 资料快照，由当前源码打开、核对、重启并继续使用。
//!
//! 可选的提交中断：首次启动期间外部持有 Space 转换激活租约，使单设备重建在提交边界失败，
//! 留下与竞争失败相同的持久状态；释放后重启必须收敛，且后续重启保持不变。

use std::fs::{File, OpenOptions};

use serde_json::{Value, json};
use uc_testkit::{FailureKind, ScenarioFailure};

use crate::{
    catalog::{Interruption, LegacyAnchor, Point},
    cell::CellRun,
    device::{Device, Launch, failure, history_contains},
    dimensions::{require_preserved, upgrade::continue_using},
    fixture::{Content, content_digest, install_legacy_profile},
};

/// Engine 在应用数据根目录下用于串行化 Space 转换激活的租约文件。只有本单元依赖这个内部名称；
/// 名称变化会使首次启动意外成功，单元以 `interruption-not-effective` 显式失败。
const ACTIVATION_LEASE: &str = ".space-transition-activation.lease";

pub(crate) async fn run(
    run: &mut CellRun,
    from: &LegacyAnchor,
    to: &Point,
    interruption: Option<Interruption>,
) -> Result<(), ScenarioFailure> {
    let mut device = Device::new(run, "a", "Device A")?;
    {
        let _stage = run.stage("import-legacy");
        let storage = install_legacy_profile(from, device.root())?;
        device.preload_secure_storage(storage)?;
    }
    let interrupted = match interruption {
        Some(Interruption::CommitInterrupted) => {
            Some(interrupt_first_upgrade(run, &mut device, to).await?)
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
