use opentelemetry::trace::TraceContextExt;
use std::time::Duration;
use tracing::Instrument;
use tracing_opentelemetry::OpenTelemetrySpanExt;

use uc_observability_contract::diagnostics::{
    complete_operation, complete_unassociated_operation, operation_span, DiagnosticDomain,
    DiagnosticOperation, DiagnosticRole, DiagnosticSpanKind, OperationCompletion, OperationContext,
};
use uc_observability_runtime::*;

#[test]
fn local_file_keeps_valid_correlation_without_remote_export_and_respects_detachment() {
    let directory = tempfile::tempdir().expect("logs");
    let config = ObservabilityConfig::new(
        ObservabilityResource::new(
            "1.1.0",
            DeploymentEnvironment::Test,
            OperatingSystem::Macos,
            "test",
        )
        .expect("resource"),
    )
    .with_local_logs(LocalLogConfig::new(directory.path()));
    let _handle = ProcessObservabilityRuntime::install(config)
        .expect("install")
        .handle();
    let completion = || {
        OperationCompletion::succeeded(
            DiagnosticDomain::Runtime,
            DiagnosticOperation::SessionRecovery,
            DiagnosticRole::Local,
            Duration::from_millis(4),
        )
    };
    let span = operation_span(OperationContext {
        domain: DiagnosticDomain::Runtime,
        operation: DiagnosticOperation::SessionRecovery,
        role: DiagnosticRole::Local,
        kind: DiagnosticSpanKind::Internal,
    });
    span.in_scope(|| {
        complete_operation(completion());
        complete_unassociated_operation(completion());
    });
    let mut expected = Vec::new();
    let runtime = tokio::runtime::Runtime::new().expect("async runtime");
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    runtime.block_on(async {
        let mut tasks = Vec::new();
        for duration in [11, 22] {
            let span = operation_span(OperationContext {
                domain: DiagnosticDomain::Runtime,
                operation: DiagnosticOperation::SessionRecovery,
                role: DiagnosticRole::Local,
                kind: DiagnosticSpanKind::Internal,
            });
            expected.push((
                duration,
                span.context().span().span_context().trace_id().to_string(),
            ));
            let barrier = barrier.clone();
            tasks.push(tokio::spawn(
                async move {
                    barrier.wait().await;
                    tokio::task::yield_now().await;
                    complete_operation(OperationCompletion::succeeded(
                        DiagnosticDomain::Runtime,
                        DiagnosticOperation::SessionRecovery,
                        DiagnosticRole::Local,
                        Duration::from_millis(duration),
                    ));
                }
                .instrument(span),
            ));
        }
        for task in tasks {
            task.await.expect("record task");
        }
    });
    assert_eq!(
        ProcessObservabilityRuntime::flush_local_logs(Duration::from_secs(5)),
        SignalResult::Completed
    );
    let output = std::fs::read_dir(directory.path())
        .expect("files")
        .map(|e| std::fs::read_to_string(e.expect("file").path()).expect("content"))
        .collect::<String>();
    let rows: Vec<serde_json::Value> = output
        .lines()
        .map(|l| serde_json::from_str(l).expect("JSON"))
        .collect();
    assert_eq!(rows.len(), 4, "one file record per completion");
    assert_eq!(
        rows[0]["trace_id"].as_str().map(str::len),
        Some(32),
        "a local-only runtime must preserve its trusted trace"
    );
    assert_eq!(rows[0]["span_id"].as_str().map(str::len), Some(16));
    assert!(
        rows[1].get("trace_id").is_none(),
        "unassociated completion must stay unassociated"
    );
    assert!(rows[1].get("span_id").is_none());
    assert_ne!(expected[0].1, expected[1].1);
    for (duration, trace_id) in expected {
        let record = rows
            .iter()
            .find(|r| r["fields"]["duration_ms"] == duration)
            .expect("concurrent record");
        assert_eq!(
            record["trace_id"], trace_id,
            "parallel operations must not inherit each other's context"
        );
    }
}
