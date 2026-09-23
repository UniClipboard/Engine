//! 入站对端拒绝诊断的封闭合同验收（执行计划 2026-09-23-inbound-peer-admission 切片 S1）。
//!
//! 只经公开解码入口验证：新事件名、固定级别、协议与原因的封闭取值，以及拒绝任意附加字段。
use serde_json::Value;
use uc_observability_contract::diagnostics::connectivity::decode_local_record;

const EVENT: &str = "peer.inbound.rejected";

const PROTOCOLS: [&str; 8] = [
    "presence",
    "membership_history",
    "membership_attestation",
    "membership_branch_recovery",
    "clipboard",
    "active_clipboard",
    "active_clipboard_pull",
    "transfer_progress",
];

/// 原因与其固定阶段：身份阶段在认出设备前失败，准入阶段在认出设备后失败。
const REASONS: [(&str, &str); 7] = [
    ("identity_unresolved", "identity"),
    ("identity_ambiguous", "identity"),
    ("member_read_failed", "identity"),
    ("fingerprint_unavailable", "identity"),
    ("ledger_denied", "admission"),
    ("ledger_unavailable", "admission"),
    ("not_accepting", "admission"),
];

fn payload(protocol: &str, reason: &str) -> String {
    format!(r#"{{"kind":"inbound_peer_rejected","protocol":"{protocol}","reason":"{reason}"}}"#)
}

#[test]
#[ignore = "inbound-peer-admission S1: remove once peer.inbound.rejected is in the contract"]
fn every_protocol_and_reason_decodes_to_fixed_fields() {
    for protocol in PROTOCOLS {
        for (reason, phase) in REASONS {
            let fields = decode_local_record(EVENT, &payload(protocol, reason), "WARN")
                .unwrap_or_else(|| panic!("{protocol}/{reason} must decode"));
            assert_eq!(fields["event.name"], EVENT);
            assert_eq!(fields["direction"], "inbound");
            assert_eq!(fields["protocol"], protocol);
            assert_eq!(fields["uc.outcome"], "rejected");
            assert_eq!(fields["error.phase"], phase, "{reason}");
            assert_eq!(fields["error.reason"], reason);
            let mut keys = fields.keys().cloned().collect::<Vec<_>>();
            keys.sort();
            assert_eq!(
                keys,
                [
                    "direction",
                    "error.phase",
                    "error.reason",
                    "event.name",
                    "protocol",
                    "uc.outcome",
                ],
                "no field beyond the fixed set may be exported"
            );
        }
    }
}

#[test]
#[ignore = "inbound-peer-admission S1: remove once peer.inbound.rejected is in the contract"]
fn rejection_is_a_warning_and_other_levels_or_names_are_refused() {
    let valid = payload("presence", "identity_unresolved");
    assert!(decode_local_record(EVENT, &valid, "WARN").is_some());
    assert!(decode_local_record(EVENT, &valid, "INFO").is_none());
    assert!(decode_local_record(EVENT, &valid, "ERROR").is_none());
    assert!(decode_local_record("presence.check.completed", &valid, "WARN").is_none());
}

#[test]
fn open_values_and_extra_fields_are_refused() {
    // 这些断言在实现前后都必须成立：未知取值和附加字段永远不能进入导出。
    for payload in [
        payload("presence", "PRIVATE_REASON"),
        payload("PRIVATE_PROTOCOL", "identity_unresolved"),
        r#"{"kind":"inbound_peer_rejected","protocol":"presence","reason":"identity_unresolved","device":"PRIVATE_DEVICE"}"#.to_owned(),
        r#"{"kind":"inbound_peer_rejected","protocol":"presence","reason":"identity_unresolved","fingerprint":"PRIVATE_FP"}"#.to_owned(),
        r#"{"kind":"inbound_peer_rejected","protocol":"presence"}"#.to_owned(),
    ] {
        assert!(
            decode_local_record(EVENT, &payload, "WARN").is_none(),
            "{payload} must be refused"
        );
    }
}

#[test]
#[ignore = "inbound-peer-admission S1: remove once peer.inbound.rejected is in the contract"]
fn exported_fields_never_contain_caller_supplied_text() {
    let fields = decode_local_record(EVENT, &payload("clipboard", "ledger_denied"), "WARN")
        .expect("valid record");
    let text = Value::Object(fields).to_string();
    assert!(!text.contains("PRIVATE"));
}
