//! 普通采集实际落盘：步骤与完成记录关联，具体失败不进入远程自由字段。
use std::time::Duration;
use uc_observability_contract::diagnostics::connectivity::{
    record_admission_network_snapshot, AdmissionExchangeFailure, AdmissionExchangeObservation,
    AdmissionExchangeSide, AdmissionExchangeStep, AdmissionNetworkPoint, AdmissionNetworkSnapshot,
    NetworkPathKind,
};
use uc_observability_contract::diagnostics::{
    operation_span, DiagnosticDomain, DiagnosticErrorType, DiagnosticOperation, DiagnosticRole,
    DiagnosticSpanKind, OperationCompletion, OperationContext,
};
use uc_observability_runtime::{
    DeploymentEnvironment, DetailedCaptureRequest, LocalLogConfig, ObservabilityConfig,
    ObservabilityResource, OperatingSystem, ProcessObservabilityRuntime, SignalResult,
};

#[test]
fn standard_capture_preserves_exchange_steps_failure_and_interruption() {
    let directory = tempfile::tempdir().expect("logs");
    let handle = ProcessObservabilityRuntime::install(
        ObservabilityConfig::new(
            ObservabilityResource::new(
                "1.1.0",
                DeploymentEnvironment::Test,
                OperatingSystem::Macos,
                "test",
            )
            .expect("resource"),
        )
        .with_local_logs(LocalLogConfig::new(directory.path())),
    )
    .expect("runtime")
    .handle();
    for failure in [
        AdmissionExchangeFailure::TimedOut,
        AdmissionExchangeFailure::ConnectionClosed,
        AdmissionExchangeFailure::IoFailed,
        AdmissionExchangeFailure::InvalidMessage,
        AdmissionExchangeFailure::AuthenticationRejected,
    ] {
        let span = operation_span(OperationContext {
            domain: DiagnosticDomain::SpaceAdmission,
            operation: DiagnosticOperation::NetworkTransport,
            role: DiagnosticRole::Joiner,
            kind: DiagnosticSpanKind::Client,
        });
        span.in_scope(|| {
            let mut observation =
                AdmissionExchangeObservation::begin(AdmissionExchangeSide::Joiner);
            observation.start_step(AdmissionExchangeStep::SendRequest);
            observation.start_step(AdmissionExchangeStep::ReceiveReply);
            observation.fail(failure);
            observation.fail(AdmissionExchangeFailure::InvalidMessage);
            observation.finish(OperationCompletion::failed(
                DiagnosticDomain::SpaceAdmission,
                DiagnosticOperation::NetworkTransport,
                DiagnosticRole::Joiner,
                DiagnosticErrorType::DecodeFailed,
                Duration::from_millis(1),
            ));
        });
    }
    let span = operation_span(OperationContext {
        domain: DiagnosticDomain::SpaceAdmission,
        operation: DiagnosticOperation::NetworkTransport,
        role: DiagnosticRole::Sponsor,
        kind: DiagnosticSpanKind::Server,
    });
    span.in_scope(|| {
        let mut observation = AdmissionExchangeObservation::begin(AdmissionExchangeSide::Sponsor);
        observation.start_step(AdmissionExchangeStep::ReceiveAcknowledgement);
    });
    drop(span);
    let snapshot = || {
        record_admission_network_snapshot(
            AdmissionExchangeSide::Joiner,
            AdmissionNetworkPoint::ExchangeStarted,
            AdmissionNetworkSnapshot {
                path: NetworkPathKind::Unknown,
                rtt_us: None,
                lost_packets_total: 0,
                sent_datagrams_total: 0,
                received_datagrams_total: 0,
            },
        )
    };
    snapshot();
    handle
        .start_local_diagnostic_capture(DetailedCaptureRequest::default())
        .expect("capture");
    snapshot();
    assert_eq!(
        ProcessObservabilityRuntime::flush_local_logs(Duration::from_secs(5)),
        SignalResult::Completed
    );
    let records: Vec<serde_json::Value> = std::fs::read_dir(directory.path())
        .expect("files")
        .flat_map(|entry| {
            std::fs::read_to_string(entry.expect("entry").path())
                .expect("file")
                .lines()
                .map(|line| serde_json::from_str(line).expect("record"))
                .collect::<Vec<_>>()
        })
        .collect();
    let completions: Vec<_> = records
        .iter()
        .filter(|r| r["fields"]["event.name"] == "uc.operation.completed")
        .collect();
    assert_eq!(completions.len(), 6);
    let network: Vec<_> = records
        .iter()
        .filter(|record| record["fields"]["event.name"] == "pairing.exchange.network.snapshot")
        .collect();
    assert_eq!(network.len(), 1, "网络快照只在详细模式落盘");
    assert_eq!(network[0]["capture_mode"], "detailed");
    assert_eq!(network[0]["fields"]["rtt_measurement"], "unavailable");
    assert!(network[0]["fields"]["rtt_us"].is_null());
    assert_eq!(
        network[0]["fields"]["retransmission_measurement"],
        "unsupported"
    );
    for (record, expected) in completions.iter().zip([
        "timeout",
        "channel_closed",
        "stream_failed",
        "decode_failed",
        "authentication_failed",
        "",
    ]) {
        assert_eq!(record["capture_mode"], "standard");
        assert!(record["trace_id"].as_str().is_some());
        assert!(record["span_id"].as_str().is_some());
        let steps: Vec<_> = records
            .iter()
            .filter(|r| {
                r["span_id"] == record["span_id"]
                    && r["fields"]["event.name"] == "pairing.exchange.step.finished"
            })
            .collect();
        let last = steps.last().expect("step completion");
        assert_eq!(last["trace_id"], record["trace_id"]);
        if expected.is_empty() {
            assert_eq!(record["fields"]["uc.outcome"], "cancelled");
            assert_eq!(last["fields"]["error.reason"], "interrupted");
        } else {
            assert_eq!(record["fields"]["error.type"], expected);
            assert_eq!(record["fields"]["error.phase"], "receive_reply");
            assert_eq!(
                record["fields"]["error.reason"],
                last["fields"]["error.reason"]
            );
            assert_eq!(steps[0]["fields"]["uc.outcome"], "ok");
        }
    }
    handle.shutdown(Duration::from_secs(5));
}
