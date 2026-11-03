use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
#[cfg(unix)]
use std::mem::MaybeUninit;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use libc::{getrusage, rusage, RUSAGE_SELF};
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

#[cfg(unix)]
fn process_cpu_time() -> Option<Duration> {
    let mut usage = MaybeUninit::<rusage>::uninit();
    // SAFETY: 输出指针指向完整、正确对齐的 rusage；仅在系统调用成功后读取。
    if unsafe { getrusage(RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: getrusage 已成功初始化完整输出。
    let usage = unsafe { usage.assume_init() };
    let micros = [usage.ru_utime, usage.ru_stime]
        .into_iter()
        .try_fold(0_u64, |total, time| {
            total.checked_add(
                u64::try_from(time.tv_sec)
                    .ok()?
                    .checked_mul(1_000_000)?
                    .checked_add(u64::try_from(time.tv_usec).ok()?)?,
            )
        })?;
    Some(Duration::from_micros(micros))
}

#[cfg(not(unix))]
fn process_cpu_time() -> Option<Duration> {
    None
}

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
        let errors = File::create(artifacts.join("process-errors.log")).expect("process errors");
        // 宿主持续排空两条管道；各使用 64 KiB 缓冲，避免逐条写工件污染业务计时。
        // 子进程仍运行完整公开运行期、真实系统输出和慢 HTTP Collector。
        let mut child = Command::new(std::env::current_exe().expect("test executable"))
            .args([
                "--exact",
                "unavailable_collector_and_full_batch_queue_do_not_block_business_work",
                "--nocapture",
            ])
            .env(CHILD_ARTIFACTS, &artifacts)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("run isolated collector scenario");
        let mut stdout = child.stdout.take().expect("child stdout");
        let mut stderr = child.stderr.take().expect("child stderr");
        let status = thread::scope(|scope| {
            let output = scope.spawn(move || {
                let mut writer = BufWriter::with_capacity(64 * 1024, output);
                io::copy(&mut stdout, &mut writer)?;
                writer.flush()
            });
            let errors = scope.spawn(move || {
                let mut writer = BufWriter::with_capacity(64 * 1024, errors);
                io::copy(&mut stderr, &mut writer)?;
                writer.flush()
            });
            let status = child.wait().expect("wait for collector scenario");
            output
                .join()
                .expect("output reader")
                .expect("retain output");
            errors.join().expect("error reader").expect("retain errors");
            status
        });
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

    // 先区分创建、进入、事件与关闭成本；保留完整负载、系统输出和总耗时断言。
    let mut stages = [Duration::ZERO; 4];
    let cpu_started = process_cpu_time();
    let started = Instant::now();
    for _ in 0..10_000 {
        let stage = Instant::now();
        let span = operation_span(OperationContext {
            domain: DiagnosticDomain::Runtime,
            operation: DiagnosticOperation::SessionLifecycle,
            role: DiagnosticRole::Local,
            kind: DiagnosticSpanKind::Internal,
        });
        stages[0] += stage.elapsed();
        let stage = Instant::now();
        let entered = span.enter();
        stages[1] += stage.elapsed();
        let stage = Instant::now();
        complete_operation(OperationCompletion::succeeded(
            DiagnosticDomain::Runtime,
            DiagnosticOperation::SessionLifecycle,
            DiagnosticRole::Local,
            Duration::ZERO,
        ));
        stages[2] += stage.elapsed();
        let stage = Instant::now();
        drop(entered);
        drop(span);
        stages[3] += stage.elapsed();
    }

    let elapsed = started.elapsed();
    let cpu_elapsed = process_cpu_time()
        .zip(cpu_started)
        .and_then(|(end, start)| end.checked_sub(start));
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
            "process_cpu_us": cpu_elapsed.map(|elapsed| elapsed.as_micros()),
            "producer_limit_us": Duration::from_secs(2).as_micros(),
            "span_create_us": stages[0].as_micros(),
            "span_enter_us": stages[1].as_micros(),
            "completion_event_us": stages[2].as_micros(),
            "span_exit_and_close_us": stages[3].as_micros(),
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
