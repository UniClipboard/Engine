//! 固定结果与实际握手协议的导出边界；不接受对端身份或任意文本。
use uc_observability_contract::diagnostics::connectivity::decode_local_record;

#[test]
fn local_read_failure_and_explicit_denial_have_distinct_exported_results() {
    for (result, outcome, reason) in [
        ("authority_unavailable", "error", "read_unavailable"),
        ("authority_denied", "rejected", "not_allowed"),
    ] {
        let payload =
            format!(r#"{{"kind":"presence_check","result":"{result}","duration_ms":20}}"#);
        let fields = decode_local_record("presence.check.completed", &payload, "WARN").unwrap();
        assert_eq!(fields["uc.outcome"], outcome);
        assert_eq!(fields["error.phase"], "authorization");
        assert_eq!(fields["error.reason"], reason);
    }
}

#[test]
fn negotiated_protocol_only_exports_the_closed_known_kinds() {
    for protocol in [
        "peer_reachability_v1",
        "peer_reachability_v2",
        "blob",
        "transfer_progress_v1",
        "transfer_progress_v2",
        "membership_branch_recovery",
        "other",
    ] {
        let payload = format!(
            r#"{{"kind":"physical","record":{{"event":"negotiated","protocol":"{protocol}"}}}}"#
        );
        let fields =
            decode_local_record("connection.protocol.negotiated", &payload, "INFO").unwrap();
        assert_eq!(fields["protocol"], protocol);
        assert_eq!(fields.len(), 2);
    }
    for payload in [
        r#"{"kind":"physical","record":{"event":"negotiated","protocol":"PRIVATE_ALPN"}}"#,
        r#"{"kind":"physical","record":{"event":"negotiated","protocol":"other","device":"PRIVATE_DEVICE"}}"#,
    ] {
        assert!(decode_local_record("connection.protocol.negotiated", payload, "INFO").is_none());
    }
}

#[test]
fn negotiated_progress_path_keeps_the_handshake_classification_without_accepting_addresses() {
    for path in ["direct", "relay", "unknown", "other"] {
        let payload = format!(
            r#"{{"kind":"physical","record":{{"event":"negotiated","protocol":"transfer_progress_v2","initial_path":"{path}"}}}}"#
        );
        let fields =
            decode_local_record("connection.protocol.negotiated", &payload, "INFO").unwrap();
        assert_eq!(fields["initial_path"], path);
        assert_eq!(fields.len(), 3);
    }
    assert!(decode_local_record("connection.protocol.negotiated", r#"{"kind":"physical","record":{"event":"negotiated","protocol":"transfer_progress_v2","initial_path":"PRIVATE_ADDRESS"}}"#, "INFO").is_none());
}
