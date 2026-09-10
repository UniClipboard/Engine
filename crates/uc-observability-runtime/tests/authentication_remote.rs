use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use prost::Message;
use std::time::Duration;
use uc_observability_contract::diagnostics::connectivity::*;
use uc_observability_runtime::*;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test(flavor = "multi_thread")]
async fn one_authentication_completion_has_local_detail_but_only_the_v1_remote_summary() {
    let receiver = MockServer::start().await;
    for endpoint in ["/v1/traces", "/v1/logs"] {
        Mock::given(method("POST"))
            .and(path(endpoint))
            .respond_with(ResponseTemplate::new(200))
            .mount(&receiver)
            .await;
    }
    let directory = tempfile::tempdir().expect("logs");
    let remote = OtlpHttpConfig::new_loopback(
        &format!("{}/v1/traces", receiver.uri()),
        &format!("{}/v1/logs", receiver.uri()),
    )
    .expect("remote");
    let config = ObservabilityConfig::new(
        ObservabilityResource::new(
            "1.1.0",
            DeploymentEnvironment::Test,
            OperatingSystem::Macos,
            "test",
        )
        .expect("resource"),
    )
    .with_local_logs(LocalLogConfig::new(directory.path()))
    .with_remote(remote);
    let handle = tokio::task::spawn_blocking(move || ProcessObservabilityRuntime::install(config))
        .await
        .expect("worker")
        .expect("install")
        .handle();
    complete_admission_authentication_failure(
        AuthenticationFailure::ContinuationCredential(CredentialFailure::RecordMissing),
        Duration::from_millis(3),
    );
    record_presence_closed(
        ConnectionDirection::Outbound,
        ConnectionCloseReason::RemoteApplicationClosed,
    );
    assert_eq!(
        handle.force_flush(Duration::from_secs(5)).logs,
        SignalResult::Completed
    );
    let requests = receiver.received_requests().await.expect("requests");
    let logs: Vec<_> = requests
        .iter()
        .filter(|r| r.url.path() == "/v1/logs")
        .flat_map(|r| {
            ExportLogsServiceRequest::decode(r.body.as_slice())
                .expect("OTLP")
                .resource_logs
        })
        .flat_map(|r| r.scope_logs)
        .flat_map(|s| s.log_records)
        .collect();
    assert_eq!(
        logs.len(),
        1,
        "pure local diagnostics must never become remote records"
    );
    assert!(logs[0].trace_id.is_empty());
    assert!(logs[0].span_id.is_empty());
    assert!(logs[0].body.is_none());
    assert!(logs[0]
        .attributes
        .iter()
        .all(|a| !["error.phase", "error.reason", "payload"].contains(&a.key.as_str())));
    assert_eq!(
        handle.health().dropped_remote_logs,
        0,
        "local routing is not a privacy rejection"
    );
    let output = std::fs::read_dir(directory.path())
        .expect("files")
        .map(|e| std::fs::read_to_string(e.expect("file").path()).expect("content"))
        .collect::<String>();
    let local: Vec<serde_json::Value> = output
        .lines()
        .map(|l| serde_json::from_str(l).expect("JSON"))
        .collect();
    assert_eq!(local.len(), 2);
    assert_eq!(local[0]["fields"]["error.reason"], "record_missing");
    assert_eq!(local[0]["fields"]["error.type"], "authentication_failed");
    assert_eq!(
        local[1]["fields"]["close.reason"],
        "remote_application_closed"
    );
    handle.shutdown(Duration::from_secs(5));
}
