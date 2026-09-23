//! 本机身份自检的运行诊断合同（执行计划 2026-09-23-inbound-peer-admission 切片 S5a）。
//!
//! 只在状态变化时记录：进入不一致记 WARN，恢复一致记 INFO；不携带任何指纹、设备或空间标识。
use uc_observability_contract::diagnostics::connectivity::decode_local_record;

const EVENT: &str = "space.local_identity.changed";

fn payload(state: &str) -> String {
    format!(r#"{{"kind":"local_identity_changed","state":"{state}"}}"#)
}

#[test]
#[ignore = "inbound-peer-admission S5a: remove once space.local_identity.changed is in the contract"]
fn mismatch_is_a_warning_that_needs_attention() {
    let fields = decode_local_record(EVENT, &payload("mismatch"), "WARN").expect("valid record");
    assert_eq!(fields["event.name"], EVENT);
    assert_eq!(fields["state"], "mismatch");
    assert_eq!(fields["uc.outcome"], "needs_attention");
    let mut keys = fields.keys().cloned().collect::<Vec<_>>();
    keys.sort();
    assert_eq!(keys, ["event.name", "state", "uc.outcome"]);
    assert!(decode_local_record(EVENT, &payload("mismatch"), "INFO").is_none());
}

#[test]
#[ignore = "inbound-peer-admission S5a: remove once space.local_identity.changed is in the contract"]
fn returning_to_consistency_is_informational() {
    let fields = decode_local_record(EVENT, &payload("consistent"), "INFO").expect("valid record");
    assert_eq!(fields["state"], "consistent");
    assert_eq!(fields["uc.outcome"], "ok");
    assert!(decode_local_record(EVENT, &payload("consistent"), "WARN").is_none());
}

#[test]
fn identifiers_and_open_values_are_refused() {
    for payload in [
        payload("PRIVATE_STATE"),
        r#"{"kind":"local_identity_changed","state":"mismatch","fingerprint":"PRIVATE_FP"}"#
            .to_owned(),
        r#"{"kind":"local_identity_changed","state":"mismatch","device":"PRIVATE_DEVICE"}"#
            .to_owned(),
    ] {
        for level in ["INFO", "WARN"] {
            assert!(decode_local_record(EVENT, &payload, level).is_none());
        }
    }
}
