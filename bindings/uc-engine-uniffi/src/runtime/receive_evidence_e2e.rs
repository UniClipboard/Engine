//! 从真实 Engine 网络场景的工件继续验证绑定交付，不构造另一套接收事实。
use super::{receive_progress, ReceiveContentLocator};
use serde_json::Value;
use uc_engine::{ReceiveContentLocator as EngineLocator, ReceiveProgressSummary};

#[test]
#[ignore = "requires UC_RECEIVE_PROGRESS_PRODUCER and UC_RECEIVE_PROGRESS_EVIDENCE"]
fn real_receive_evidence_through_uniffi_projection() {
    let producer =
        std::env::var_os("UC_RECEIVE_PROGRESS_PRODUCER").expect("Engine E2E executable required");
    let path = std::path::PathBuf::from(
        std::env::var_os("UC_RECEIVE_PROGRESS_EVIDENCE").expect("E2E artifact path required"),
    )
    .with_extension("uniffi.json");
    let result = std::process::Command::new(producer)
        .args(["testing::host_adapter_contract::receive_retry::item_progress::real_engine_file_addressable_query_survives_reconnect_and_selective_retry", "--exact", "--nocapture"])
        .env("UC_RECEIVE_PROGRESS_EVIDENCE", &path)
        .status().unwrap();
    assert!(
        result.success(),
        "real Engine transfer must finish before binding validation"
    );
    let evidence: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(evidence["sha256"].as_array().unwrap().len(), 3);
    let base = std::path::PathBuf::from(std::env::var_os("UC_RECEIVE_PROGRESS_EVIDENCE").unwrap());
    let mut checkpoints = ["first", "failed", "retry", "completed", "restarted"]
        .iter()
        .map(|name| evidence[*name].clone())
        .collect::<Vec<_>>();
    let mixed: Value =
        serde_json::from_slice(&std::fs::read(base.with_extension("mixed.json")).unwrap()).unwrap();
    checkpoints.extend([mixed["receiving"].clone(), mixed["completed"].clone()]);
    for name in ["directory", "zero"] {
        checkpoints.push(
            serde_json::from_slice(
                &std::fs::read(base.with_extension(format!("{name}.json"))).unwrap(),
            )
            .unwrap(),
        );
    }
    for checkpoint in checkpoints {
        let engine: ReceiveProgressSummary = serde_json::from_value(checkpoint).unwrap();
        let binding = receive_progress(engine.clone());
        assert_eq!(
            (&binding.entry_id, &binding.attempt_id, &binding.state),
            (&engine.entry_id, &engine.attempt_id, &engine.state)
        );
        assert_eq!(
            (
                binding.completed_bytes,
                binding.total_bytes,
                binding.items_total,
                binding.items_completed
            ),
            (
                engine.completed_bytes,
                engine.total_bytes,
                engine.items_total,
                engine.items_completed
            )
        );
        assert_eq!(
            binding.snapshot_version.as_ref().map(|version| (
                &version.producer_id,
                version.attempt_revision,
                version.snapshot_revision
            )),
            engine.snapshot_version.as_ref().map(|version| (
                &version.producer_id,
                version.attempt_revision,
                version.snapshot_revision
            ))
        );
        assert_eq!(binding.items.len(), engine.items.len());
        for (actual, expected) in binding.items.iter().zip(&engine.items) {
            assert_eq!(
                (&actual.item_id, &actual.state, &actual.kind),
                (
                    &expected.item_id,
                    &expected.state,
                    &expected.kind.to_string()
                )
            );
            assert_eq!(actual.history_file_index, expected.history_file_index);
            assert_eq!(
                actual.failure_reason,
                expected.failure_reason.map(|reason| reason.to_string())
            );
            assert_eq!(
                actual.transfer.as_ref().map(|value| (
                    value.received_bytes,
                    value.total_bytes,
                    value.round
                )),
                expected.transfer.map(|value| (
                    value.received_bytes,
                    value.total_bytes,
                    value.round
                ))
            );
            match (&actual.content, expected.content) {
                (
                    Some(ReceiveContentLocator::File { index: actual }),
                    Some(EngineLocator::File { index: expected }),
                )
                | (
                    Some(ReceiveContentLocator::Representation { index: actual }),
                    Some(EngineLocator::Representation { index: expected }),
                ) => assert_eq!(*actual, expected),
                (
                    Some(ReceiveContentLocator::DirectoryUnit),
                    Some(EngineLocator::DirectoryUnit),
                )
                | (None, None) => {}
                other => panic!("binding changed content association: {other:?}"),
            }
        }
    }
}
