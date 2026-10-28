//! 独立子进程验证真实完整操作的OTLP输出；接收端是原始记录器，不冒充Collector。
use super::super::{member_id, progress, receive_case, ReleaseOnDrop, ENGINE_TEST_LOCK};
use crate::{
    flush_test_tracing, init_test_tracing_with_otlp, EngineErrorCategory, EntryReceiveRetryOutcome,
    Operation, OperationResult, OutboundProgressInput, OutboundProgressReconciliation,
    ReconcileOutboundProgressInput,
};
use opentelemetry_proto::tonic::collector::{
    logs::v1::ExportLogsServiceRequest, trace::v1::ExportTraceServiceRequest,
};
use opentelemetry_proto::tonic::common::v1::{any_value::Value, KeyValue};
use opentelemetry_proto::tonic::logs::v1::LogRecord;
use opentelemetry_proto::tonic::trace::v1::Span;
use prost::Message;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::process::Command;
use tokio::time::{sleep, timeout};
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const CHILD: &str = "UC_ENGINE_PROGRESS_OBSERVATION_CHILD";
const TEST: &str = "testing::host_adapter_contract::receive_retry::observation::progress::real_engine_reconciliation_exports_its_result_without_extending_receive_or_retry";
const OPERATION: &str = "clipboard.reconcile_progress";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_reconciliation_exports_its_result_without_extending_receive_or_retry() {
    if std::env::var(CHILD).as_deref() != Ok("1") {
        let status = timeout(
            Duration::from_secs(80),
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", TEST, "--nocapture"])
                .env(CHILD, "1")
                .kill_on_drop(true)
                .status(),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(status.success(), "isolated Engine observation must pass");
        return;
    }
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let endpoint = MockServer::start().await;
    for signal in ["/v1/traces", "/v1/logs"] {
        Mock::given(method("POST"))
            .and(path(signal))
            .respond_with(ResponseTemplate::new(200))
            .mount(&endpoint)
            .await;
    }
    assert!(init_test_tracing_with_otlp(
        &format!("{}/v1/traces", endpoint.uri()),
        &format!("{}/v1/logs", endpoint.uri()),
    ));
    let case = receive_case(false).await;
    let _release = ReleaseOnDrop(case.exports.clone());
    let peer = member_id(&case.receiver).await;
    let OperationResult::OutboundProgressList(views) = case
        .source
        .execute(Operation::ListOutboundProgress)
        .await
        .unwrap()
    else {
        panic!("original complete source list")
    };
    let view = views
        .into_iter()
        .find(|view| view.attempt_id.as_deref() == Some(case.first.attempt_id.as_str()))
        .unwrap();
    let input = OutboundProgressInput {
        entry_id: view.entry_id,
        target_device_id: peer,
    };
    let query = ReconcileOutboundProgressInput {
        entry_id: input.entry_id.clone(),
        target_device_id: input.target_device_id.clone(),
        expected_attempt_id: None,
    };
    // 完整事实核对成功不表示文件全部成功；本轮实际仍有一项失败。
    assert_eq!(
        case.source
            .execute(Operation::ReconcileOutboundProgress(query.clone()))
            .await
            .unwrap(),
        OperationResult::OutboundProgressReconciliation(OutboundProgressReconciliation::Settled)
    );
    assert_eq!(case.first.items_completed, 2);
    assert_eq!(case.exports.exports.lock().unwrap().len(), 3);
    case.exports.hold_terminals.store(true, Ordering::SeqCst);
    let OperationResult::EntryReceiveRetry(EntryReceiveRetryOutcome::Accepted { attempt_id }) =
        case.receiver
            .execute(Operation::RetryEntryReceive(case.retry_input()))
            .await
            .unwrap()
    else {
        panic!("original selected retry")
    };
    timeout(Duration::from_secs(20), case.exports.paused.notified())
        .await
        .unwrap();
    assert_eq!(
        case.source
            .execute(Operation::ReconcileOutboundProgress(query.clone()))
            .await
            .unwrap(),
        OperationResult::OutboundProgressReconciliation(OutboundProgressReconciliation::Running)
    );
    case.exports.release.add_permits(1);
    let completed = progress(&case.receiver, &case.first.entry_id, "completed").await;
    assert_eq!(completed.attempt_id, attempt_id);
    timeout(
        Duration::from_secs(20),
        case.exports.terminal_held.notified(),
    )
    .await
    .unwrap();
    assert_eq!(
        case.source
            .execute(Operation::ReconcileOutboundProgress(query))
            .await
            .unwrap(),
        OperationResult::OutboundProgressReconciliation(OutboundProgressReconciliation::Settled)
    );
    assert_eq!(
        case.source
            .execute(Operation::ReconcileOutboundProgress(
                ReconcileOutboundProgressInput {
                    entry_id: input.entry_id.clone(),
                    target_device_id: input.target_device_id.clone(),
                    expected_attempt_id: Some(case.first.attempt_id.clone()),
                }
            ))
            .await
            .unwrap(),
        OperationResult::OutboundProgressReconciliation(OutboundProgressReconciliation::Superseded)
    );
    assert_eq!(
        case.source
            .execute(Operation::ReconcileOutboundProgress(
                ReconcileOutboundProgressInput {
                    entry_id: Uuid::new_v4().to_string(),
                    target_device_id: input.target_device_id.clone(),
                    expected_attempt_id: None,
                }
            ))
            .await
            .unwrap(),
        OperationResult::OutboundProgressReconciliation(
            OutboundProgressReconciliation::NotRetained
        )
    );
    assert_eq!(
        case.source
            .execute(Operation::ReconcileOutboundProgress(
                ReconcileOutboundProgressInput {
                    entry_id: "invalid-source-entry".into(),
                    target_device_id: input.target_device_id.clone(),
                    expected_attempt_id: None,
                }
            ))
            .await
            .unwrap_err()
            .category(),
        EngineErrorCategory::InvalidInput
    );
    assert_eq!(
        case.source
            .execute(Operation::ReconcileOutboundProgress(
                ReconcileOutboundProgressInput {
                    entry_id: input.entry_id.clone(),
                    target_device_id: "not-a-current-member".into(),
                    expected_attempt_id: None,
                }
            ))
            .await
            .unwrap_err()
            .category(),
        EngineErrorCategory::Unauthorized
    );
    for _ in 0..3 {
        case.source
            .execute(Operation::QueryOutboundProgress(input.clone()))
            .await
            .unwrap();
        case.source
            .execute(Operation::ListOutboundProgress)
            .await
            .unwrap();
    }
    assert_eq!(case.exports.exports.lock().unwrap().len(), 4);
    for (bytes, path) in &case.saved {
        assert_eq!(&std::fs::read(path).unwrap(), bytes);
    }
    // 接收器可能收到与OTLP无关的根路径探测，不能把它当成业务记录。
    let mut probe = TcpStream::connect(endpoint.address()).await.unwrap();
    probe
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut response = Vec::new();
    probe.read_to_end(&mut response).await.unwrap();
    assert!(response.starts_with(b"HTTP/1.1 404"));
    // 原通知仍被持有时，完整接收与重试必须已经结束并实际导出。
    let (spans, logs) = timeout(Duration::from_secs(10), async {
        loop {
            flush_test_tracing();
            let records = records(&endpoint).await;
            if records.0.iter().any(|span| {
                field(&span.attributes, "uc.operation") == Some("clipboard.retry_receive")
            }) {
                break records;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let reconciliations = spans
        .iter()
        .filter(|span| field(&span.attributes, "uc.operation") == Some(OPERATION))
        .collect::<Vec<_>>();
    assert_eq!(
        reconciliations.len(),
        7,
        "only seven explicit reconciliations create business records"
    );
    let mut names = reconciliations
        .iter()
        .map(|span| span.name.as_str())
        .collect::<Vec<_>>();
    names.sort();
    assert_eq!(
        names,
        vec![
            "clipboard.reconcile_progress.access_denied",
            "clipboard.reconcile_progress.invalid_input",
            "clipboard.reconcile_progress.not_retained",
            "clipboard.reconcile_progress.running",
            "clipboard.reconcile_progress.settled",
            "clipboard.reconcile_progress.settled",
            "clipboard.reconcile_progress.superseded"
        ]
    );
    for span in reconciliations {
        assert!(span.parent_span_id.is_empty());
        assert_eq!(field(&span.attributes, "uc.record.kind"), Some("business"));
        let expected = if span.name.ends_with(".settled") {
            "ok"
        } else if span.name.ends_with(".superseded") {
            "skipped"
        } else if span.name.ends_with(".access_denied") || span.name.ends_with(".invalid_input") {
            "rejected"
        } else {
            "deferred"
        };
        assert_eq!(field(&span.attributes, "uc.outcome"), Some(expected));
        let matches = logs
            .iter()
            .filter(|log| log.trace_id == span.trace_id && log.span_id == span.span_id)
            .collect::<Vec<_>>();
        assert_eq!(matches.len(), 1);
        assert_eq!(field(&matches[0].attributes, "uc.outcome"), Some(expected));
    }
    assert!(spans
        .iter()
        .any(|span| field(&span.attributes, "uc.operation") == Some("clipboard_receive")));
    let retry = spans
        .iter()
        .filter(|span| field(&span.attributes, "uc.operation") == Some("clipboard.retry_receive"))
        .collect::<Vec<_>>();
    assert_eq!(retry.len(), 1);
    assert_eq!(field(&retry[0].attributes, "uc.outcome"), Some("ok"));
    let requests = endpoint.received_requests().await.unwrap();
    for needle in [
        super::super::NAME,
        case.first.entry_id.as_str(),
        input.entry_id.as_str(),
        input.target_device_id.as_str(),
    ] {
        assert!(
            !requests.iter().any(|request| request
                .body
                .windows(needle.len())
                .any(|bytes| bytes == needle.as_bytes())),
            "raw telemetry must exclude content names and original business identities"
        );
    }
    if let Some(root) = std::env::var_os("UC_PROGRESS_OBSERVATION_EVIDENCE_DIR") {
        let root = PathBuf::from(root);
        std::fs::create_dir_all(&root).unwrap();
        for (index, request) in requests.iter().enumerate() {
            let signal = match request.url.path() {
                "/v1/traces" => "traces",
                "/v1/logs" => "logs",
                _ => continue,
            };
            std::fs::write(root.join(format!("{index}-{signal}.pb")), &request.body).unwrap();
        }
        let summary = serde_json::json!({
            "spans": spans.iter().map(|span| serde_json::json!({
                "name": span.name, "trace_id": span.trace_id, "span_id": span.span_id,
                "parent_span_id": span.parent_span_id, "start": span.start_time_unix_nano, "end": span.end_time_unix_nano,
                "operation": field(&span.attributes, "uc.operation"), "outcome": field(&span.attributes, "uc.outcome"),
                "kind": field(&span.attributes, "uc.record.kind")
            })).collect::<Vec<_>>(),
            "logs": logs.iter().map(|log| serde_json::json!({
                "trace_id": log.trace_id, "span_id": log.span_id,
                "operation": field(&log.attributes, "uc.operation"), "outcome": field(&log.attributes, "uc.outcome")
            })).collect::<Vec<_>>(),
            "exports": case.exports.exports.lock().unwrap().len(), "terminal_still_held": case.exports.hold_terminals.load(Ordering::SeqCst)
        });
        std::fs::write(
            root.join("summary.json"),
            serde_json::to_vec_pretty(&summary).unwrap(),
        )
        .unwrap();
    }
    case.exports.hold_terminals.store(false, Ordering::SeqCst);
    case.exports.terminal_release.add_permits(64);
    case.shutdown().await;
}

async fn records(endpoint: &MockServer) -> (Vec<Span>, Vec<LogRecord>) {
    let mut spans = Vec::new();
    let mut logs = Vec::new();
    for request in endpoint.received_requests().await.unwrap() {
        match request.url.path() {
            // 无关的根路径探测不是OTLP记录；隐私检查仍覆盖全部收到的请求。
            "/" if request.method.as_str() == "GET" => {}
            "/v1/traces" => spans.extend(
                ExportTraceServiceRequest::decode(request.body.as_slice())
                    .unwrap()
                    .resource_spans
                    .into_iter()
                    .flat_map(|resource| resource.scope_spans)
                    .flat_map(|scope| scope.spans),
            ),
            "/v1/logs" => logs.extend(
                ExportLogsServiceRequest::decode(request.body.as_slice())
                    .unwrap()
                    .resource_logs
                    .into_iter()
                    .flat_map(|resource| resource.scope_logs)
                    .flat_map(|scope| scope.log_records),
            ),
            _ => panic!("unexpected raw receiver path"),
        }
    }
    (spans, logs)
}
fn field<'a>(fields: &'a [KeyValue], key: &str) -> Option<&'a str> {
    fields
        .iter()
        .find(|field| field.key == key)
        .and_then(|field| match field.value.as_ref()?.value.as_ref()? {
            Value::StringValue(value) => Some(value.as_str()),
            _ => None,
        })
}
