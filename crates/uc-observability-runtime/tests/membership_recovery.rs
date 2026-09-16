use opentelemetry_proto::tonic::collector::{
    logs::v1::ExportLogsServiceRequest, trace::v1::ExportTraceServiceRequest,
};
use opentelemetry_proto::tonic::common::v1::{any_value::Value, KeyValue};
use prost::Message;
use std::time::Duration;
use uc_observability_contract::diagnostics::*;
use uc_observability_runtime::*;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn field<'a>(fields: &'a [KeyValue], key: &str) -> Option<&'a str> {
    fields
        .iter()
        .find(|field| field.key == key)
        .and_then(|field| match field.value.as_ref()?.value.as_ref()? {
            Value::StringValue(value) => Some(value.as_str()),
            _ => None,
        })
}

#[tokio::test(flavor = "multi_thread")]
async fn recovery_reports_trigger_result_and_omits_empty_checks() {
    let receiver = MockServer::start().await;
    for endpoint in ["/v1/traces", "/v1/logs"] {
        Mock::given(method("POST"))
            .and(path(endpoint))
            .respond_with(ResponseTemplate::new(200))
            .mount(&receiver)
            .await;
    }
    let remote = OtlpHttpConfig::new_loopback(
        &format!("{}/v1/traces", receiver.uri()),
        &format!("{}/v1/logs", receiver.uri()),
    )
    .expect("endpoints");
    let config = ObservabilityConfig::new(
        ObservabilityResource::new(
            "1.1.0",
            DeploymentEnvironment::Test,
            OperatingSystem::Macos,
            "test",
        )
        .expect("resource"),
    )
    .with_remote(remote);
    let handle = tokio::task::spawn_blocking(move || ProcessObservabilityRuntime::install(config))
        .await
        .expect("worker")
        .expect("install")
        .handle();
    for (trigger, outcome, conflict) in [
        (
            MembershipRecoveryTrigger::Startup,
            MembershipRecoveryOutcome::Completed,
            false,
        ),
        (
            MembershipRecoveryTrigger::Resume,
            MembershipRecoveryOutcome::Deferred,
            false,
        ),
        (
            MembershipRecoveryTrigger::PeerOnline,
            MembershipRecoveryOutcome::Partial,
            false,
        ),
        (
            MembershipRecoveryTrigger::Retry,
            MembershipRecoveryOutcome::Failed,
            true,
        ),
        (
            MembershipRecoveryTrigger::StateChanged,
            MembershipRecoveryOutcome::Corrupt,
            false,
        ),
        (
            MembershipRecoveryTrigger::Requested,
            MembershipRecoveryOutcome::Failed,
            false,
        ),
    ] {
        scope_membership_recovery_trigger(trigger, async {
            let observation = MembershipRecoveryObservation::begin();
            let result = observation
                .scope(async {
                    if conflict {
                        describe_membership_conflict();
                    }
                    7
                })
                .await;
            assert_eq!(result, 7);
            observation.finish(outcome);
        })
        .await;
    }
    let empty = MembershipRecoveryObservation::begin();
    empty.finish(MembershipRecoveryOutcome::NoWork);
    drop(empty);
    let cancelled = MembershipRecoveryObservation::begin();
    drop(cancelled);
    let flush = handle.force_flush(Duration::from_secs(10));
    assert_eq!(flush.traces, SignalResult::Completed);
    assert_eq!(flush.logs, SignalResult::Completed);
    let requests = receiver.received_requests().await.expect("requests");
    let spans = requests
        .iter()
        .filter(|r| r.url.path() == "/v1/traces")
        .flat_map(|r| {
            ExportTraceServiceRequest::decode(r.body.as_slice())
                .expect("traces")
                .resource_spans
        })
        .flat_map(|r| r.scope_spans)
        .flat_map(|s| s.spans)
        .collect::<Vec<_>>();
    let logs = requests
        .iter()
        .filter(|r| r.url.path() == "/v1/logs")
        .flat_map(|r| {
            ExportLogsServiceRequest::decode(r.body.as_slice())
                .expect("logs")
                .resource_logs
        })
        .flat_map(|r| r.scope_logs)
        .flat_map(|s| s.log_records)
        .collect::<Vec<_>>();
    assert_eq!(spans.len(), 7, "empty check must not appear");
    assert_eq!(logs.len(), 7, "no log may refer to the omitted empty check");
    for (name, expected) in [
        ("startup", "ok"),
        ("resume", "deferred"),
        ("peer_online", "partial"),
        ("retry", "conflict"),
        ("state_changed", "error"),
    ] {
        let span = spans
            .iter()
            .find(|span| span.name == format!("membership.recover.{name}"))
            .expect("trigger");
        assert_eq!(field(&span.attributes, "uc.outcome"), Some(expected));
    }
    for span in spans {
        let log = logs
            .iter()
            .find(|log| log.trace_id == span.trace_id && log.span_id == span.span_id)
            .expect("matching log");
        assert_eq!(
            field(&span.attributes, "uc.outcome"),
            field(&log.attributes, "uc.outcome")
        );
    }
}
