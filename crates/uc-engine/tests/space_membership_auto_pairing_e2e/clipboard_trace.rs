//! 剪贴板复制、手动发送与写入失败的业务 trace 验收。

use super::*;

struct TraceTestClipboard {
    snapshot: Arc<Mutex<HostClipboardSnapshot>>,
    changes: Option<tokio::sync::mpsc::UnboundedReceiver<()>>,
    fail_write: bool,
    write_delay: Duration,
    write_finished: Arc<std::sync::atomic::AtomicBool>,
}

impl HostClipboard for TraceTestClipboard {
    fn read(&self) -> Result<HostClipboardSnapshot, HostCapabilityError> {
        Ok(self.snapshot.lock().expect("test clipboard").clone())
    }

    fn write(&self, snapshot: HostClipboardSnapshot) -> Result<(), HostCapabilityError> {
        std::thread::sleep(self.write_delay);
        self.write_finished
            .store(true, std::sync::atomic::Ordering::SeqCst);
        if self.fail_write {
            return Err(HostCapabilityError::new(
                HostCapabilityErrorCategory::Unavailable,
                "PRIVATE_CLIPBOARD_WRITE_FAILURE",
            ));
        }
        *self.snapshot.lock().expect("test clipboard") = snapshot;
        Ok(())
    }

    fn take_change_stream(
        &mut self,
    ) -> Result<Option<Box<dyn uc_engine::HostClipboardChangeStream>>, HostCapabilityError> {
        Ok(self
            .changes
            .take()
            .map(|receiver| Box::new(TraceTestClipboardChanges(receiver)) as Box<_>))
    }
}

struct TraceTestClipboardChanges(tokio::sync::mpsc::UnboundedReceiver<()>);

#[async_trait::async_trait]
impl uc_engine::HostClipboardChangeStream for TraceTestClipboardChanges {
    async fn next(&mut self) -> Result<uc_engine::HostClipboardChange, HostCapabilityError> {
        Ok(if self.0.recv().await.is_some() {
            uc_engine::HostClipboardChange::Changed
        } else {
            uc_engine::HostClipboardChange::Closed
        })
    }

