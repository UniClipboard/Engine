//! 配对热路径耗时、配对观测模式与准入 trace 验收。

use super::*;

const PAIRING_HOT_PATH_BUDGET: Duration = Duration::from_secs(1);

const PAIRING_HANDOVER_BUDGET: Duration = Duration::from_secs(3);

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "显式完整配对 trace 验收：独占进程级 OTLP receiver，使用 --ignored 精确运行"]
async fn uninterrupted_admission_uses_one_trace() {
    let _scenario = TestScenario::start();
    let telemetry = MockServer::start().await;
    for endpoint in ["/v1/traces", "/v1/logs"] {
        Mock::given(method("POST"))
            .and(path(endpoint))
            .respond_with(ResponseTemplate::new(200))
            .mount(&telemetry)
            .await;
    }
    let local_logs = TempDir::new().expect("local logs");
    let os = match std::env::consts::OS {
        "macos" => OperatingSystem::Macos,
        "linux" => OperatingSystem::Linux,
        "windows" => OperatingSystem::Windows,
        _ => OperatingSystem::Other,
    };
    let config = ObservabilityConfig::new(
        ObservabilityResource::new(
            env!("CARGO_PKG_VERSION"),
            DeploymentEnvironment::Test,
            os,
            "test",
        )
        .expect("resource"),
    )
    .with_remote(
        OtlpHttpConfig::new_loopback(
            &format!("{}/v1/traces", telemetry.uri()),
            &format!("{}/v1/logs", telemetry.uri()),
        )
        .expect("OTLP"),
    )
    .with_local_logs(LocalLogConfig::new(local_logs.path()));
    let observation = std::thread::spawn(move || {
        ProcessObservabilityRuntime::install(config)
            .expect("runtime")
            .handle()
    })
    .join()
    .expect("install");

    let rendezvous = mount_rendezvous().await;
    let sponsor_harness = DeviceHarness::new(rendezvous.uri());
    let joiner_harness = DeviceHarness::new(rendezvous.uri());
    let sponsor = sponsor_harness.start().await;
    let joiner = joiner_harness.start().await;
    let space_id = create_space(&sponsor, "Sponsor").await.0;
    let invitation = issue_invitation(&sponsor).await;
    let started_at = SystemTime::now();
    let started = Instant::now();

    join_with_invitation(&joiner, "Joiner", &space_id, invitation).await;
    wait_for_active_member_count(&sponsor, 2).await;
    wait_for_active_member_count(&joiner, 2).await;
    let elapsed = started.elapsed();

    // Active 早于最后一轮通信完成；先收齐证据，不能用关闭打断待验收的确认。
    let evidence_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let trace_evidence = loop {
        let _ = observation.force_flush(Duration::from_secs(10));
        let requests = telemetry
            .received_requests()
            .await
            .expect("OTLP request capture");
        let evidence = pairing_trace_evidence(&requests, started_at, elapsed);
        if evidence.client_count >= 4
            && evidence.paired_server_count >= 4
            && evidence.paired_admission_endpoint_count >= evidence.client_count
            && evidence.lifecycle_root_count == 1
            && evidence.invalid_completion_log_count == 0
        {
            break evidence;
        }
        assert!(
            tokio::time::Instant::now() < evidence_deadline,
            "OTLP receiver did not collect four complete admission exchanges: {evidence:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };

    assert_eq!(trace_evidence.flow_ids.len(), 1);
    assert_eq!(trace_evidence.lifecycle_root_count, 1);
    assert_eq!(
        trace_evidence.invalid_pair_count, 0,
        "{:?}",
        trace_evidence.invalid_pair_details
    );
    assert_eq!(trace_evidence.invalid_completion_log_count, 0);
    let requests = telemetry
        .received_requests()
        .await
        .expect("captured requests");
    assert_readable_admission_actions(&requests);
    assert_eq!(
        trace_evidence.admission_trace_ids.len(),
        1,
        "one uninterrupted admission must be visible as one trace: {:?}",
        trace_evidence.admission_trace_counts,
    );
    let _ = observation.force_flush(Duration::from_secs(10));
    let local_records: Vec<serde_json::Value> = std::fs::read_dir(local_logs.path())
        .expect("files")
        .flat_map(|entry| {
            std::fs::read_to_string(entry.expect("entry").path())
                .expect("file")
                .lines()
                .map(|line| serde_json::from_str(line).expect("record"))
                .collect::<Vec<_>>()
        })
        .collect();
    let lifecycle = local_records
        .iter()
        .find(|r| {
            r["fields"]["event.name"] == "uc.operation.completed"
                && r["fields"]["uc.role"] == "local"
                && r["fields"]["uc.domain"] == "space_admission"
        })
        .expect("lifecycle");
    for (step, role) in [
        ("sponsor_state_load", "sponsor"),
        ("sponsor_state_commit", "sponsor"),
        ("sponsor_prepare_candidate", "sponsor"),
        ("sponsor_prepare_commit", "sponsor"),
        ("sponsor_prepare_complete", "sponsor"),
        ("sponsor_prepare_settled", "sponsor"),
        ("sponsor_activate", "sponsor"),
        ("re_pairing_state_commit", "sponsor"),
        ("joiner_process_reply", "joiner"),
        ("joiner_prepare_candidate", "joiner"),
        ("joiner_prepare_applied", "joiner"),
        ("joiner_prepare_activation", "joiner"),
        ("joiner_activate", "joiner"),
    ] {
        let records: Vec<_> = local_records
            .iter()
            .filter(|r| {
                r["fields"]["event.name"] == "runtime.work.finished"
                    && r["fields"]["step"] == step
                    && r["fields"]["uc.role"] == role
            })
            .collect();
        assert!(!records.is_empty(), "缺少真实内部步骤：{step}");
        for record in records {
            assert_eq!(
                record["trace_id"], lifecycle["trace_id"],
                "步骤关联丢失：{step}"
            );
            assert_eq!(record["capture_mode"], "standard");
            assert_eq!(
                record["fields"]["uc.outcome"], "ok",
                "正常配对步骤结果：{step}"
            );
        }
    }
    for message in ["join_request", "prepared", "applied", "complete_ack"] {
        for role in ["joiner", "sponsor"] {
            assert!(
                local_records
                    .iter()
                    .any(|r| r["fields"]["message"] == message
                        && r["fields"]["uc.role"] == role
                        && r["trace_id"] == lifecycle["trace_id"]),
                "四轮协议必须在两端可识别：{role} {message}"
            );
        }
    }
    let transition = local_records
        .iter()
        .find(|r| {
            r["fields"]["event.name"] == "runtime.work.finished"
                && r["fields"]["step"] == "session_complete_transition"
        })
        .expect("实际会话切换");
    assert!(transition["trace_id"].is_string());
    assert!(
        local_records
            .iter()
            .any(|r| r["fields"]["event.name"] == "uc.operation.completed"
                && r["fields"]["uc.operation"] == "session_lifecycle"
                && r["trace_id"] == transition["trace_id"]),
        "实际切换必须有完整结果"
    );
    for step in [
        "session_drain_operations",
        "session_stop_application",
        "session_stop_network",
        "session_complete_transition",
        "session_prepare",
        "session_start",
        "session_recover",
    ] {
        assert!(
            local_records
                .iter()
                .any(|r| r["fields"]["event.name"] == "runtime.work.finished"
                    && r["fields"]["step"] == step
                    && r["trace_id"] == transition["trace_id"]),
            "实际会话切换缺少关联步骤：{step}"
        );
    }
    sponsor
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("stop sponsor");
    joiner
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("stop joiner");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "显式重启链路验收：独占进程级 OTLP receiver，使用 --ignored 精确运行"]
async fn in_flight_admission_restart_uses_new_traces_and_one_flow() {
    let _scenario = TestScenario::start();
    let telemetry = MockServer::start().await;
    for endpoint in ["/v1/traces", "/v1/logs"] {
        Mock::given(method("POST"))
            .and(path(endpoint))
            .respond_with(ResponseTemplate::new(200))
            .mount(&telemetry)
            .await;
    }
    assert!(uc_engine::init_test_tracing_with_otlp(
        &format!("{}/v1/traces", telemetry.uri()),
        &format!("{}/v1/logs", telemetry.uri()),
    ));

    let rendezvous = mount_rendezvous().await;
    let sponsor_harness = DeviceHarness::new(rendezvous.uri());
    let joiner_harness = DeviceHarness::new(rendezvous.uri());
    let sponsor = sponsor_harness.start().await;
    let joiner = joiner_harness.start().await;
    let space_id = create_space(&sponsor, "Sponsor").await.0;
    let invitation = issue_invitation(&sponsor).await;
    let OperationResult::JoinSpace(status) = joiner
        .execute(Operation::JoinSpace(JoinSpaceInput {
            invitation_code: invitation,
            device_name: Some("Restarted Joiner".to_owned()),
            passphrase: SecretString::new(PASSPHRASE),
            preserve_unreadable_history: false,
        }))
        .await
        .expect("start restartable admission")
    else {
        panic!("unexpected join result");
    };
    assert!(matches!(&status, JoinSpaceStatusSummary::Pending { .. }));

    let evidence_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let (flow_id, before_restart_traces) = loop {
        uc_engine::flush_test_tracing();
        let requests = telemetry
            .received_requests()
            .await
            .expect("pre-restart OTLP requests");
        let flows = complete_admission_flow_traces(&requests);
        if flows.len() == 1 {
            let (flow_id, traces) = flows.into_iter().next().expect("one admission flow");
            if !traces.is_empty() {
                break (
                    flow_id,
                    traces
                        .into_iter()
                        .map(|trace| trace.trace_id)
                        .collect::<BTreeSet<_>>(),
                );
            }
        }
        assert!(
            tokio::time::Instant::now() < evidence_deadline,
            "admission emitted no pre-restart trace"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    let OperationResult::DeviceGroupChoices(before_restart) = joiner
        .execute(Operation::QueryDeviceGroupChoices)
        .await
        .expect("query in-flight admission")
    else {
        panic!("unexpected device trust result");
    };
    assert!(matches!(
        before_restart.device_trust.current_join,
        Some(JoinSpaceStatusSummary::Pending { .. })
    ));

    joiner
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("stop in-flight joiner");
    let restarted_at_ns = unix_time_ns(SystemTime::now());
    let restarted_joiner = joiner_harness.start().await;
    wait_for_completed_join(&restarted_joiner, "Restarted Joiner", status, &space_id).await;
    wait_for_active_member_count(&sponsor, 2).await;
    wait_for_active_member_count(&restarted_joiner, 2).await;
    sponsor
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("stop sponsor");
    restarted_joiner
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("stop restarted joiner");
    uc_engine::flush_test_tracing();

    let requests = telemetry
        .received_requests()
        .await
        .expect("post-restart OTLP requests");
    let flows = complete_admission_flow_traces(&requests);
    assert_eq!(flows.len(), 1);
    let after_restart_traces = flows.get(&flow_id).expect("same flow after restart");
    assert!(
        after_restart_traces
            .iter()
            .any(|trace| trace.started_at_ns >= restarted_at_ns
                && !before_restart_traces.contains(&trace.trace_id)),
        "restart produced no new complete online trace"
    );
}

// 新设备只经过稳定 JoinSpace 入口，并最终形成可查询的活动 Space。
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "显式性能门禁：需要空闲的本机真实网络栈，使用 --ignored 运行"]
async fn two_device_hot_path_pairing_completes_within_one_second() {
    let _scenario = TestScenario::start();
    let telemetry = start_test_telemetry().await;
    assert!(uc_engine::init_test_tracing_with_otlp(
        &format!("{}/v1/traces", telemetry.uri()),
        &format!("{}/v1/logs", telemetry.uri()),
    ));
    let (started_at, elapsed) = measure_two_device_pairing().await;
    let trace_evidence = collect_pairing_trace_evidence(&telemetry, started_at, elapsed).await;
    assert_pairing_trace_evidence(&trace_evidence);
    assert!(
        trace_evidence.local_elapsed < PAIRING_HOT_PATH_BUDGET,
        "two-device local pairing work took {:?}, network-related time {:?}, end-to-end {:?}, local budget {:?}",
        trace_evidence.local_elapsed,
        trace_evidence.network_elapsed,
        elapsed,
        PAIRING_HOT_PATH_BUDGET,
    );
}

// 普通 Space 交接复用同一网络入口，端到端配对必须稳定落在三秒预算内。
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "显式性能门禁：需要空闲的本机真实网络栈，使用 --ignored 运行"]
async fn two_device_pairing_with_session_handover_completes_within_three_seconds() {
    let _scenario = TestScenario::start();
    let telemetry = start_test_telemetry().await;
    assert!(uc_engine::init_test_tracing_with_otlp(
        &format!("{}/v1/traces", telemetry.uri()),
        &format!("{}/v1/logs", telemetry.uri()),
    ));
    let (started_at, elapsed) = measure_two_device_pairing().await;
    let trace_evidence = collect_pairing_trace_evidence(&telemetry, started_at, elapsed).await;
    assert_pairing_trace_evidence(&trace_evidence);
    eprintln!(
        "UC_PAIRING_HANDOVER_RESULT elapsed_us={} local_us={} network_us={}",
        elapsed.as_micros(),
        trace_evidence.local_elapsed.as_micros(),
        trace_evidence.network_elapsed.as_micros(),
    );
    assert!(
        elapsed < PAIRING_HANDOVER_BUDGET,
        "two-device end-to-end pairing took {elapsed:?}, budget {PAIRING_HANDOVER_BUDGET:?}",
    );
}

// 不同观测装配只验证配对能完成，不承担一秒或三秒性能门。
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "显式观测开销验收：通过 UC_PAIRING_OBSERVABILITY_BENCHMARK 选择模式"]
async fn two_device_pairing_observability_mode_completes() {
    let _scenario = TestScenario::start();
    let mode = std::env::var("UC_PAIRING_OBSERVABILITY_BENCHMARK")
        .expect("UC_PAIRING_OBSERVABILITY_BENCHMARK must select an observability mode");
    let telemetry = if mode == "healthy" {
        Some(start_test_telemetry().await)
    } else {
        None
    };
    match mode.as_str() {
        "none" => {}
        "disabled" => assert!(uc_engine::init_test_tracing_without_remote()),
        "unavailable" => assert!(uc_engine::init_test_tracing_with_otlp(
            "http://127.0.0.1:1/v1/traces",
            "http://127.0.0.1:1/v1/logs",
        )),
        "healthy" => {
            let telemetry = telemetry.as_ref().expect("healthy OTLP receiver");
            assert!(uc_engine::init_test_tracing_with_otlp(
                &format!("{}/v1/traces", telemetry.uri()),
                &format!("{}/v1/logs", telemetry.uri()),
            ));
        }
        other => panic!("unknown observability benchmark mode: {other}"),
    }
    let (_, elapsed) = measure_two_device_pairing().await;
    eprintln!(
        "UC_PAIRING_OBSERVABILITY_RESULT mode={mode} elapsed_us={}",
        elapsed.as_micros()
    );
    assert!(elapsed < Duration::from_secs(30));
}

async fn start_test_telemetry() -> MockServer {
    let telemetry = MockServer::start().await;
    for endpoint in ["/v1/traces", "/v1/logs"] {
        Mock::given(method("POST"))
            .and(path(endpoint))
            .respond_with(ResponseTemplate::new(200))
            .mount(&telemetry)
            .await;
    }
    telemetry
}

async fn measure_two_device_pairing() -> (SystemTime, Duration) {
    let rendezvous = mount_rendezvous().await;
    let sponsor_harness = DeviceHarness::new(rendezvous.uri());
    let joiner_harness = DeviceHarness::new(rendezvous.uri());
    let sponsor = sponsor_harness.start().await;
    let joiner = joiner_harness.start().await;
    let space_id = create_space(&sponsor, "Sponsor").await.0;
    let invitation = issue_invitation(&sponsor).await;

    let started_at = SystemTime::now();
    let started = Instant::now();
    join_with_invitation(&joiner, "Joiner", &space_id, invitation).await;
    wait_for_active_member_count(&sponsor, 2).await;
    wait_for_active_member_count(&joiner, 2).await;
    let elapsed = started.elapsed();

    sponsor
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shut down sponsor");
    joiner
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shut down joiner");
    uc_engine::flush_test_tracing();
    (started_at, elapsed)
}

async fn collect_pairing_trace_evidence(
    telemetry: &MockServer,
    started_at: SystemTime,
    elapsed: Duration,
) -> PairingTraceEvidence {
    let evidence_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        uc_engine::flush_test_tracing();
        let requests = telemetry
            .received_requests()
            .await
            .expect("OTLP request capture");
        let evidence = pairing_trace_evidence(&requests, started_at, elapsed);
        if evidence.client_count >= 4
            && evidence.paired_server_count >= 4
            && evidence.paired_admission_endpoint_count >= evidence.client_count
            && evidence.lifecycle_root_count == 1
            && evidence.invalid_completion_log_count == 0
        {
            break evidence;
        }
        assert!(
            tokio::time::Instant::now() < evidence_deadline,
            "OTLP receiver did not collect four complete admission exchanges"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn assert_pairing_trace_evidence(trace_evidence: &PairingTraceEvidence) {
    assert!(trace_evidence.client_count >= 4);
    assert!(trace_evidence.paired_server_count >= 4);
    assert_eq!(
        trace_evidence.paired_admission_endpoint_count,
        trace_evidence.client_count
    );
    assert_eq!(trace_evidence.invalid_pair_count, 0);
    assert_eq!(trace_evidence.invalid_completion_log_count, 0);
    assert_eq!(trace_evidence.flow_ids.len(), 1);
    assert_eq!(trace_evidence.lifecycle_root_count, 1);
    assert_eq!(trace_evidence.admission_trace_ids.len(), 1);
}

#[derive(Debug)]
struct PairingTraceEvidence {
    local_elapsed: Duration,
    network_elapsed: Duration,
    client_count: usize,
    paired_server_count: usize,
    paired_admission_endpoint_count: usize,
    lifecycle_root_count: usize,
    invalid_pair_count: usize,
    invalid_pair_details: Vec<String>,
    invalid_completion_log_count: usize,
    flow_ids: BTreeSet<String>,
    admission_trace_ids: BTreeSet<Vec<u8>>,
    admission_trace_counts: BTreeMap<Vec<u8>, usize>,
}

fn assert_readable_admission_actions(requests: &[Request]) {
    let spans = requests
        .iter()
        .filter(|request| request.url.path() == "/v1/traces")
        .flat_map(|request| {
            ExportTraceServiceRequest::decode(request.body.as_slice())
                .expect("trace batch")
                .resource_spans
        })
        .flat_map(|resource| resource.scope_spans)
        .flat_map(|scope| scope.spans)
        .collect::<Vec<_>>();
    for action in [
        "request_join",
        "confirm_prepared",
        "confirm_applied",
        "settle",
    ] {
        let send_name = format!("pairing.{action}.send");
        let sends = spans
            .iter()
            .filter(|span| span.name == send_name)
            .collect::<Vec<_>>();
        assert_eq!(
            sends.len(),
            1,
            "each protocol purpose must be visible: {send_name}"
        );
        let send = sends[0];
        let receive = spans
            .iter()
            .find(|span| {
                span.trace_id == send.trace_id
                    && span.parent_span_id == send.span_id
                    && span.name == "pairing.receive_request"
            })
            .expect("Sponsor receive under send");
        assert!(
            spans.iter().any(|span| span.trace_id == receive.trace_id
                && span.parent_span_id == receive.span_id
                && span.name == format!("pairing.{action}.process")),
            "Sponsor must describe the same request purpose"
        );
    }
    assert_eq!(
        spans
            .iter()
            .filter(|span| span.name == "pairing.lifecycle")
            .count(),
        1
    );
    assert_eq!(
        spans
            .iter()
            .filter(|span| span.name == "pairing.authenticate")
            .count(),
        1
    );
    assert_eq!(
        spans
            .iter()
            .filter(|span| span.name == "pairing.reconnect")
            .count(),
        3
    );
}

fn pairing_trace_evidence(
    requests: &[Request],
    started_at: SystemTime,
    elapsed: Duration,
) -> PairingTraceEvidence {
    let started_ns = u64::try_from(
        started_at
            .duration_since(UNIX_EPOCH)
            .expect("wall clock after epoch")
            .as_nanos(),
    )
    .unwrap_or(u64::MAX);
    let finished_ns =
        started_ns.saturating_add(u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX));
    let mut spans = Vec::new();
    for request in requests
        .iter()
        .filter(|request| request.url.path() == "/v1/traces")
    {
        let batch = ExportTraceServiceRequest::decode(request.body.as_slice())
            .expect("decode pairing trace batch");
        spans.extend(
            batch
                .resource_spans
                .into_iter()
                .flat_map(|resource| resource.scope_spans)
                .flat_map(|scope| scope.spans),
        );
    }
    let mut completion_logs = HashMap::<(Vec<u8>, Vec<u8>), usize>::new();
    let mut completion_durations = HashMap::new();
    let mut log_flow_violation_count = 0;
    for request in requests
        .iter()
        .filter(|request| request.url.path() == "/v1/logs")
    {
        let batch = ExportLogsServiceRequest::decode(request.body.as_slice())
            .expect("decode pairing log batch");
        for log in batch
            .resource_logs
            .into_iter()
            .flat_map(|resource| resource.scope_logs)
            .flat_map(|scope| scope.log_records)
        {
            if otlp_string_attribute(&log.attributes, "event.name")
                != Some("uc.operation.completed")
                || otlp_string_attribute(&log.attributes, "uc.domain") != Some("space_admission")
            {
                continue;
            }
            if otlp_string_attribute(&log.attributes, "uc.flow.id").is_some() {
                log_flow_violation_count += 1;
            }
            if let Some(ms) = log.attributes.iter().find_map(|field| {
                if field.key != "duration_ms" {
                    return None;
                }
                match field.value.as_ref()?.value.as_ref()? {
                    OtlpValue::IntValue(value) => Some(*value),
                    _ => None,
                }
            }) {
                completion_durations.insert((log.trace_id.clone(), log.span_id.clone()), ms);
            }
            *completion_logs
                .entry((log.trace_id, log.span_id))
                .or_default() += 1;
        }
    }

    let relevant = spans
        .iter()
        .filter(|span| {
            otlp_string_attribute(&span.attributes, "uc.domain") == Some("space_admission")
                && otlp_string_attribute(&span.attributes, "uc.operation")
                    == Some("network_transport")
        })
        .collect::<Vec<_>>();
    let client_kind = opentelemetry_proto::tonic::trace::v1::span::SpanKind::Client as i32;
    let server_kind = opentelemetry_proto::tonic::trace::v1::span::SpanKind::Server as i32;
    let internal_kind = opentelemetry_proto::tonic::trace::v1::span::SpanKind::Internal as i32;
    let clients = relevant
        .iter()
        .copied()
        .filter(|span| span.kind == client_kind)
        .collect::<Vec<_>>();
    let lifecycle_roots = spans
        .iter()
        .filter(|span| {
            span.kind == internal_kind
                && span.parent_span_id.is_empty()
                && otlp_string_attribute(&span.attributes, "uc.domain") == Some("space_admission")
                && otlp_string_attribute(&span.attributes, "uc.operation")
                    == Some("space_admission")
                && otlp_string_attribute(&span.attributes, "uc.role") == Some("local")
                && otlp_string_attribute(&span.attributes, "uc.flow.id").is_some()
        })
        .collect::<Vec<_>>();
    let lifecycle_root_ids = lifecycle_roots
        .iter()
        .map(|span| ((span.trace_id.clone(), span.span_id.clone()), *span))
        .collect::<HashMap<_, _>>();
    let client_ids = clients
        .iter()
        .map(|span| ((span.trace_id.clone(), span.span_id.clone()), *span))
        .collect::<HashMap<_, _>>();
    let paired_servers = relevant
        .iter()
        .filter_map(|span| {
            (span.kind == server_kind)
                .then(|| {
                    client_ids
                        .get(&(span.trace_id.clone(), span.parent_span_id.clone()))
                        .map(|client| (*client, *span))
                })
                .flatten()
        })
        .collect::<Vec<_>>();
    let server_ids = paired_servers
        .iter()
        .map(|(_, server)| ((server.trace_id.clone(), server.span_id.clone()), *server))
        .collect::<HashMap<_, _>>();
    let paired_admission_endpoints = spans
        .iter()
        .filter(|span| {
            otlp_string_attribute(&span.attributes, "uc.operation") == Some("space_admission")
                && span.kind == internal_kind
                && otlp_string_attribute(&span.attributes, "uc.role") == Some("sponsor")
                && otlp_string_attribute(&span.attributes, "uc.flow.id").is_none()
                && server_ids.contains_key(&(span.trace_id.clone(), span.parent_span_id.clone()))
        })
        .collect::<Vec<_>>();
    let paired_admission_endpoint_count = paired_admission_endpoints.len();
    let invalid_completion_log_count = clients
        .iter()
        .copied()
        .chain(paired_servers.iter().map(|(_, server)| *server))
        .chain(paired_admission_endpoints.iter().copied())
        .chain(lifecycle_roots.iter().copied())
        .filter(|span| {
            completion_logs
                .get(&(span.trace_id.clone(), span.span_id.clone()))
                .copied()
                != Some(1)
        })
        .count()
        + log_flow_violation_count;
    let flow_ids = relevant
        .iter()
        .filter_map(|span| otlp_string_attribute(&span.attributes, "uc.flow.id"))
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    let admission_trace_ids = clients
        .iter()
        .map(|span| span.trace_id.clone())
        .collect::<BTreeSet<_>>();
    let mut admission_trace_counts = BTreeMap::new();
    for client in &clients {
        *admission_trace_counts
            .entry(client.trace_id.clone())
            .or_insert(0) += 1;
    }
    let mut invalid_pair_details = Vec::new();
    let mut invalid_pair_count = clients
        .iter()
        .filter(|client| {
            let root =
                lifecycle_root_ids.get(&(client.trace_id.clone(), client.parent_span_id.clone()));
            let invalid = client.parent_span_id.is_empty()
                || root.is_none()
                || root.and_then(|span| otlp_string_attribute(&span.attributes, "uc.flow.id"))
                    != otlp_string_attribute(&client.attributes, "uc.flow.id")
                || otlp_string_attribute(&client.attributes, "uc.flow.id").is_none()
                || !otlp_span_succeeded(client);
            if invalid {
                invalid_pair_details.push(format!(
                    "client {} root_present={} status={:?}",
                    client.name,
                    root.is_some(),
                    client.status
                ));
            }
            invalid
        })
        .count()
        + paired_servers
            .iter()
            .filter(|(_, server)| {
                let invalid = otlp_string_attribute(&server.attributes, "uc.flow.id").is_some()
                    || !otlp_span_succeeded(server);
                if invalid {
                    invalid_pair_details
                        .push(format!("server {} status={:?}", server.name, server.status));
                }
                invalid
            })
            .count()
        + spans
            .iter()
            .filter(|span| {
                otlp_string_attribute(&span.attributes, "uc.operation") == Some("space_admission")
                    && span.kind == internal_kind
                    && otlp_string_attribute(&span.attributes, "uc.role") == Some("sponsor")
                    && server_ids
                        .contains_key(&(span.trace_id.clone(), span.parent_span_id.clone()))
                    && !otlp_span_succeeded(span)
            })
            .count();
    for connection in spans.iter().filter(|span| {
        span.kind == client_kind
            && otlp_string_attribute(&span.attributes, "uc.operation") == Some("space_admission")
            && otlp_string_attribute(&span.attributes, "uc.role") == Some("joiner")
    }) {
        let duration_ms = connection
            .end_time_unix_nano
            .saturating_sub(connection.start_time_unix_nano)
            / 1_000_000;
        assert!(
            completion_durations
                .get(&(connection.trace_id.clone(), connection.span_id.clone()))
                .is_some_and(|ms| duration_ms.abs_diff(*ms as u64) <= 20),
            "connection span outlives its completed operation: {} ms",
            duration_ms
        );
    }
    let mut network_intervals = paired_servers
        .iter()
        .flat_map(|(client, server)| {
            if client.start_time_unix_nano > server.start_time_unix_nano
                || server.end_time_unix_nano > client.end_time_unix_nano
            {
                invalid_pair_details.push(format!(
                    "配对两端时间边界：client={} server={} start_delta_ns={} end_delta_ns={}",
                    client.name,
                    server.name,
                    i128::from(server.start_time_unix_nano)
                        - i128::from(client.start_time_unix_nano),
                    i128::from(server.end_time_unix_nano) - i128::from(client.end_time_unix_nano)
                ));
                invalid_pair_count += 1;
                return Vec::new();
            }
            [
                (client.start_time_unix_nano, server.start_time_unix_nano),
                (server.end_time_unix_nano, client.end_time_unix_nano),
            ]
            .into_iter()
            .filter_map(|(start, end)| {
                let start = start.max(started_ns);
                let end = end.min(finished_ns);
                (start < end).then_some((start, end))
            })
            .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    network_intervals.sort_unstable();
    let mut network_ns = 0_u64;
    let mut merged: Option<(u64, u64)> = None;
    for (start, end) in network_intervals {
        match merged {
            Some((current_start, current_end)) if start <= current_end => {
                merged = Some((current_start, current_end.max(end)));
            }
            Some((current_start, current_end)) => {
                network_ns = network_ns.saturating_add(current_end - current_start);
                merged = Some((start, end));
            }
            None => merged = Some((start, end)),
        }
    }
    if let Some((start, end)) = merged {
        network_ns = network_ns.saturating_add(end - start);
    }
    let network_elapsed = Duration::from_nanos(network_ns);
    PairingTraceEvidence {
        local_elapsed: elapsed.saturating_sub(network_elapsed),
        network_elapsed,
        client_count: clients.len(),
        paired_server_count: paired_servers.len(),
        paired_admission_endpoint_count,
        lifecycle_root_count: lifecycle_roots.len(),
        invalid_pair_count,
        invalid_pair_details,
        invalid_completion_log_count,
        flow_ids,
        admission_trace_ids,
        admission_trace_counts,
    }
}

struct CompleteAdmissionTrace {
    trace_id: Vec<u8>,
    started_at_ns: u64,
}

fn complete_admission_flow_traces(
    requests: &[Request],
) -> HashMap<String, Vec<CompleteAdmissionTrace>> {
    let mut spans = Vec::new();
    for request in requests
        .iter()
        .filter(|request| request.url.path() == "/v1/traces")
    {
        let batch = ExportTraceServiceRequest::decode(request.body.as_slice())
            .expect("decode admission trace batch");
        spans.extend(
            batch
                .resource_spans
                .into_iter()
                .flat_map(|resource| resource.scope_spans)
                .flat_map(|scope| scope.spans),
        );
    }
    let client_kind = opentelemetry_proto::tonic::trace::v1::span::SpanKind::Client as i32;
    let server_kind = opentelemetry_proto::tonic::trace::v1::span::SpanKind::Server as i32;
    let internal_kind = opentelemetry_proto::tonic::trace::v1::span::SpanKind::Internal as i32;
    let mut flows = HashMap::<String, Vec<CompleteAdmissionTrace>>::new();
    for client in spans.iter().filter(|span| {
        span.kind == client_kind
            && !span.parent_span_id.is_empty()
            && otlp_span_succeeded(span)
            && otlp_string_attribute(&span.attributes, "uc.domain") == Some("space_admission")
            && otlp_string_attribute(&span.attributes, "uc.operation") == Some("network_transport")
            && otlp_string_attribute(&span.attributes, "uc.role") == Some("joiner")
    }) {
        let Some(flow_id) = otlp_string_attribute(&client.attributes, "uc.flow.id") else {
            continue;
        };
        let Some(server) = spans.iter().find(|span| {
            span.trace_id == client.trace_id
                && span.parent_span_id == client.span_id
                && span.kind == server_kind
                && otlp_string_attribute(&span.attributes, "uc.domain") == Some("space_admission")
                && otlp_string_attribute(&span.attributes, "uc.operation")
                    == Some("network_transport")
                && otlp_string_attribute(&span.attributes, "uc.role") == Some("sponsor")
                && otlp_string_attribute(&span.attributes, "uc.flow.id").is_none()
                && otlp_span_succeeded(span)
        }) else {
            continue;
        };
        let endpoint_exists = spans.iter().any(|span| {
            span.trace_id == server.trace_id
                && span.parent_span_id == server.span_id
                && span.kind == internal_kind
                && otlp_string_attribute(&span.attributes, "uc.operation")
                    == Some("space_admission")
                && otlp_string_attribute(&span.attributes, "uc.role") == Some("sponsor")
                && otlp_string_attribute(&span.attributes, "uc.flow.id").is_none()
                && otlp_span_succeeded(span)
        });
        if endpoint_exists {
            let traces = flows.entry(flow_id.to_owned()).or_default();
            if !traces.iter().any(|trace| trace.trace_id == client.trace_id) {
                traces.push(CompleteAdmissionTrace {
                    trace_id: client.trace_id.clone(),
                    started_at_ns: client.start_time_unix_nano,
                });
            }
        }
    }
    flows
}

fn otlp_span_succeeded(span: &opentelemetry_proto::tonic::trace::v1::Span) -> bool {
    span.status.as_ref().is_some_and(|status| {
        status.code == opentelemetry_proto::tonic::trace::v1::status::StatusCode::Ok as i32
    })
}
