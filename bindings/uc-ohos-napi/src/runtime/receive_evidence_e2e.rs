//! 使用真实 Engine 工件验证 OHOS 的逐项关联、空值和十进制整数交付。
use super::receive_progress;
use serde_json::Value;
use uc_engine::{ReceiveContentLocator, ReceiveProgressSummary};

#[test]
#[ignore = "requires UC_RECEIVE_PROGRESS_PRODUCER and UC_RECEIVE_PROGRESS_EVIDENCE"]
fn real_receive_evidence_through_ohos_projection() {
    let producer =
        std::env::var_os("UC_RECEIVE_PROGRESS_PRODUCER").expect("Engine E2E executable required");
    let path = std::path::PathBuf::from(
        std::env::var_os("UC_RECEIVE_PROGRESS_EVIDENCE").expect("E2E artifact path required"),
    )
    .with_extension("ohos.json");
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
                engine.completed_bytes.to_string(),
                engine.total_bytes.to_string(),
                engine.items_total,
                engine.items_completed
            )
        );
        assert_eq!(
            binding.snapshot_version.as_ref().map(|version| (
                version.producer_id.clone(),
                version.attempt_revision.clone(),
                version.snapshot_revision.clone()
            )),
            engine.snapshot_version.as_ref().map(|version| (
                version.producer_id.clone(),
                version.attempt_revision.to_string(),
                version.snapshot_revision.to_string()
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
                    value.received_bytes.clone(),
                    value.total_bytes.clone(),
                    value.round
                )),
                expected.transfer.map(|value| (
                    value.received_bytes.to_string(),
                    value.total_bytes.map(|total| total.to_string()),
                    value.round
                ))
            );
            let expected = expected.content.map(|locator| match locator {
                ReceiveContentLocator::File { index } => ("file", Some(index)),
                ReceiveContentLocator::Representation { index } => ("representation", Some(index)),
                ReceiveContentLocator::DirectoryUnit => ("directory_unit", None),
            });
            assert_eq!(
                actual
                    .content
                    .as_ref()
                    .map(|content| (content.kind.as_str(), content.index)),
                expected
            );
        }
    }
}
