use std::fs::{self, File};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use uc_observability_contract::diagnostics::{
    complete_operation, operation_span, DiagnosticDomain, DiagnosticOperation, DiagnosticRole,
    DiagnosticSpanKind, OperationCompletion, OperationContext,
};
use uc_observability_runtime::{
    DeploymentEnvironment, ObservabilityConfig, ObservabilityResource, OperatingSystem,
    OtlpHttpConfig, ProcessObservabilityRuntime,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test(flavor = "multi_thread")]
async fn unavailable_collector_and_full_batch_queue_do_not_block_business_work() {
    const CHILD_ARTIFACTS: &str = "UC_COLLECTOR_UNAVAILABLE_CHILD_ARTIFACTS";
    let artifacts = if let Some(directory) = std::env::var_os(CHILD_ARTIFACTS) {
        PathBuf::from(directory)
    } else {
        let temporary = tempfile::tempdir().expect("isolated collector artifacts");
        let artifacts = std::env::var_os("UC_TEST_ARTIFACTS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| temporary.path().to_path_buf())
            .join("collector-unavailable");
        fs::create_dir_all(&artifacts).expect("artifact directory");
        let output = File::create(artifacts.join("process-output.log")).expect("process output");
        // Linux 系统输出同步写 stdout；不得让 CI 捕获管道阻塞污染远程队列计时。
        // 保留真实系统输出，子进程仍运行完整的公开运行期和慢 HTTP Collector。
        let status = Command::new(std::env::current_exe().expect("test executable"))
            .args([
                "--exact",
                "unavailable_collector_and_full_batch_queue_do_not_block_business_work",
                "--nocapture",
            ])
            .env(CHILD_ARTIFACTS, &artifacts)
            .stdout(Stdio::from(output.try_clone().expect("clone output")))
            .stderr(Stdio::from(output))
            .status()
            .expect("run isolated collector scenario");
        assert!(
            status.success(),
            "collector scenario failed; artifacts: {artifacts:?}"
        );
        assert!(
            artifacts.join("summary.json").is_file(),
            "missing scenario evidence"
        );
        #[cfg(not(any(target_vendor = "apple", target_os = "android")))]
        assert_eq!(
            fs::read_to_string(artifacts.join("process-output.log"))
                .expect("retained system output")
                .lines()
                .filter(|line| line.contains("\"event.name\":\"uc.operation.completed\""))
                .count(),
            10_000,
            "all JSON system records must remain enabled and retained"
        );
        return;
    };
    let receiver = MockServer::start().await;
    for endpoint in ["/v1/traces", "/v1/logs"] {
        Mock::given(method("POST"))
            .and(path(endpoint))
            .respond_with(ResponseTemplate::new(503).set_delay(Duration::from_secs(5)))
            .mount(&receiver)
            .await;
    }
    let remote = OtlpHttpConfig::new_loopback(
        &format!("{}/v1/traces", receiver.uri()),
        &format!("{}/v1/logs", receiver.uri()),
    )
    .expect("valid slow endpoints")
    .with_timeout(Duration::from_secs(1))
    .expect("bounded timeout");
    let config = ObservabilityConfig::new(
        ObservabilityResource::new(
            "1.2.3",
            DeploymentEnvironment::Test,
            OperatingSystem::Other,
            "test",
        )
        .expect("approved resource"),
    )
    .with_remote(remote);
    let handle = tokio::task::spawn_blocking(move || ProcessObservabilityRuntime::install(config))
        .await
        .expect("install worker")
        .expect("runtime install must degrade at delivery time")
        .handle();

    let started = Instant::now();
    for _ in 0..10_000 {
        let span = operation_span(OperationContext {
            domain: DiagnosticDomain::Runtime,
            operation: DiagnosticOperation::SessionLifecycle,
            role: DiagnosticRole::Local,
            kind: DiagnosticSpanKind::Internal,
        });
        let _entered = span.enter();
        complete_operation(OperationCompletion::succeeded(
            DiagnosticDomain::Runtime,
            DiagnosticOperation::SessionLifecycle,
            DiagnosticRole::Local,
            Duration::ZERO,
        ));
    }

    let elapsed = started.elapsed();
    let flush = handle.force_flush(Duration::from_millis(1_500));
    let health = handle.health();
    let shutdown = handle.shutdown(Duration::from_millis(200));
    let requests = receiver
        .received_requests()
        .await
        .expect("collector requests");
    let trace_requests = requests
        .iter()
        .filter(|request| request.url.path() == "/v1/traces")
        .count();
    let log_requests = requests
        .iter()
        .filter(|request| request.url.path() == "/v1/logs")
        .count();
    fs::write(
        artifacts.join("summary.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "business_operations": 10_000,
            "producer_elapsed_us": elapsed.as_micros(),
            "producer_limit_us": Duration::from_secs(2).as_micros(),
            "dropped_remote_spans": health.dropped_remote_spans,
            "dropped_remote_logs": health.dropped_remote_logs,
            "failed_remote_span_batches": health.failed_remote_span_batches,
            "failed_remote_log_batches": health.failed_remote_log_batches,
            "trace_http_requests": trace_requests,
            "log_http_requests": log_requests,
            "operating_system": std::env::consts::OS,
            "flush": format!("{flush:?}"),
            "shutdown": format!("{shutdown:?}"),
        }))
        .expect("serialize collector evidence"),
    )
    .expect("write collector evidence");
    assert!(elapsed < Duration::from_secs(2));
    assert!(health.dropped_remote_spans > 0);
    assert!(health.dropped_remote_logs > 0);
    assert!(health.failed_remote_span_batches > 0);
    assert!(health.failed_remote_log_batches > 0);
    assert!(trace_requests > 0);
    assert!(log_requests > 0);
}
