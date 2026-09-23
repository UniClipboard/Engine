//! 计划 049 S0：用当前真实用例推进出的成员账本状态生成 V4 固定向量。
//!
//! 固定向量是 S2 迁移到新成员记录格式的输入。生成测试每次都运行并断言残留形态；
//! 只有设置 `UC_WRITE_LEGACY_LEDGER_FIXTURES=1` 时才覆盖写入 Infra 固定向量目录。
//! 历史签名使用测试替身，读取时必须配合接受任意签名的验证器。
//! S3 删除旧账本类型时连同本模块与生成测试一起删除，固定向量文件保留。

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use uc_core::ids::DeviceId;
use uc_core::ports::ClockPort;

use crate::space::membership::{
    ActivateMembershipEffectPort, ApplyMembershipMemberFactsPort, ApplyMembershipSecurityPort,
    DeliverRestrictedMembershipUseCase, LoadedMembershipLedger, MembershipEffectExecutionError,
    MembershipLedger, PendingMembershipEffect, RecoverMembershipEffectsUseCase,
    RestrictedMembershipDelivery, RestrictedMembershipDeliveryError,
    RestrictedMembershipDeliveryPort,
};

/// 与 Infra 成员账本 V4 布局一致：格式版本、profile generation、完整账本依次编码。
const MEMBERSHIP_LEDGER_FORMAT_V4: u16 = 4;
const FIXTURE_PROFILE_GENERATION: [u8; 16] = [0x49; 16];
const WRITE_FIXTURES_ENV: &str = "UC_WRITE_LEGACY_LEDGER_FIXTURES";

/// 以接受投递的对端推进一轮受限投递，模拟移除通知或决定已送达。
pub(crate) async fn deliver_restricted_once(ledger: &Arc<MembershipLedger>, now_ms: i64) {
    let report = DeliverRestrictedMembershipUseCase::new(
        Arc::clone(ledger),
        Arc::new(AcceptingDelivery),
        Arc::new(FixedClock(now_ms)),
    )
    .execute()
    .await;
    assert_eq!(
        report.corrupt_count, 0,
        "restricted delivery must not be corrupt"
    );
    assert_eq!(
        report.deferred_count, 0,
        "restricted delivery must complete"
    );
}

/// 以全部成功的能力推进所有待执行成员效果到最终阶段。
pub(crate) async fn settle_membership_effects(ledger: &Arc<MembershipLedger>) {
    let effects = Arc::new(SucceedingEffects);
    let report = RecoverMembershipEffectsUseCase::new(
        Arc::clone(ledger),
        effects.clone(),
        effects.clone(),
        effects,
    )
    .execute()
    .await;
    assert_eq!(
        report.corrupt_count, 0,
        "membership effects must not be corrupt"
    );
    assert_eq!(report.deferred_count, 0, "membership effects must complete");
}

/// 编码为 V4 字节；只有显式设置环境变量时才写入 Infra 固定向量目录。
pub(crate) fn write_v4_fixture(name: &str, ledger: &LoadedMembershipLedger) {
    let bytes = postcard::to_stdvec(&(
        MEMBERSHIP_LEDGER_FORMAT_V4,
        FIXTURE_PROFILE_GENERATION,
        ledger,
    ))
    .expect("encode legacy membership ledger fixture");
    if std::env::var_os(WRITE_FIXTURES_ENV).is_none() {
        return;
    }
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../uc-infra/src/space/membership_ledger/fixtures");
    std::fs::create_dir_all(&directory).expect("create fixture directory");
    std::fs::write(directory.join(format!("{name}.v4.bin")), bytes)
        .expect("write legacy membership ledger fixture");
}

struct FixedClock(i64);

impl ClockPort for FixedClock {
    fn now_ms(&self) -> i64 {
        self.0
    }
}

struct AcceptingDelivery;

#[async_trait]
impl RestrictedMembershipDeliveryPort for AcceptingDelivery {
    async fn deliver_restricted_membership(
        &self,
        _peer: &DeviceId,
        _delivery: &RestrictedMembershipDelivery,
    ) -> Result<(), RestrictedMembershipDeliveryError> {
        Ok(())
    }
}

struct SucceedingEffects;

#[async_trait]
impl ApplyMembershipMemberFactsPort for SucceedingEffects {
    async fn apply_member_facts(
        &self,
        _effect: &PendingMembershipEffect,
    ) -> Result<(), MembershipEffectExecutionError> {
        Ok(())
    }
}

#[async_trait]
impl ApplyMembershipSecurityPort for SucceedingEffects {
    async fn apply_membership_security(
        &self,
        _effect: &PendingMembershipEffect,
    ) -> Result<(), MembershipEffectExecutionError> {
        Ok(())
    }
}

#[async_trait]
impl ActivateMembershipEffectPort for SucceedingEffects {
    async fn activate_membership_effect(
        &self,
        _effect: &PendingMembershipEffect,
    ) -> Result<(), MembershipEffectExecutionError> {
        Ok(())
    }
}
