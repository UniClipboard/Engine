//! 本机身份与成员历史不一致时的公开状态取值（执行计划 2026-09-23-inbound-peer-admission 切片 S5a）。
//!
//! S5a 只公开“需要处理”及其原因，不给恢复动作；恢复方式在现场验证后由 S5b 决定。
//! 宿主只消费稳定线值；新增原因不得改变已有取值。
use uc_engine::{
    SpaceDeviceUpdatePhaseSummary, SpaceDeviceUpdateProblemSummary,
    SpaceDeviceUpdateRecoverySummary, SpaceDeviceUpdateStatusSummary,
};

#[test]
fn identity_mismatch_is_needs_attention_without_a_recovery_action() {
    let status: SpaceDeviceUpdateStatusSummary =
        serde_json::from_str(r#"{"phase":"needs_attention","reason":"local_identity_mismatch"}"#)
            .expect("the new reason must decode");
    assert_eq!(status.phase, SpaceDeviceUpdatePhaseSummary::NeedsAttention);
    assert_eq!(status.recovery, None);
    assert_eq!(status.next_retry_at_ms, None);
    let encoded = serde_json::to_value(status).expect("encode");
    assert_eq!(encoded["reason"], "local_identity_mismatch");
    assert!(
        encoded.get("recovery").is_none(),
        "S5a must not suggest a recovery action to hosts"
    );
}

#[test]
fn no_re_pair_recovery_is_published_before_s5b() {
    // S5b 若决定公开“重新配对”，必须有意修改这条断言并同步产品仓，不能顺手带上。
    assert!(serde_json::from_str::<SpaceDeviceUpdateRecoverySummary>("\"re_pair\"").is_err());
}

#[test]
fn existing_values_are_unchanged() {
    for (value, expected) in [
        (
            "device_state_rejected",
            SpaceDeviceUpdateProblemSummary::DeviceStateRejected,
        ),
        (
            "device_relationship_conflict",
            SpaceDeviceUpdateProblemSummary::DeviceRelationshipConflict,
        ),
        (
            "device_security_update_rejected",
            SpaceDeviceUpdateProblemSummary::DeviceSecurityUpdateRejected,
        ),
        (
            "device_upgrade_required",
            SpaceDeviceUpdateProblemSummary::DeviceUpgradeRequired,
        ),
    ] {
        let decoded: SpaceDeviceUpdateProblemSummary =
            serde_json::from_str(&format!("\"{value}\"")).expect("existing reason");
        assert_eq!(decoded, expected);
    }
    for (value, expected) in [
        (
            "review_devices",
            SpaceDeviceUpdateRecoverySummary::ReviewDevices,
        ),
        ("update_app", SpaceDeviceUpdateRecoverySummary::UpdateApp),
    ] {
        let decoded: SpaceDeviceUpdateRecoverySummary =
            serde_json::from_str(&format!("\"{value}\"")).expect("existing recovery");
        assert_eq!(decoded, expected);
    }
}
