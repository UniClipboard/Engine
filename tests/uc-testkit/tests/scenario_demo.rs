use std::{fs, net::TcpListener, time::Duration};

use serde_json::Value;
use tempfile::TempDir;
use uc_testkit::{
    CleanupStatus, FailureKind, Scenario, ScenarioBudget, ScenarioConfig, ScenarioIdentity,
};

const REPRODUCE: &str =
    "cargo test -p uc-testkit --test scenario_demo -- --exact successful_scenario_writes_artifacts";

#[test]
fn fixed_seed_produces_stable_identity_and_reproduction() {
    let first = ScenarioIdentity::new("testkit-identity-demo", 42).expect("valid identity");
    let second = ScenarioIdentity::new("testkit-identity-demo", 42).expect("valid identity");

    assert_eq!(first, second);
    assert_eq!(
        first.artifact_id(),
        "testkit-identity-demo-seed-000000000000002a"
    );
}

#[tokio::test]
async fn successful_scenario_writes_artifacts() {
    let artifact_root = TempDir::new().expect("artifact root");
    let mut scenario = Scenario::start(config(
        artifact_root.path(),
        "testkit-success-demo",
        42,
        Duration::from_secs(1),
    ))
    .expect("scenario starts");

    let temp_path = {
        let lease = scenario.temp_dir("profile").expect("temporary directory");
        let path = lease.path().to_path_buf();
        assert!(path.is_dir());
        drop(lease);
        path
    };
    assert!(!temp_path.exists());

    let address = {
        let lease = scenario.tcp_port("control").expect("port lease");
        let address = lease.local_addr().expect("loopback address");
        assert!(TcpListener::bind(address).is_err());
        drop(lease);
        address
    };
    let rebound = TcpListener::bind(address).expect("released port can be rebound");
    drop(rebound);

    let recorder = scenario.event_recorder();
    let publisher = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(15)).await;
        recorder.record("condition-ready");
    });

    let event = {
        let _stage = scenario.stage("wait-ready");
        scenario
            .wait_for_event("condition-ready", |event| event.kind() == "condition-ready")
            .await
            .expect("condition becomes ready")
    };
    publisher.await.expect("event publisher completes");
    assert_eq!(event.kind(), "condition-ready");

    let completion = scenario.finish(Ok(())).expect("successful completion");
    assert_eq!(completion.report().cleanup, CleanupStatus::Completed);
    assert!(completion.result_json().is_file());
    assert!(completion.summary().is_file());

    let report_text = fs::read_to_string(completion.result_json()).expect("read JSON report");
    let report: Value = serde_json::from_str(&report_text).expect("parse JSON report");
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["scenario"], "testkit-success-demo");
    assert_eq!(report["outcome"], "passed");
    assert_eq!(report["last_event"]["kind"], "condition-ready");
    assert_eq!(report["resources"].as_array().map(Vec::len), Some(2));
    assert!(!report_text.contains(temp_path.to_string_lossy().as_ref()));
    assert!(!report_text.contains(&address.to_string()));
}

#[tokio::test]
async fn timeout_failure_reports_condition_last_event_and_artifacts() {
    let artifact_root = TempDir::new().expect("artifact root");
    let scenario = Scenario::start(config(
        artifact_root.path(),
        "testkit-failure-demo",
        7,
        Duration::from_millis(40),
    ))
    .expect("scenario starts");
    scenario.record_event("demo-started");

    let failure = {
        let _stage = scenario.stage("wait-never-ready");
        scenario
            .wait_for_event("never-ready", |_| false)
            .await
            .expect_err("condition must time out")
    };
    assert_eq!(failure.kind(), FailureKind::DriverProtocol);
    assert_eq!(failure.condition(), Some("never-ready"));
    assert_eq!(
        failure.last_event().map(|event| event.kind()),
        Some("demo-started")
    );

    let completion = scenario
        .finish(Err(failure))
        .expect_err("failed scenario preserves failure result");
    assert!(completion.result_json().is_file());
    assert!(completion.summary().is_file());

    let report_text = fs::read_to_string(completion.result_json()).expect("read JSON report");
    let report: Value = serde_json::from_str(&report_text).expect("parse JSON report");
    assert_eq!(report["outcome"], "failed");
    assert_eq!(report["failure"]["kind"], "driver_protocol");
    assert_eq!(report["failure"]["condition"], "never-ready");
    assert_eq!(report["last_event"]["kind"], "demo-started");
    assert_eq!(report["reproduce"], REPRODUCE);
    assert!(
        report["stages"]
            .as_array()
            .is_some_and(|stages| !stages.is_empty())
    );

    let summary = fs::read_to_string(completion.summary()).expect("read summary");
    assert!(summary.contains("failure: driver_protocol"));
    assert!(summary.contains("condition: never-ready"));
    assert!(summary.contains("last event: demo-started"));
    assert!(summary.contains("artifact:"));
}

fn config(
    artifact_root: &std::path::Path,
    name: &'static str,
    seed: u64,
    budget: Duration,
) -> ScenarioConfig {
    ScenarioConfig::new(
        name,
        seed,
        ScenarioBudget::new(budget),
        REPRODUCE,
        artifact_root,
    )
}
