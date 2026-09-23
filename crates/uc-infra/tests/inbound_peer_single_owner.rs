//! 入站身份解析与网络准入的唯一负责人结构验收（执行计划 2026-09-23-inbound-peer-admission 切片 S2–S4）。
//!
//! 行为由其他验收测试证明；这里只锁定“不再有第二份实现”，防止之后有人在某个协议里重新手写解析或准入规则。
use std::path::{Path, PathBuf};

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root")
}

fn source(relative: &str) -> String {
    std::fs::read_to_string(workspace().join(relative))
        .unwrap_or_else(|error| panic!("{relative}: {error}"))
}

/// 去掉每个 `#[cfg(test)]` 标注的条目（模块、函数、导入等），只检查生产代码。
fn production(relative: &str) -> String {
    const MARKER: &str = "#[cfg(test)]";
    let text = source(relative);
    let mut kept = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(start) = rest.find(MARKER) {
        kept.push_str(&rest[..start]);
        let item = &rest[start + MARKER.len()..];
        let brace = item.find('{');
        let semicolon = item.find(';');
        let end = match (brace, semicolon) {
            (Some(open), Some(stop)) if stop < open => stop + 1,
            (None, Some(stop)) => stop + 1,
            (Some(open), _) => matching_brace(item, open) + 1,
            (None, None) => item.len(),
        };
        rest = &item[end..];
    }
    kept.push_str(rest);
    kept
}

fn matching_brace(text: &str, open: usize) -> usize {
    let mut depth = 0usize;
    for (index, character) in text[open..].char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return open + index;
                }
            }
            _ => {}
        }
    }
    text.len() - 1
}

const INBOUND_HANDLERS: [&str; 9] = [
    "crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs",
    "crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs",
    "crates/uc-infra/src/network/iroh/active_clipboard/receiver_adapter.rs",
    "crates/uc-infra/src/network/iroh/active_clipboard/pull_serve_adapter.rs",
    "crates/uc-infra/src/network/iroh/transfer_progress_adapter.rs",
    "crates/uc-infra/src/network/iroh/membership_attestation_adapter.rs",
    "crates/uc-infra/src/network/iroh/membership_branch_recovery_adapter.rs",
    "crates/uc-infra/src/network/iroh/membership_history_exchange_adapter.rs",
    "crates/uc-infra/src/network/iroh/node.rs",
];

#[test]
fn identity_resolution_has_a_single_implementation() {
    let gate = production("crates/uc-infra/src/network/iroh/inbound_peer.rs");
    assert!(gate.contains("struct PeerIdentityResolver"));
    assert!(gate.contains("struct InboundPeerGate"));
    for handler in INBOUND_HANDLERS {
        let code = production(handler);
        for forbidden in [
            "fn resolve_device",
            "fn resolve_source_device",
            "fn source_device(",
            "fn fingerprints_equal",
            "member_repo.list()",
            "members.list()",
            ".identity_fingerprint == ",
        ] {
            assert!(
                !code.contains(forbidden),
                "{handler} still contains `{forbidden}`; use inbound_peer instead"
            );
        }
    }
}

#[test]
fn inbound_paths_do_not_log_peer_identifiers() {
    for handler in INBOUND_HANDLERS {
        let code = production(handler);
        // 只收口被统一门取代的拒绝分支；其他既有调试日志的清理不在本计划范围。
        for forbidden in ["remote = %remote", "peer = %device_id.as_str()"] {
            assert!(
                !code.contains(forbidden),
                "{handler} still logs a peer identifier via `{forbidden}`"
            );
        }
    }
}

#[test]
fn network_admission_rule_lives_only_in_the_membership_ledger() {
    assert!(
        !workspace()
            .join("crates/uc-infra/src/space/security/peer_admission.rs")
            .exists(),
        "the Infra copy of the admission rule must be deleted"
    );
    for relative in [
        "crates/uc-infra/src/space/mod.rs",
        "crates/uc-infra/src/space/security/mod.rs",
        "crates/uc-engine/src/assembly/wire/infra.rs",
        "crates/uc-engine/src/assembly/wire/mod.rs",
    ] {
        assert!(
            !source(relative).contains("MlsPeerAdmissionAdapter"),
            "{relative} still references MlsPeerAdmissionAdapter"
        );
    }
    let ledger = production("crates/uc-application/src/space/membership/ledger/peer_admission.rs");
    assert!(ledger.contains("impl PeerAdmissionPort for MembershipLedger"));
}

#[test]
#[ignore = "inbound-peer-admission S4: remove once joiner activation verifies the sponsor identity"]
fn joiner_activation_checks_the_sponsor_identity() {
    let activation = production("crates/uc-infra/src/space/admission/joiner/activation.rs");
    assert!(
        activation.contains("verify_sponsor_route_identity("),
        "prepare() must verify the sponsor facts against its continuation endpoint"
    );
}