    async fn shutdown(&mut self) -> Result<(), HostCapabilityError> {
        self.0.close();
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "真实剪贴板链路验收，独占进程观测，精确运行"]
async fn copied_clipboard_is_saved_and_written_remotely_in_one_trace() {
    let _scenario = TestScenario::start();
    clipboard_trace_case(false, Duration::ZERO, false, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "真实手动发送验收，独占进程观测，精确运行"]
async fn explicit_send_is_one_distinct_business_trace() {
    let _scenario = TestScenario::start();
    clipboard_trace_case(false, Duration::ZERO, true, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "真实双设备写入失败验收，独占进程观测，精确运行"]
async fn clipboard_write_failure_keeps_saved_receipt_and_linked_error() {
    let _scenario = TestScenario::start();
    clipboard_trace_case(true, Duration::ZERO, false, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "真实双设备慢写入验收，独占进程观测，精确运行"]
async fn slow_clipboard_write_does_not_extend_network_receive() {
    let _scenario = TestScenario::start();
    clipboard_trace_case(false, Duration::from_millis(1500), false, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "真实三设备部分完成验收，独占进程观测，精确运行"]
async fn multi_target_copy_reports_partial_delivery_in_one_trace() {
    let _scenario = TestScenario::start();
    clipboard_trace_case(false, Duration::ZERO, false, true).await;
}

async fn clipboard_trace_case(
    fail_write: bool,
    write_delay: Duration,
    explicit: bool,
    partial_target: bool,
) {
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
        &format!("{}/v1/logs", telemetry.uri())
    ));
    let rendezvous = mount_rendezvous().await;
    let source_harness = DeviceHarness::new(rendezvous.uri());
    let target_harness = DeviceHarness::new(rendezvous.uri());
    let empty = || {
        Arc::new(Mutex::new(HostClipboardSnapshot {
            observed_at_ms: 0,
            representations: vec![],
        }))
    };
    let source_clipboard = empty();
    let target_clipboard = empty();
    let write_finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (change_tx, change_rx) = tokio::sync::mpsc::unbounded_channel();
    let source = source_harness
        .start_with_clipboard(Box::new(TraceTestClipboard {
            snapshot: source_clipboard.clone(),
            changes: Some(change_rx),
            fail_write: false,
            write_delay: Duration::ZERO,
            write_finished: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }))
        .await;
    let target = target_harness
        .start_with_clipboard(Box::new(TraceTestClipboard {
            snapshot: target_clipboard.clone(),
            changes: None,
            fail_write,
            write_delay,
            write_finished: write_finished.clone(),
        }))
        .await;
    let space_id = create_space(&source, "Clipboard Source").await.0;
    let target_id = join_through(&source, &target, "Clipboard Target", &space_id)
        .await
        .self_device_id;
    let offline_harness = partial_target.then(|| DeviceHarness::new(rendezvous.uri()));
    if let Some(harness) = &offline_harness {
        let offline = harness.start().await;
        join_through(&source, &offline, "Offline Target", &space_id).await;
        for engine in [&source, &target, &offline] {
            wait_for_active_member_count(engine, 3).await;
        }
        let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
        loop {
            let result = source
                .execute(Operation::QueryPeerConnections)
                .await
                .expect("peer connections");
            if let OperationResult::PeerConnections(peers) = result {
                if peers
                    .iter()
                    .any(|peer| peer.peer_id == target_id && peer.is_paired)
                {
                    break;
                }
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "online target must be ready before partial delivery"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        offline
            .shutdown(SHUTDOWN_TIMEOUT)
            .await
            .expect("stop one target");
    }
    wait_for_peer_refresh(&source, "source ready").await;
    wait_for_peer_refresh(&target, "target ready").await;
    let text = "private clipboard trace acceptance";
    *source_clipboard.lock().expect("source clipboard") = HostClipboardSnapshot {
        observed_at_ms: (unix_time_ns(SystemTime::now()) / 1_000_000) as i64,
        representations: vec![uc_engine::HostClipboardRepresentation::Inline {
            format: "text".to_owned(),
            mime_type: Some("text/plain".to_owned()),
            bytes: text.as_bytes().to_vec(),
        }],
    };
    let resend_entry = if explicit {
        let result = source
            .execute(Operation::SendText(SendTextInput {
                text: text.to_owned(),
                target_devices: vec![],
            }))
            .await
            .expect("send text");
        let OperationResult::EntrySent(report) = result else {
            panic!("send result");
        };
        Some(report.entry_id)
    } else {
        change_tx.send(()).expect("copy notification");
        None
    };
    wait_for_received_text(&target, text).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if fail_write && write_finished.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }
        if target_clipboard.lock().expect("target clipboard").representations.iter().any(|representation| matches!(representation, uc_engine::HostClipboardRepresentation::Inline { bytes, .. } if bytes == text.as_bytes())) { break; }
        assert!(
            tokio::time::Instant::now() < deadline,
            "saved clipboard was not written to the target system clipboard"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    if let Some(entry_id) = resend_entry {
        let result = source
            .execute(Operation::ResendEntry(uc_engine::ResendEntryInput {
                entry_id,
                target_devices: vec![target_id],
            }))
            .await
            .expect("resend");
        let OperationResult::EntryResent(uc_engine::ResendEntryOutcome::Completed(report)) = result
        else {
            panic!("resend result");
        };
        assert!(
            report.accepted != 0 || report.duplicate != 0,
            "explicit resend must reach the target"
        );
    }
    source
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("stop source");
    target
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("stop target");
    let restart_started = unix_time_ns(SystemTime::now());
    let restarted = source_harness.start().await;
    restarted
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("stop restarted source");
    uc_engine::flush_test_tracing();
    let requests = telemetry.received_requests().await.expect("telemetry");
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
    let root = spans
        .iter()
        .find(|span| {
            span.name
                == if explicit {
                    "clipboard.send"
                } else {
                    "clipboard.copy_and_sync"
                }
        })
        .expect("copy must create a complete clipboard trace");
    assert_eq!(
        otlp_string_attribute(&root.attributes, "uc.record.kind"),
        Some("business")
    );
    assert!(
        !spans.iter().any(|span| span.name == "session_lifecycle"),
        "runtime actions must be named explicitly"
    );
    assert!(
        !spans
            .iter()
            .any(|span| span.name == "runtime.shutdown_tasks"
                && otlp_string_attribute(&span.attributes, "uc.outcome") == Some("ok")),
        "normal cleanup must not create separate trace entries"
    );
    for name in [
        "clipboard_dispatch",
        "clipboard_receive",
        "clipboard.persist",
        "clipboard.write_system",
    ] {
        assert!(
            spans
                .iter()
                .any(|span| span.trace_id == root.trace_id && span.name == name),
            "missing linked clipboard action: {name}"
        );
    }
    assert!(root.parent_span_id.is_empty());
    assert_eq!(
        otlp_string_attribute(&root.attributes, "uc.outcome"),
        Some(if partial_target { "partial" } else { "ok" })
    );
    if explicit {
        assert!(!spans
            .iter()
            .any(|span| span.name == "clipboard.copy_and_sync"));
        let resend = spans
            .iter()
            .find(|span| span.name == "clipboard.resend")
            .expect("resend root");
        assert_ne!(resend.trace_id, root.trace_id);
        assert!(resend.parent_span_id.is_empty());
        assert_eq!(
            otlp_string_attribute(&resend.attributes, "uc.outcome"),
            Some("ok")
        );
        assert!(spans.iter().any(|span| span.name == "clipboard_dispatch"
            && span.trace_id == resend.trace_id
            && span.parent_span_id == resend.span_id));
    }
    let receive = spans
        .iter()
        .find(|span| span.trace_id == root.trace_id && span.name == "clipboard_receive")
        .expect("receive");
    let dispatch = spans
        .iter()
        .find(|span| span.trace_id == root.trace_id && span.span_id == receive.parent_span_id)
        .expect("online target dispatch");
    assert_eq!(dispatch.parent_span_id, root.span_id);
    assert_eq!(receive.parent_span_id, dispatch.span_id);
    for name in ["clipboard.persist", "clipboard.write_system"] {
        assert!(
            spans.iter().any(|span| span.trace_id == root.trace_id
                && span.name == name
                && span.parent_span_id == receive.span_id),
            "target action must belong to receive: {name}"
        );
    }
    assert!(
        spans.iter().any(|span| span.trace_id == root.trace_id
            && span.name == "clipboard.persist"
            && span.parent_span_id == root.span_id),
        "source persistence must belong to copy"
    );
    let logs = requests
        .iter()
        .filter(|request| request.url.path() == "/v1/logs")
        .flat_map(|request| {
            ExportLogsServiceRequest::decode(request.body.as_slice())
                .expect("logs")
                .resource_logs
        })
        .flat_map(|resource| resource.scope_logs)
        .flat_map(|scope| scope.log_records)
        .collect::<Vec<_>>();
    for span in spans.iter().filter(|span| {
        span.name == "runtime.transition_session" || span.name == "runtime.recover_session"
    }) {
        let log = logs
            .iter()
            .find(|log| {
                log.trace_id == span.trace_id
                    && log.span_id == span.span_id
                    && otlp_string_attribute(&log.attributes, "uc.operation")
                        == otlp_string_attribute(&span.attributes, "uc.operation")
                    && otlp_string_attribute(&log.attributes, "event.name")
                        == Some("uc.operation.completed")
            })
            .expect("runtime completion");
        let elapsed_ms = log
            .attributes
            .iter()
            .find_map(|field| {
                if field.key == "duration_ms" {
                    match field.value.as_ref()?.value.as_ref()? {
                        OtlpValue::IntValue(value) => Some(*value as u64),
                        _ => None,
                    }
                } else {
                    None
                }
            })
            .expect("elapsed");
        let span_ms = (span.end_time_unix_nano - span.start_time_unix_nano) / 1_000_000;
        assert!(span_ms.abs_diff(elapsed_ms) < 100, "runtime background tasks extended a completed operation: span={span_ms}ms completion={elapsed_ms}ms");
    }
    let upgrades = spans
        .iter()
        .filter(|span| {
            otlp_string_attribute(&span.attributes, "uc.operation")
                == Some("profile_storage_upgrade")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        upgrades.len(),
        if partial_target { 3 } else { 2 },
        "only initial preparation, not an up-to-date restart, is a storage operation"
    );
    for span in upgrades {
        assert!(span.start_time_unix_nano < restart_started);
        assert_eq!(span.name, "storage.initialize_profile");
        assert_eq!(
            otlp_string_attribute(&span.attributes, "uc.outcome"),
            Some("ok")
        );
        assert_eq!(
            logs.iter()
                .filter(|log| log.trace_id == span.trace_id && log.span_id == span.span_id)
                .count(),
            1
        );
    }
    let checks = logs
        .iter()
        .filter(|log| {
            otlp_string_attribute(&log.attributes, "uc.operation")
                == Some("profile_storage_upgrade")
                && otlp_string_attribute(&log.attributes, "uc.outcome") == Some("skipped")
        })
        .collect::<Vec<_>>();
    assert!(
        !checks.is_empty(),
        "up-to-date check remains a diagnostic log"
    );
    assert!(
        checks
            .iter()
            .all(|log| log.trace_id.is_empty() && log.span_id.is_empty()),
        "do not leave logs referring to an intentionally omitted operation"
    );
    let write = spans
        .iter()
        .find(|span| span.trace_id == root.trace_id && span.name == "clipboard.write_system")
        .expect("system write span");
    let write_log = logs
        .iter()
        .find(|log| log.trace_id == write.trace_id && log.span_id == write.span_id)
        .expect("system write result");
    assert_eq!(
        otlp_string_attribute(&write_log.attributes, "uc.outcome"),
        Some(if fail_write { "error" } else { "ok" })
    );
    if fail_write {
        assert!(target_clipboard
            .lock()
            .expect("target clipboard")
            .representations
            .is_empty());
        assert_eq!(
            otlp_string_attribute(&write_log.attributes, "error.type"),
            Some("unavailable")
        );
    }
    let receive_log = logs
        .iter()
        .find(|log| log.trace_id == receive.trace_id && log.span_id == receive.span_id)
        .expect("receive result");
    assert_eq!(
        otlp_string_attribute(&receive_log.attributes, "uc.outcome"),
        Some("ok"),
        "system write failure must not rewrite saved receipt"
    );
    if !write_delay.is_zero() {
        assert!(
            write.end_time_unix_nano > receive.end_time_unix_nano + 500_000_000,
            "slow OS write must be a late child, not extend receive"
        );
        assert!(
            write.end_time_unix_nano > dispatch.end_time_unix_nano + 500_000_000,
            "sender must receive acknowledgement before OS write finishes"
        );
    }
    for span in spans.iter().filter(|span| span.trace_id == root.trace_id) {
        assert_eq!(
            logs.iter()
                .filter(|log| log.trace_id == span.trace_id
                    && log.span_id == span.span_id
                    && otlp_string_attribute(&log.attributes, "event.name")
                        == Some("uc.operation.completed"))
                .count(),
            1,
            "one completion per action: {}",
            span.name
        );
    }
    for request in &requests {
        assert!(!request
            .body
            .windows("PRIVATE_CLIPBOARD_WRITE_FAILURE".len())
            .any(|bytes| bytes == b"PRIVATE_CLIPBOARD_WRITE_FAILURE"));
        assert!(
            !request
                .body
                .windows(text.len())
                .any(|bytes| bytes == text.as_bytes()),
            "clipboard content leaked to telemetry"
        );
    }
    let membership = spans
        .iter()
        .filter(|span| {
            otlp_string_attribute(&span.attributes, "uc.operation")
                == Some("membership_history_sync")
        })
        .collect::<Vec<_>>();
    assert!(
        !membership.is_empty(),
        "two devices must exercise membership exchange"
    );
    for span in &membership {
        if span.name == "membership.compare_summary.exchange" {
            assert!(
                spans.iter().any(|parent| parent.trace_id == span.trace_id
                    && parent.span_id == span.parent_span_id
                    && parent.name.starts_with("membership.recover.")),
                "membership exchange must belong to a recovery intent"
            );
        }
        if span.name.ends_with(".handle_and_reply") {
            assert!(
                membership
                    .iter()
                    .any(|parent| parent.trace_id == span.trace_id
                        && parent.span_id == span.parent_span_id
                        && parent.name.ends_with(".exchange")),
                "member reply must retain its initiating exchange"
            );
        }
        assert!(
            span.name.starts_with("membership."),
            "membership purpose must be visible: {}",
            span.name
        );
        assert!(
            otlp_string_attribute(&span.attributes, "uc.outcome").is_some(),
            "result must be visible on the trace page"
        );
        if otlp_string_attribute(&span.attributes, "uc.outcome") == Some("error") {
            let error = otlp_string_attribute(&span.attributes, "error.type")
                .expect("failed span must explain why");
            let log = logs
                .iter()
                .find(|log| log.trace_id == span.trace_id && log.span_id == span.span_id)
                .expect("failure log");
            assert_eq!(
                otlp_string_attribute(&log.attributes, "error.type"),
                Some(error)
            );
        }
    }
    // 仅显式验收时将已通过隐私断言的合成设备记录送到本机可视化环境。
    if std::env::var_os("UC_CLIPBOARD_TRACE_JAEGER").is_some() {
        for span in membership
            .iter()
            .filter(|span| span.parent_span_id.is_empty())
        {
            println!(
                "membership trace: {} {} {:?}",
                span.trace_id
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>(),
                span.name,
                otlp_string_attribute(&span.attributes, "error.type")
            );
        }
        for request in &requests {
            use std::io::Write;
            use std::process::{Command, Stdio};
            let mut child = Command::new("curl")
                .args([
                    "--fail",
                    "--silent",
                    "--show-error",
                    "--max-time",
                    "10",
                    "-H",
                    "Content-Type: application/x-protobuf",
                    "--data-binary",
                    "@-",
                ])
                .arg(format!("http://127.0.0.1:4318{}", request.url.path()))
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .spawn()
                .expect("local collector client");
            child
                .stdin
                .take()
                .expect("client input")
                .write_all(&request.body)
                .expect("send synthetic telemetry");
            assert!(child.wait().expect("collector reply").success());
        }
        println!(
            "clipboard trace: {}",
            root.trace_id
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
    }
}
