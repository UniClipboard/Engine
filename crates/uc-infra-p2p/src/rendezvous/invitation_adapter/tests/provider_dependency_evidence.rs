use std::path::PathBuf;

use serde_json::json;
use uc_core::ports::PairingInvitationPort;
use uc_testkit::{
    CleanupStatus, FailureKind, Scenario, ScenarioBudget, ScenarioConfig, ScenarioFailure,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::{loopback_endpoint, make_adapter, InMemorySettings};
use crate::rendezvous::invitation_adapter::{InvitationError, LOCAL_MINT_TTL};

const REPRODUCE: &str =
    "cargo nextest run --profile ci --locked -p uc-infra -E 'test(provider_dependency_evidence)'";

fn scenario(name: &'static str, seed: u64) -> Scenario {
    let artifact_root = PathBuf::from("../../target/test-artifacts/real-dependencies");
    Scenario::start(ScenarioConfig::new(
        name,
        seed,
        ScenarioBudget::new(std::time::Duration::from_secs(3)),
        REPRODUCE,
        artifact_root,
    ))
    .expect("provider diagnostic scenario starts")
}

#[tokio::test]
async fn provider_dependency_evidence_reports_all_outcomes() {
    let mut success = scenario("provider-success", 0x0040_0301);
    let endpoint = loopback_endpoint().await;
    let server = MockServer::start().await;
    let expires_at =
        (chrono::Utc::now() + LOCAL_MINT_TTL + chrono::Duration::minutes(1)).timestamp_millis();
    Mock::given(method("POST"))
        .and(path("/v1/pairings"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": "ABCD-EFGH",
            "expiresAtMs": expires_at,
        })))
        .mount(&server)
        .await;
    let adapter = make_adapter(
        endpoint.clone(),
        InMemorySettings::with_device_name(Some("provider-evidence")),
        server.uri(),
    );
    let issued = {
        let _stage = success.stage("provider-request");
        adapter.issue_invitation().await.expect("provider success")
    };
    assert_eq!(issued.code.as_str(), "ABCD-EFGH");
    success.record_event("provider-response-accepted");
    drop(adapter);
    endpoint.close().await;
    drop(server);
    success.record_external_resource("loopback-endpoint", "provider", CleanupStatus::Completed);
    success.record_external_resource("http-server", "directory", CleanupStatus::Completed);
    success.finish(Ok(())).expect("success report");

    let mut rejected = scenario("provider-invalid-response", 0x0040_0302);
    let endpoint = loopback_endpoint().await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/pairings"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not-json"))
        .mount(&server)
        .await;
    let adapter = make_adapter(
        endpoint.clone(),
        InMemorySettings::with_device_name(Some("provider-evidence")),
        server.uri(),
    );
    let error = adapter
        .issue_invitation()
        .await
        .expect_err("invalid response must be rejected");
    assert!(matches!(
        error,
        InvitationError::DirectoryInvalidResponse { .. }
    ));
    rejected.record_event("provider-response-invalid");
    drop(adapter);
    endpoint.close().await;
    drop(server);
    rejected.record_external_resource("loopback-endpoint", "provider", CleanupStatus::Completed);
    rejected.record_external_resource("http-server", "directory", CleanupStatus::Completed);
    let report = rejected
        .finish(Err(ScenarioFailure::new(
            FailureKind::ProductInvariant,
            "directory-invalid-response",
        )))
        .expect_err("controlled product failure report");
    assert_eq!(report.failure().kind(), FailureKind::ProductInvariant);

    let mut unavailable = scenario("provider-unavailable", 0x0040_0303);
    let endpoint = loopback_endpoint().await;
    let port = unavailable.tcp_port("directory-port").expect("port lease");
    let address = port.local_addr().expect("leased address");
    drop(port);
    let adapter = make_adapter(
        endpoint.clone(),
        InMemorySettings::with_device_name(Some("provider-evidence")),
        format!("http://{address}"),
    );
    let issued = adapter
        .issue_invitation()
        .await
        .expect("transport failure falls back to local mint");
    assert_eq!(issued.code.as_str().len(), 7);
    unavailable.record_event("provider-transport-unavailable");
    drop(adapter);
    endpoint.close().await;
    unavailable.record_external_resource("loopback-endpoint", "provider", CleanupStatus::Completed);
    let report = unavailable
        .finish(Err(ScenarioFailure::new(
            FailureKind::EnvironmentUnavailable,
            "directory-transport-unavailable",
        )))
        .expect_err("controlled environment failure report");
    assert_eq!(report.failure().kind(), FailureKind::EnvironmentUnavailable);

    let mut cleanup = scenario("provider-cleanup-failure", 0x0040_0305);
    cleanup.record_event("cleanup-result-received");
    cleanup.record_external_resource("http-server", "directory", CleanupStatus::Failed);
    let report = cleanup
        .finish(Ok(()))
        .expect_err("controlled cleanup failure report");
    assert_eq!(report.failure().kind(), FailureKind::CleanupFailed);
    assert_eq!(report.report().cleanup, CleanupStatus::Failed);
}
