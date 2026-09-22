use std::path::PathBuf;
use std::time::Duration;

use uc_core::ids::{FormatId, RepresentationId};
use uc_core::{MimeType, ObservedClipboardRepresentation, SystemClipboardSnapshot};
use uc_testkit::{FailureKind, Scenario, ScenarioBudget, ScenarioConfig, ScenarioFailure};

use super::*;

const REPRODUCE: &str = "cargo nextest run --profile ci --locked -p uc-application -E 'test(text_transfer_scenario_encodes_and_dispatches_snapshot)'";

#[tokio::test]
async fn text_transfer_scenario_encodes_and_dispatches_snapshot() {
    let artifact_root = std::env::var_os("UC_TEST_ARTIFACTS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("../../target/test-artifacts/text-transfer"));
    let scenario = Scenario::start(ScenarioConfig::new(
        "text-transfer-dispatch",
        0x0040_3404,
        ScenarioBudget::new(Duration::from_secs(1)),
        REPRODUCE,
        artifact_root,
    ))
    .expect("text transfer scenario starts");

    let result = async {
        let mut repo = MockPeerAddrRepo::new();
        repo.expect_list()
            .times(1)
            .returning(|| Ok(vec![record("peer-a")]));

        let mut cipher = MockCipher::new();
        cipher
            .expect_encrypt()
            .times(1)
            .withf(|plaintext| plaintext.len() > 10 && plaintext[8..10] == [0x01, 0x00])
            .returning(|plaintext| Ok(plaintext.to_vec()));

        let mut dispatch = MockDispatch::new();
        dispatch
            .expect_dispatch()
            .with(eq(DeviceId::new("peer-a")), always(), always())
            .times(1)
            .withf(|_target, header, _payload| header.payload_version == 3)
            .returning(|_, _, _| dispatch_report(Ok(DispatchAck::Accepted)));

        let facade = build_facade(
            repo,
            make_peer_reachability_unknown(),
            cipher,
            dispatch,
            make_device_identity("self"),
            make_local_identity(),
            make_settings(),
        );
        let snapshot = SystemClipboardSnapshot {
            ts_ms: 7,
            representations: vec![ObservedClipboardRepresentation::new(
                RepresentationId::new(),
                FormatId::from("text"),
                Some(MimeType("text/plain".to_string())),
                b"fast text transfer".to_vec(),
            )],
            file_content_digests: Vec::new(),
            file_set_v1_component: None,
        };

        let outcome = {
            let _stage = scenario.stage("encode-and-dispatch-text");
            facade
                .dispatch_snapshot(
                    snapshot,
                    uc_core::ClipboardChangeOrigin::LocalCapture,
                    None,
                    None,
                )
                .await
                .map_err(|_| fixture_failure("text-dispatch"))?
        };
        scenario.record_event("text-dispatch-accepted");
        if outcome.total_accepted != 1 {
            return Err(product_failure("text-target-was-not-accepted"));
        }
        if !outcome.snapshot_hash.starts_with("blake3v1:") {
            return Err(product_failure("canonical-snapshot-hash-missing"));
        }
        Ok(())
    }
    .await;

    scenario
        .finish(result)
        .expect("text transfer scenario report succeeds");
}

fn fixture_failure(condition: &'static str) -> ScenarioFailure {
    ScenarioFailure::new(FailureKind::FixtureInvalid, condition)
}

fn product_failure(condition: &'static str) -> ScenarioFailure {
    ScenarioFailure::new(FailureKind::ProductInvariant, condition)
}
