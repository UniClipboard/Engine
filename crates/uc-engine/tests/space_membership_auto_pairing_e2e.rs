#![cfg(feature = "dev-tools")]
#![cfg(not(coverage))]

#[path = "space_membership_auto_pairing_e2e/six_digit_pairing.rs"]
mod six_digit_pairing;

#[path = "space_membership_auto_pairing_e2e/automatic_connections.rs"]
mod automatic_connections;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value as OtlpValue;
use prost::Message;
use tempfile::TempDir;
use uc_engine::{
    ChooseDeviceGroupInput, CreateSpaceInput, Engine, EngineConfig, HistoryEntryInput,
    HostCapabilities, HostCapabilityError, HostCapabilityErrorCategory, HostClipboard,
    HostClipboardSnapshot, HostDirectories, HostFileAccess, HostFileHandle, HostFileMetadata,
    HostSecureStorage, JoinSpaceInput, JoinSpaceStatusSummary, ListHistoryEntriesInput, Operation,
    OperationResult, RemoveMemberInput, SecretString, SendTextInput,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const PASSPHRASE: &str = "space-membership-e2e-passphrase";
const WAIT_TIMEOUT: Duration = Duration::from_secs(60);
const ADMISSION_WAIT_TIMEOUT: Duration = Duration::from_secs(120);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(15);
const EXPIRES_AT_MS: i64 = 2_000_000_000_000;
const PAIRING_HOT_PATH_BUDGET: Duration = Duration::from_secs(1);

#[derive(Clone, Default)]
struct MemorySecureStorage(Arc<Mutex<HashMap<String, Vec<u8>>>>);

impl MemorySecureStorage {
    fn values(&self) -> MutexGuard<'_, HashMap<String, Vec<u8>>> {
        match self.0.lock() {
            Ok(values) => values,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

impl HostSecureStorage for MemorySecureStorage {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, HostCapabilityError> {
        Ok(self.values().get(key).cloned())
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<(), HostCapabilityError> {
        self.values().insert(key.to_owned(), value.to_vec());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), HostCapabilityError> {
        self.values().remove(key);
        Ok(())
    }
}

struct EmptyClipboard;

impl HostClipboard for EmptyClipboard {
    fn read(&self) -> Result<HostClipboardSnapshot, HostCapabilityError> {
        Ok(HostClipboardSnapshot {
            observed_at_ms: 0,
            representations: Vec::new(),
        })
    }

    fn write(&self, _snapshot: HostClipboardSnapshot) -> Result<(), HostCapabilityError> {
        Ok(())
    }
}

struct EmptyFiles;

impl HostFileAccess for EmptyFiles {
    fn metadata(&self, _handle: &HostFileHandle) -> Result<HostFileMetadata, HostCapabilityError> {
        Err(HostCapabilityError::new(
            HostCapabilityErrorCategory::InvalidHandle,
            "missing test file",
        ))
    }

    fn read_chunk(
        &self,
        _handle: &HostFileHandle,
        _offset: u64,
        _max_bytes: u32,
    ) -> Result<Vec<u8>, HostCapabilityError> {
        Ok(Vec::new())
    }

    fn write_chunk(
        &self,
        _handle: &HostFileHandle,
        _offset: u64,
        _bytes: &[u8],
    ) -> Result<(), HostCapabilityError> {
        Ok(())
    }

    fn finish_write(&self, _handle: &HostFileHandle) -> Result<(), HostCapabilityError> {
        Ok(())
    }
}

struct DeviceHarness {
    root: TempDir,
    secure_storage: MemorySecureStorage,
    rendezvous_base_url: String,
}

impl DeviceHarness {
    fn new(rendezvous_base_url: String) -> Self {
        Self {
            root: TempDir::new().expect("create device directory"),
            secure_storage: MemorySecureStorage::default(),
            rendezvous_base_url,
        }
    }

    async fn start(&self) -> Engine {
        self.start_with_clipboard(Box::new(EmptyClipboard)).await
    }

    async fn start_with_clipboard(&self, clipboard: Box<dyn HostClipboard>) -> Engine {
        self.start_configured(clipboard, true).await
    }

    async fn start_with_relay_fallback(&self, relay_fallback: bool) -> Engine {
        self.start_configured(Box::new(EmptyClipboard), relay_fallback)
            .await
    }

    async fn start_configured(
        &self,
        clipboard: Box<dyn HostClipboard>,
        relay_fallback: bool,
    ) -> Engine {
        let root = self.root.path();
        let host = HostCapabilities::new(
            HostDirectories::new(
                root.join("private"),
                root.join("cache"),
                root.join("temporary"),
                root.join("logs"),
            ),
            Box::new(self.secure_storage.clone()),
            clipboard,
            Box::new(EmptyFiles),
        );
        let config = EngineConfig::new("1.1.0")
            .with_rendezvous_base_url(self.rendezvous_base_url.clone())
            .with_test_relay_fallback(relay_fallback);
        let (engine, _events) = Engine::start(config, host)
            .await
            .expect("start complete engine");
        engine
    }
}

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
    clipboard_trace_case(false, Duration::ZERO, false, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "真实手动发送验收，独占进程观测，精确运行"]
async fn explicit_send_is_one_distinct_business_trace() {
    clipboard_trace_case(false, Duration::ZERO, true, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "真实双设备写入失败验收，独占进程观测，精确运行"]
async fn clipboard_write_failure_keeps_saved_receipt_and_linked_error() {
    clipboard_trace_case(true, Duration::ZERO, false, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "真实双设备慢写入验收，独占进程观测，精确运行"]
async fn slow_clipboard_write_does_not_extend_network_receive() {
    clipboard_trace_case(false, Duration::from_millis(1500), false, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "真实三设备部分完成验收，独占进程观测，精确运行"]
async fn multi_target_copy_reports_partial_delivery_in_one_trace() {
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

#[derive(Default)]
struct TicketState {
    next_code: u16,
    tickets: HashMap<String, String>,
}

type TicketVault = Arc<Mutex<TicketState>>;

struct CreatePairing(TicketVault);

impl Respond for CreatePairing {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: serde_json::Value =
            serde_json::from_slice(&request.body).expect("pairing create request must be JSON");
        let ticket = body["sponsorTicket"]
            .as_str()
            .expect("sponsor ticket missing")
            .to_owned();
        assert!(
            ticket.starts_with("ucspace1_"),
            "directory must store a full admission invitation"
        );
        let mut state = lock_ticket_vault(&self.0);
        state.next_code += 1;
        let code = ticket.clone();
        state.tickets.insert(code.clone(), ticket);
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": code,
            "expiresAtMs": EXPIRES_AT_MS,
        }))
    }
}

struct ResolvePairing(TicketVault);

impl Respond for ResolvePairing {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: serde_json::Value =
            serde_json::from_slice(&request.body).expect("pairing resolve request must be JSON");
        let code = body["code"].as_str().expect("pairing code missing");
        let state = lock_ticket_vault(&self.0);
        let ticket = state
            .tickets
            .get(code)
            .or_else(|| state.tickets.values().next())
            .cloned()
            .expect("pairing ticket was not registered");
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "sponsorTicket": ticket,
            "sponsorEndpointId": "local-e2e",
            "expiresAtMs": EXPIRES_AT_MS,
        }))
    }
}

fn lock_ticket_vault(vault: &TicketVault) -> MutexGuard<'_, TicketState> {
    match vault.lock() {
        Ok(state) => state,
        Err(poisoned) => poisoned.into_inner(),
    }
}

async fn mount_rendezvous() -> MockServer {
    let server = MockServer::start().await;
    let vault = Arc::new(Mutex::new(TicketState::default()));
    Mock::given(method("POST"))
        .and(path("/v1/pairings"))
        .respond_with(CreatePairing(Arc::clone(&vault)))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/pairings/resolve"))
        .respond_with(ResolvePairing(vault))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/pairings/consume"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    server
}

#[derive(Debug, Clone, Copy)]
enum TopologyAction<'a> {
    Start {
        node: &'a str,
    },
    Create {
        node: &'a str,
    },
    Join {
        sponsor: &'a str,
        joiner: &'a str,
    },
    Remove {
        sponsor: &'a str,
        target: &'a str,
    },
    Restart {
        node: &'a str,
    },
    Stop {
        node: &'a str,
    },
    Decide {
        node: &'a str,
        choice: PendingChangeChoice,
    },
    AssertSnapshot {
        node: &'a str,
        active_members: usize,
        pending_choices: usize,
    },
    AssertDiagnostics {
        node: &'a str,
        effective_members: u32,
        pending_conflicts: u32,
        pending_effects: u32,
    },
    Partition {
        left: &'a [&'a str],
        right: &'a [&'a str],
    },
    PartitionGroups {
        groups: &'a [&'a [&'a str]],
    },
    GroupedBridge {
        groups: &'a [&'a [&'a str]],
        left: &'a str,
        right: &'a str,
    },
    Bridge {
        left: &'a str,
        right: &'a str,
        left_group: &'a [&'a str],
        right_group: &'a [&'a str],
    },
    Ring {
        nodes: &'a [&'a str],
        isolated: &'a [&'a str],
    },
    Chain {
        nodes: &'a [&'a str],
        offline: &'a [&'a str],
    },
    Heal {
        nodes: &'a [&'a str],
    },
    ResolveConflict {
        node: &'a str,
        branch_from: &'a str,
    },
}

#[derive(Debug, Clone, Copy)]
enum PendingChangeChoice {
    Apply,
    Keep,
}

// 三台设备依次加入、移除、重新加入后发生交叉移除时，至少一台设备必须报告设备组分歧。
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn cross_removals_after_rejoin_must_keep_divergence_visible() {
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let mut topology = MembershipTopology::new(rendezvous.uri());
    topology
        .run(&[
            TopologyAction::Start { node: "A" },
            TopologyAction::Start { node: "B" },
            TopologyAction::Start { node: "C" },
            TopologyAction::Create { node: "A" },
            TopologyAction::Create { node: "B" },
            TopologyAction::Create { node: "C" },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "B",
            },
            TopologyAction::Join {
                sponsor: "B",
                joiner: "C",
            },
        ])
        .await;
    topology
        .wait_for_equivalent_branch_named(&["A", "B", "C"], 3, "initial A-B-C group")
        .await;

    topology
        .run(&[TopologyAction::Remove {
            sponsor: "C",
            target: "A",
        }])
        .await;
    topology.wait_for_pending_change(&["B"]).await;
    topology
        .run(&[TopologyAction::Decide {
            node: "B",
            choice: PendingChangeChoice::Apply,
        }])
        .await;
    topology.wait_for_pending_change(&["A"]).await;
    topology.apply_local_removal_with_confirmation("A").await;
    topology
        .wait_for_equivalent_branch_named(&["B", "C"], 2, "A removed from the first group")
        .await;

    topology
        .run(&[TopologyAction::Join {
            sponsor: "B",
            joiner: "A",
        }])
        .await;
    wait_for_active_member_count(topology.engine("A"), 3).await;

    topology
        .run(&[TopologyAction::Remove {
            sponsor: "C",
            target: "B",
        }])
        .await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    topology
        .run(&[TopologyAction::Remove {
            sponsor: "A",
            target: "C",
        }])
        .await;
    topology
        .wait_for_stable_pending_effects(&["A", "B", "C"])
        .await;

    let final_diagnostics = [
        topology.diagnostics("A").await,
        topology.diagnostics("B").await,
        topology.diagnostics("C").await,
    ];
    let effective_members = final_diagnostics
        .each_ref()
        .map(|diagnostics| diagnostics.effective_member_count);
    let pending_conflicts = final_diagnostics
        .each_ref()
        .map(|diagnostics| diagnostics.pending_conflict_count);
    let pending_confirmations = final_diagnostics
        .each_ref()
        .map(|diagnostics| diagnostics.pending_confirmation_count);
    assert!(
        pending_conflicts
            .iter()
            .chain(pending_confirmations.iter())
            .any(|count| *count > 0),
        "cross-removal split must remain visible instead of reporting every device group as healthy; effective members: {effective_members:?}; pending conflicts: {pending_conflicts:?}; pending confirmations: {pending_confirmations:?}"
    );

    topology.shutdown().await;
}

// F7：十节点不平衡树形成三个 sibling 后，冲突 peer 不得饿死合法 peer 的反熵。
#[tokio::test(flavor = "multi_thread", worker_threads = 16)]
async fn f7_three_sibling_branches_keep_fair_anti_entropy_for_legal_peers() {
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let mut topology = MembershipTopology::new(rendezvous.uri());
    topology
        .run(&[
            TopologyAction::Start { node: "A" },
            TopologyAction::Start { node: "B" },
            TopologyAction::Start { node: "C" },
            TopologyAction::Start { node: "D" },
            TopologyAction::Start { node: "E" },
            TopologyAction::Start { node: "F" },
            TopologyAction::Start { node: "G" },
            TopologyAction::Start { node: "H" },
            TopologyAction::Start { node: "I" },
            TopologyAction::Start { node: "J" },
            TopologyAction::Create { node: "A" },
        ])
        .await;
    let mut baseline = vec!["A"];
    for (sponsor, joiner) in [
        ("A", "B"),
        ("B", "C"),
        ("C", "D"),
        ("D", "E"),
        ("E", "F"),
        ("F", "G"),
    ] {
        topology.join(sponsor, joiner).await;
        baseline.push(joiner);
        topology
            .wait_for_equivalent_branch_named(&baseline, baseline.len() as u32, "F7 baseline")
            .await;
        let epoch = topology.diagnostics("A").await.group_epoch;
        topology
            .wait_for_group_epoch_named(&baseline, epoch, "F7 baseline")
            .await;
    }
    let invitation_h = issue_invitation_named(topology.engine("A"), "A").await;
    let invitation_i = issue_invitation_named(topology.engine("B"), "B").await;
    let invitation_j = issue_invitation_named(topology.engine("C"), "C").await;

    topology
        .run(&[TopologyAction::PartitionGroups {
            groups: &[
                &["A", "G", "H"][..],
                &["D"][..],
                &["B", "E", "I"][..],
                &["C", "F", "J"][..],
            ],
        }])
        .await;
    let space_id = topology
        .space_ids
        .get("A")
        .unwrap_or_else(|| panic!("node A has no space"))
        .clone();
    let (joined_h, joined_i, joined_j) = tokio::join!(
        join_with_invitation(topology.engine("H"), "H", &space_id, invitation_h,),
        join_with_invitation(topology.engine("I"), "I", &space_id, invitation_i,),
        join_with_invitation(topology.engine("J"), "J", &space_id, invitation_j,),
    );
    for (node, joined) in [("H", joined_h), ("I", joined_i), ("J", joined_j)] {
        topology.space_ids.insert(node.to_owned(), space_id.clone());
        topology
            .device_ids
            .insert(node.to_owned(), joined.self_device_id);
    }
    for (nodes, phase) in [
        (&["A", "G", "H"][..], "F7 branch H"),
        (&["B", "E", "I"][..], "F7 branch I"),
        (&["C", "F", "J"][..], "F7 branch J"),
    ] {
        topology
            .wait_for_equivalent_branch_named(nodes, 8, phase)
            .await;
        let epoch = topology.diagnostics(nodes[0]).await.group_epoch;
        topology
            .wait_for_group_epoch_named(nodes, epoch, phase)
            .await;
    }
    let target = topology.diagnostics("A").await;
    assert_eq!(topology.diagnostics("D").await.effective_member_count, 7);

    topology
        .run(&[TopologyAction::GroupedBridge {
            groups: &[
                &["A", "D", "G", "H"][..],
                &["B", "E", "I"][..],
                &["C", "F", "J"][..],
            ],
            left: "A",
            right: "B",
        }])
        .await;
    topology.wait_for_branch_conflict(&["A", "B"]).await;
    topology
        .wait_for_equivalent_branch_named(&["A", "D", "G", "H"], 8, "F7 fair legal peer")
        .await;
    topology
        .wait_for_group_epoch_named(
            &["A", "D", "G", "H"],
            target.group_epoch,
            "F7 fair legal peer",
        )
        .await;
    assert_eq!(topology.diagnostics("D").await.pending_conflict_count, 0);

    let branches: [&[&str]; 3] = [&["A", "D", "G", "H"], &["B", "E", "I"], &["C", "F", "J"]];
    let all_nodes = ["A", "B", "C", "D", "E", "F", "G", "H", "I", "J"];
    for branch in branches {
        for sender in branch {
            for receiver in branch {
                if sender != receiver {
                    topology.wait_for_paired_peer(sender, receiver).await;
                }
            }
        }
    }
    for sender in all_nodes {
        for receiver in all_nodes {
            if sender == receiver {
                continue;
            }
            let same_branch = branches
                .iter()
                .any(|branch| branch.contains(&sender) && branch.contains(&receiver));
            let text = format!("F7 matrix {sender}-{receiver}");
            let report = topology.send(sender, receiver, &text).await;
            assert_eq!(
                report.total_accepted + report.total_pending,
                usize::from(same_branch),
                "unexpected F7 transfer result for {sender}-{receiver}: {report:?}"
            );
            if same_branch {
                wait_for_received_text(topology.engine(receiver), &text).await;
            } else {
                assert!(!receiver_has_exact_text(topology.engine(receiver), &text).await);
            }
        }
    }
    topology.shutdown().await;
}

// F6：深准入链的中间 Sponsor 离线后，叶子仍须经剩余相邻节点恢复到共同分支。
#[tokio::test(flavor = "multi_thread", worker_threads = 12)]
async fn f6_deep_chain_recovers_selected_branch_without_online_sponsors() {
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let mut topology = MembershipTopology::new(rendezvous.uri());
    topology
        .run(&[
            TopologyAction::Start { node: "A" },
            TopologyAction::Start { node: "B" },
            TopologyAction::Start { node: "C" },
            TopologyAction::Start { node: "D" },
            TopologyAction::Start { node: "E" },
            TopologyAction::Start { node: "F" },
            TopologyAction::Start { node: "G" },
            TopologyAction::Start { node: "H" },
            TopologyAction::Create { node: "A" },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "B",
            },
            TopologyAction::Join {
                sponsor: "B",
                joiner: "C",
            },
            TopologyAction::Join {
                sponsor: "C",
                joiner: "D",
            },
            TopologyAction::Join {
                sponsor: "D",
                joiner: "E",
            },
            TopologyAction::Join {
                sponsor: "E",
                joiner: "F",
            },
        ])
        .await;
    topology
        .wait_for_equivalent_branch_named(&["A", "B", "C", "D", "E", "F"], 6, "F6 common baseline")
        .await;
    let baseline_epoch = topology.diagnostics("A").await.group_epoch;
    topology
        .wait_for_group_epoch(&["B", "C", "D", "E", "F"], baseline_epoch)
        .await;
    let left_invitation = issue_invitation(topology.engine("A")).await;
    let right_invitation = issue_invitation(topology.engine("F")).await;

    topology
        .run(&[TopologyAction::Partition {
            left: &["A", "C", "E", "G"],
            right: &["B", "D", "F", "H"],
        }])
        .await;
    topology
        .join_with_invitation("A", "G", left_invitation)
        .await;
    topology
        .join_with_invitation("F", "H", right_invitation)
        .await;
    topology
        .wait_for_equivalent_branch_named(&["A", "C", "E", "G"], 7, "F6 selected left branch")
        .await;
    topology
        .wait_for_equivalent_branch_named(&["F", "H"], 7, "F6 sibling right branch")
        .await;
    let target_epoch = topology.diagnostics("A").await.group_epoch;
    topology
        .wait_for_group_epoch(&["C", "E", "G"], target_epoch)
        .await;
    let sibling_epoch = topology.diagnostics("F").await.group_epoch;
    topology
        .wait_for_group_epoch(&["B", "D", "H"], sibling_epoch)
        .await;
    let target = topology.diagnostics("E").await;

    topology
        .run(&[
            TopologyAction::Stop { node: "B" },
            TopologyAction::Stop { node: "D" },
            TopologyAction::Chain {
                nodes: &["A", "C", "E", "F"],
                offline: &["B", "D", "G", "H"],
            },
        ])
        .await;
    topology.wait_for_branch_conflict(&["E", "F"]).await;
    topology
        .run(&[TopologyAction::ResolveConflict {
            node: "F",
            branch_from: "E",
        }])
        .await;
    topology
        .wait_for_equivalent_branch_named(&["A", "C", "E", "F"], 7, "F6 recovered online chain")
        .await;
    topology
        .run(&[TopologyAction::Heal {
            nodes: &["A", "C", "E", "F"],
        }])
        .await;
    let recovered_epoch = topology.diagnostics("E").await.group_epoch;
    topology
        .wait_for_group_epoch_named(
            &["A", "C", "E", "F"],
            recovered_epoch,
            "F6 recovered online chain",
        )
        .await;
    for node in ["A", "C", "E", "F"] {
        wait_for_peer_refresh(topology.engine(node), node).await;
    }
    let recovered = topology.diagnostics("F").await;
    assert_eq!(recovered.branch_id, target.branch_id);
    assert_eq!(recovered.head_event_id, target.head_event_id);

    for (sender, receiver) in [("A", "C"), ("C", "E"), ("E", "F")] {
        topology.wait_for_paired_peer(sender, receiver).await;
        let text = format!("F6 converged hop {sender}-{receiver}");
        let report = topology.send(sender, receiver, &text).await;
        assert!(
            report.total_accepted > 0,
            "F6 converged hop {sender}-{receiver} was rejected: {report:?}"
        );
        assert!(receiver_has_exact_text(topology.engine(receiver), &text).await);
    }
    topology.shutdown().await;
}

// F5：同一 sibling conflict 沿四节点环的两个方向传播时，只能提示一次且不得形成消息环。
#[tokio::test(flavor = "multi_thread", worker_threads = 12)]
async fn f5_ring_propagates_one_conflict_without_message_or_effect_loops() {
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let mut topology = MembershipTopology::new(rendezvous.uri());
    topology
        .run(&[
            TopologyAction::Start { node: "A" },
            TopologyAction::Start { node: "B" },
            TopologyAction::Start { node: "C" },
            TopologyAction::Start { node: "D" },
            TopologyAction::Start { node: "E" },
            TopologyAction::Start { node: "F" },
            TopologyAction::Create { node: "A" },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "B",
            },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "C",
            },
        ])
        .await;
    topology
        .wait_for_equivalent_branch(&["A", "B", "C"], 3)
        .await;
    let epoch_three = topology.diagnostics("A").await.group_epoch;
    topology
        .wait_for_group_epoch(&["B", "C"], epoch_three)
        .await;
    topology
        .run(&[TopologyAction::Join {
            sponsor: "A",
            joiner: "D",
        }])
        .await;
    topology
        .wait_for_equivalent_branch(&["A", "B", "C", "D"], 4)
        .await;
    let epoch_four = topology.diagnostics("A").await.group_epoch;
    topology
        .wait_for_group_epoch(&["B", "C", "D"], epoch_four)
        .await;
    topology
        .run(&[TopologyAction::Partition {
            left: &["A", "B", "E"],
            right: &["C", "D", "F"],
        }])
        .await;
    topology
        .run(&[
            TopologyAction::Join {
                sponsor: "A",
                joiner: "E",
            },
            TopologyAction::Join {
                sponsor: "C",
                joiner: "F",
            },
        ])
        .await;
    topology
        .wait_for_equivalent_branch(&["A", "B", "E"], 5)
        .await;
    topology
        .wait_for_equivalent_branch(&["C", "D", "F"], 5)
        .await;
    let left_branch = topology.diagnostics("A").await.branch_id;
    let right_branch = topology.diagnostics("C").await.branch_id;
    assert_ne!(left_branch, right_branch);
    let effects_before_ring = topology
        .wait_for_stable_pending_effects(&["A", "B", "C", "D"])
        .await;

    topology
        .run(&[TopologyAction::Ring {
            nodes: &["A", "B", "C", "D"],
            isolated: &["E", "F"],
        }])
        .await;
    topology
        .wait_for_branch_conflict(&["A", "B", "C", "D"])
        .await;

    for (index, node) in ["A", "B", "C", "D"].into_iter().enumerate() {
        let choices = topology.device_group_choices(node).await;
        assert_eq!(
            choices
                .issues
                .iter()
                .filter(|issue| issue.issue_id.starts_with("c:"))
                .count(),
            1,
            "node {node} must expose one conflict prompt"
        );
        assert!(
            topology.diagnostics(node).await.pending_effect_count <= effects_before_ring[index],
            "node {node} must not enqueue an effect while propagating conflict evidence"
        );
    }
    assert_eq!(topology.diagnostics("A").await.branch_id, left_branch);
    assert_eq!(topology.diagnostics("B").await.branch_id, left_branch);
    assert_eq!(topology.diagnostics("C").await.branch_id, right_branch);
    assert_eq!(topology.diagnostics("D").await.branch_id, right_branch);

    let effects_before_refresh = topology
        .wait_for_stable_pending_effects(&["A", "B", "C", "D"])
        .await;
    for _ in 0..2 {
        for node in ["A", "B", "C", "D"] {
            wait_for_peer_refresh(topology.engine(node), node).await;
        }
    }
    topology
        .wait_for_branch_conflict(&["A", "B", "C", "D"])
        .await;
    let effects_after_refresh = topology
        .wait_for_stable_pending_effects(&["A", "B", "C", "D"])
        .await;
    assert_eq!(effects_after_refresh, effects_before_refresh);
    topology.shutdown().await;
}

// F4：两个三节点 sibling 分支只开放一条 bridge 后，不得被拼成六节点联合历史。
#[tokio::test(flavor = "multi_thread", worker_threads = 12)]
async fn f4_single_bridge_cannot_splice_sibling_histories_into_a_union() {
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let mut topology = MembershipTopology::new(rendezvous.uri());
    topology
        .run(&[
            TopologyAction::Start { node: "A" },
            TopologyAction::Start { node: "B" },
            TopologyAction::Start { node: "C" },
            TopologyAction::Start { node: "D" },
            TopologyAction::Start { node: "E" },
            TopologyAction::Start { node: "F" },
            TopologyAction::Create { node: "A" },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "B",
            },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "C",
            },
        ])
        .await;
    topology
        .wait_for_equivalent_branch(&["A", "B", "C"], 3)
        .await;
    let epoch_three = topology.diagnostics("A").await.group_epoch;
    topology
        .wait_for_group_epoch(&["B", "C"], epoch_three)
        .await;
    for (joiner, established) in [
        ("D", &["A", "B", "C", "D"][..]),
        ("E", &["A", "B", "C", "D", "E"][..]),
        ("F", &["A", "B", "C", "D", "E", "F"][..]),
    ] {
        topology
            .run(&[TopologyAction::Join {
                sponsor: "A",
                joiner,
            }])
            .await;
        topology
            .wait_for_equivalent_branch(established, established.len() as u32)
            .await;
        let epoch = topology.diagnostics("A").await.group_epoch;
        topology
            .wait_for_group_epoch(&established[1..], epoch)
            .await;
    }
    topology
        .wait_for_equivalent_branch(&["A", "B", "C", "D", "E", "F"], 6)
        .await;
    let baseline_epoch = topology.diagnostics("A").await.group_epoch;
    topology
        .wait_for_group_epoch(&["B", "C", "D", "E", "F"], baseline_epoch)
        .await;
    topology
        .run(&[TopologyAction::Partition {
            left: &["A", "B", "C"],
            right: &["D", "E", "F"],
        }])
        .await;

    for target in ["C", "E", "F"] {
        topology
            .run(&[TopologyAction::Remove {
                sponsor: "A",
                target,
            }])
            .await;
    }
    for target in ["B", "C", "F"] {
        topology
            .run(&[TopologyAction::Remove {
                sponsor: "D",
                target,
            }])
            .await;
    }

    topology.assert_snapshot("A", 3, 0).await;
    topology.assert_snapshot("D", 3, 0).await;
    let left_before = topology.diagnostics("A").await;
    let right_before = topology.diagnostics("D").await;
    assert_ne!(left_before.branch_id, right_before.branch_id);

    topology
        .run(&[TopologyAction::Bridge {
            left: "A",
            right: "D",
            left_group: &["A", "B", "C"],
            right_group: &["D", "E", "F"],
        }])
        .await;
    topology.wait_for_branch_conflict(&["A", "D"]).await;

    topology.assert_snapshot("A", 3, 1).await;
    topology.assert_snapshot("D", 3, 1).await;
    assert_eq!(
        topology.diagnostics("A").await.branch_id,
        left_before.branch_id
    );
    assert_eq!(
        topology.diagnostics("D").await.branch_id,
        right_before.branch_id
    );
    let bridge_text = "F4 sibling bridge must not carry content";
    assert_eq!(topology.send("A", "D", bridge_text).await.total_accepted, 0);
    assert!(!receiver_has_exact_text(topology.engine("D"), bridge_text).await);
    topology.shutdown().await;
}

// 交接复现：真实四实例分别接受、保留，最后对照事前预览和事后成员。
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn handoff_four_device_removal_preview_matches_executed_choice() {
    let rendezvous = mount_rendezvous().await;
    let mut topology = MembershipTopology::new(rendezvous.uri());
    topology
        .run(&[
            TopologyAction::Start { node: "A" },
            TopologyAction::Start { node: "B" },
            TopologyAction::Start { node: "C" },
            TopologyAction::Start { node: "D" },
            TopologyAction::Create { node: "A" },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "B",
            },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "C",
            },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "D",
            },
        ])
        .await;
    topology
        .wait_for_equivalent_branch(&["A", "B", "C", "D"], 4)
        .await;
    let epoch = topology.diagnostics("A").await.group_epoch;
    topology.wait_for_group_epoch(&["B", "C", "D"], epoch).await;
    topology
        .run(&[TopologyAction::Remove {
            sponsor: "B",
            target: "C",
        }])
        .await;
    topology.wait_for_pending_change(&["A", "C", "D"]).await;
    let apply_preview = topology.device_group_choices("A").await;
    let keep_preview = topology.device_group_choices("D").await;
    let pending = apply_preview.issues.first().unwrap();
    assert_eq!(
        pending.reason.kind,
        uc_engine::DeviceGroupChoiceReasonKind::PendingRemoval
    );
    assert_eq!(pending.reason.changes.len(), 1);
    let removed = &pending.reason.changes[0].target.device_id;
    let candidate = pending
        .choices
        .iter()
        .find(|choice| !choice.is_current_group)
        .unwrap();
    assert!(candidate.members_complete);
    assert_eq!(candidate.members.len(), 3);
    assert!(candidate
        .members
        .iter()
        .all(|member| !member.display_name.is_empty() && &member.device_id != removed));
    assert!(!candidate
        .impact
        .as_ref()
        .unwrap()
        .sync_scope_device_ids
        .contains(removed));
    topology
        .decide_pending_change("A", PendingChangeChoice::Apply)
        .await;
    topology
        .decide_pending_change("D", PendingChangeChoice::Keep)
        .await;
    let applied = topology.device_group_choices("A").await;
    let kept = topology.device_group_choices("D").await;
    let applied_diagnostics = topology.diagnostics("A").await;
    let kept_diagnostics = topology.diagnostics("D").await;
    topology.run(&[TopologyAction::Restart { node: "D" }]).await;
    let restarted_kept = topology.device_group_choices("D").await;
    let restarted_diagnostics = topology.diagnostics("D").await;
    topology.shutdown().await;

    assert_eq!(applied_diagnostics.effective_member_count, 3);
    assert_eq!(kept_diagnostics.effective_member_count, 4);
    assert_eq!(restarted_diagnostics.effective_member_count, 4);
    assert!(restarted_kept.device_trust.current_change.is_none());
    assert!(restarted_kept.issues.is_empty());
    assert!(applied.device_trust.current_change.is_none());
    assert!(kept.device_trust.current_change.is_none());
    let change = apply_preview.device_trust.current_change.unwrap();
    let kept_change = keep_preview.device_trust.current_change.unwrap();
    assert!(change.target_device_ids.iter().all(|id| applied
        .device_trust
        .devices
        .iter()
        .any(|device| &device.device_id == id
            && device.membership == uc_engine::DeviceMembershipSummary::Removed)));
    assert!(kept_change.target_device_ids.iter().all(|id| kept
        .device_trust
        .devices
        .iter()
        .any(|device| &device.device_id == id
            && device.membership == uc_engine::DeviceMembershipSummary::Active)));
    assert!(
        change
            .apply_impact
            .usable_device_ids
            .iter()
            .all(|id| !change.target_device_ids.contains(id)),
        "真实选择已经移除目标，但事前继续同步名单仍包含目标"
    );
}

// F3：同一远端移除被不同设备接受和拒绝后，决定必须跨重启持久并保持内容隔离。
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn f3_opposite_removal_decisions_persist_divergence_across_restart() {
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let mut topology = MembershipTopology::new(rendezvous.uri());
    topology
        .run(&[
            TopologyAction::Start { node: "A" },
            TopologyAction::Start { node: "B" },
            TopologyAction::Start { node: "C" },
            TopologyAction::Create { node: "A" },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "B",
            },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "C",
            },
        ])
        .await;
    topology
        .wait_for_equivalent_branch(&["A", "B", "C"], 3)
        .await;
    let baseline_epoch = topology.diagnostics("A").await.group_epoch;
    topology
        .wait_for_group_epoch(&["B", "C"], baseline_epoch)
        .await;
    topology
        .run(&[TopologyAction::Remove {
            sponsor: "A",
            target: "C",
        }])
        .await;
    topology.wait_for_pending_change(&["B", "C"]).await;
    topology
        .run(&[
            TopologyAction::Decide {
                node: "B",
                choice: PendingChangeChoice::Apply,
            },
            TopologyAction::Decide {
                node: "C",
                choice: PendingChangeChoice::Keep,
            },
        ])
        .await;

    topology.assert_snapshot("B", 2, 0).await;
    topology.assert_snapshot("C", 3, 0).await;
    let accepted_before = topology.diagnostics("B").await;
    let rejected_before = topology.diagnostics("C").await;
    assert_ne!(accepted_before.branch_id, rejected_before.branch_id);
    assert_ne!(accepted_before.head_event_id, rejected_before.head_event_id);
    topology
        .wait_for_group_epoch(&["B"], topology.diagnostics("A").await.group_epoch)
        .await;

    let accepted_text = "F3 accepted branch transfer";
    assert_eq!(
        topology.send("A", "B", accepted_text).await.total_accepted,
        1
    );
    wait_for_received_text(topology.engine("B"), accepted_text).await;
    let rejected_text = "F3 rejected branch must stay isolated";
    assert_eq!(
        topology.send("B", "C", rejected_text).await.total_accepted,
        0
    );
    assert!(!receiver_has_exact_text(topology.engine("C"), rejected_text).await);

    topology
        .run(&[
            TopologyAction::Restart { node: "B" },
            TopologyAction::Restart { node: "C" },
        ])
        .await;
    let accepted_after = topology.diagnostics("B").await;
    let rejected_after = topology.diagnostics("C").await;
    assert_eq!(accepted_after.branch_id, accepted_before.branch_id);
    assert_eq!(accepted_after.head_event_id, accepted_before.head_event_id);
    assert!(accepted_after.revision >= accepted_before.revision);
    assert_eq!(rejected_after.branch_id, rejected_before.branch_id);
    assert_eq!(rejected_after.head_event_id, rejected_before.head_event_id);
    assert!(rejected_after.revision >= rejected_before.revision);
    topology.assert_snapshot("B", 2, 0).await;
    topology.assert_snapshot("C", 3, 0).await;
    let restarted_text = "F3 restart preserves divergence";
    assert_eq!(
        topology.send("C", "B", restarted_text).await.total_accepted,
        0
    );
    assert!(!receiver_has_exact_text(topology.engine("B"), restarted_text).await);
    topology.shutdown().await;
}

// F2：不同 Sponsor 从共同父 head 移除不同叶子，明确选择后必须精确切换到目标分支。
#[tokio::test(flavor = "multi_thread", worker_threads = 10)]
async fn f2_concurrent_leaf_removals_resolve_to_selected_branch() {
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let mut topology = MembershipTopology::new(rendezvous.uri());
    topology
        .run(&[
            TopologyAction::Start { node: "A" },
            TopologyAction::Start { node: "B" },
            TopologyAction::Start { node: "C" },
            TopologyAction::Start { node: "D" },
            TopologyAction::Start { node: "E" },
            TopologyAction::Create { node: "A" },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "B",
            },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "C",
            },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "D",
            },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "E",
            },
        ])
        .await;
    topology
        .wait_for_equivalent_branch(&["A", "B", "C", "D", "E"], 5)
        .await;
    let baseline_epoch = topology.diagnostics("A").await.group_epoch;
    topology
        .wait_for_group_epoch(&["B", "C", "D", "E"], baseline_epoch)
        .await;
    topology
        .run(&[
            TopologyAction::Partition {
                left: &["A", "B", "D"],
                right: &["C", "E"],
            },
            TopologyAction::Remove {
                sponsor: "B",
                target: "D",
            },
            TopologyAction::Remove {
                sponsor: "C",
                target: "E",
            },
            TopologyAction::Heal {
                nodes: &["A", "B", "C", "D", "E"],
            },
        ])
        .await;
    topology.assert_snapshot("B", 4, 1).await;
    topology.assert_snapshot("C", 4, 1).await;
    let selected = topology.diagnostics("B").await;
    let choices = topology
        .device_group_choices_for_branch("C", &selected.branch_id)
        .await;
    let choice_id = format!("b:{}", selected.branch_id);
    let conflict = choices
        .issues
        .iter()
        .find(|issue| {
            issue
                .choices
                .iter()
                .any(|choice| choice.choice_id == choice_id)
        })
        .unwrap();
    let remote = choices
        .issues
        .iter()
        .flat_map(|issue| &issue.choices)
        .find(|choice| choice.choice_id == choice_id)
        .unwrap();
    assert!(
        remote.members_complete,
        "已验证远端分支必须提供完整候选名单"
    );
    assert_eq!(remote.member_device_ids.len(), 4);
    assert_eq!(remote.members.len(), 4);
    assert!(remote
        .members
        .iter()
        .all(|member| !member.display_name.is_empty()));
    assert_eq!(
        conflict.reason.kind,
        uc_engine::DeviceGroupChoiceReasonKind::DifferentRemovals
    );
    assert_eq!(conflict.reason.changes.len(), 2);
    let removed = &conflict
        .reason
        .changes
        .iter()
        .find(|change| change.side == uc_engine::DeviceGroupChangeSideSummary::Remote)
        .unwrap()
        .target
        .device_id;
    let impact = remote.impact.as_ref().unwrap();
    assert!(impact.paused_device_ids.contains(removed));
    assert!(impact.requires_rejoin_device_ids.contains(removed));
    assert!(!impact.sync_scope_device_ids.contains(removed));
    assert_eq!(
        impact.pending_confirmation_device_ids,
        impact.sync_scope_device_ids
    );
    topology
        .run(&[TopologyAction::ResolveConflict {
            node: "C",
            branch_from: "B",
        }])
        .await;
    topology
        .wait_for_equivalent_branch_named(&["B", "C"], 4, "F2 selected candidate recovery")
        .await;
    topology.assert_snapshot("C", 4, 0).await;
    let selected_after_recovery = topology.diagnostics("B").await;
    topology
        .wait_for_group_epoch(&["C"], selected_after_recovery.group_epoch)
        .await;
    let resolved = topology.diagnostics("C").await;
    assert_eq!(resolved.branch_id, selected.branch_id);
    assert_eq!(resolved.head_event_id, selected.head_event_id);
    assert_eq!(resolved.group_epoch, selected_after_recovery.group_epoch);
    // 切到保留 E 的新分支后，旧 Remove(E) 不得在重启维护中删除 E 的资料。
    topology.run(&[TopologyAction::Restart { node: "C" }]).await;
    let restarted = topology.device_group_choices("C").await;
    assert_eq!(
        restarted
            .device_trust
            .devices
            .iter()
            .filter(|device| device.membership == uc_engine::DeviceMembershipSummary::Active)
            .count(),
        4,
        "选中组的有效成员在重启后必须仍可完整查询"
    );
    topology.shutdown().await;
}

// F1：共同父 head 上并发移除与新增，两个合法分支必须保持各自成员语义。
#[tokio::test(flavor = "multi_thread", worker_threads = 10)]
async fn f1_remove_and_add_from_parent_head_preserve_branch_membership() {
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let mut topology = MembershipTopology::new(rendezvous.uri());
    topology
        .run(&[
            TopologyAction::Start { node: "A" },
            TopologyAction::Start { node: "B" },
            TopologyAction::Start { node: "C" },
            TopologyAction::Start { node: "D" },
            TopologyAction::Start { node: "E" },
            TopologyAction::Create { node: "A" },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "B",
            },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "C",
            },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "D",
            },
        ])
        .await;
    topology
        .wait_for_equivalent_branch(&["A", "B", "C", "D"], 4)
        .await;
    let baseline = topology.diagnostics("A").await;

    // 分区前确保现有成员已应用 group 更新，不能只依据成员列表已保存。
    topology
        .wait_for_group_epoch(&["B", "C", "D"], baseline.group_epoch)
        .await;

    // 与其他分区场景一致：网络切断前取得邀请，隔离期间只执行加入。
    let invitation = issue_invitation_named(topology.engine("B"), "B").await;
    topology
        .run(&[TopologyAction::Partition {
            left: &["A", "C", "D"],
            right: &["B", "E"],
        }])
        .await;
    topology.join_with_invitation("B", "E", invitation).await;
    topology
        .run(&[TopologyAction::Remove {
            sponsor: "A",
            target: "D",
        }])
        .await;

    // 移除提交允许 effect 留待恢复；先等待本机 group 更新，再断言分支状态。
    topology
        .wait_for_group_epoch(&["A"], baseline.group_epoch + 1)
        .await;
    let removed_branch = topology.diagnostics("A").await;
    let added_branch = topology.diagnostics("B").await;
    assert_ne!(removed_branch.branch_id, added_branch.branch_id);
    assert_ne!(removed_branch.head_event_id, added_branch.head_event_id);
    assert_eq!(removed_branch.effective_member_count, 3);
    assert_eq!(added_branch.effective_member_count, 5);
    assert!(removed_branch.group_epoch > baseline.group_epoch);
    assert!(added_branch.group_epoch > baseline.group_epoch);
    topology
        .wait_for_group_epoch(&["C"], removed_branch.group_epoch)
        .await;
    topology
        .wait_for_group_epoch(&["E"], added_branch.group_epoch)
        .await;

    let left_text = "F1 removal branch transfer";
    let left_report = topology.send("A", "C", left_text).await;
    assert_eq!(
        left_report.total_accepted + left_report.total_pending,
        1,
        "F1 removal branch transfer was neither accepted nor left in flight: {left_report:?}"
    );
    wait_for_received_text(topology.engine("C"), left_text).await;
    let right_text = "F1 addition branch transfer";
    let right_report = topology.send("B", "E", right_text).await;
    assert_eq!(
        right_report.total_accepted + right_report.total_pending,
        1,
        "F1 addition branch transfer was neither accepted nor left in flight: {right_report:?}"
    );
    wait_for_received_text(topology.engine("E"), right_text).await;
    let removed_text = "F1 removed member must not receive";
    assert_eq!(
        topology.send("A", "D", removed_text).await.total_accepted,
        0
    );
    assert!(!receiver_has_exact_text(topology.engine("D"), removed_text).await);

    topology
        .run(&[TopologyAction::Heal {
            nodes: &["A", "B", "C", "D", "E"],
        }])
        .await;
    for node in ["A", "B", "C", "D", "E"] {
        wait_for_peer_refresh(topology.engine(node), node).await;
    }
    topology.assert_snapshot("A", 3, 1).await;
    topology.assert_snapshot("B", 5, 1).await;
    let choices_a = topology.device_group_choices("A").await;
    let choices_b = topology.device_group_choices("B").await;
    let d_device_id = topology.device_ids.get("D").unwrap();
    let e_device_id = topology.device_ids.get("E").unwrap();
    assert_eq!(
        choices_a
            .device_trust
            .devices
            .iter()
            .find(|device| &device.device_id == d_device_id)
            .map(|device| device.membership),
        Some(uc_engine::DeviceMembershipSummary::Removed)
    );
    assert_eq!(
        choices_b
            .device_trust
            .devices
            .iter()
            .find(|device| &device.device_id == d_device_id)
            .map(|device| device.membership),
        Some(uc_engine::DeviceMembershipSummary::Active)
    );
    assert_eq!(
        choices_b
            .device_trust
            .devices
            .iter()
            .find(|device| &device.device_id == e_device_id)
            .map(|device| device.membership),
        Some(uc_engine::DeviceMembershipSummary::Active)
    );
    let healed_a = topology.diagnostics("A").await;
    let healed_b = topology.diagnostics("B").await;
    assert_eq!(healed_a.pending_conflict_count, 1);
    assert_eq!(healed_b.pending_conflict_count, 1);
    assert_ne!(healed_a.branch_id, healed_b.branch_id);
    let isolated_text = "F1 healed sibling branches remain isolated";
    assert_eq!(
        topology.send("A", "E", isolated_text).await.total_accepted,
        0
    );
    assert!(!receiver_has_exact_text(topology.engine("E"), isolated_text).await);
    topology.shutdown().await;
}

struct MembershipTopology {
    rendezvous_base_url: String,
    harnesses: HashMap<String, DeviceHarness>,
    engines: HashMap<String, Engine>,
    endpoint_ids_by_node: HashMap<String, [u8; 32]>,
    space_ids: HashMap<String, String>,
    device_ids: HashMap<String, String>,
}

impl MembershipTopology {
    fn new(rendezvous_base_url: String) -> Self {
        Self {
            rendezvous_base_url,
            harnesses: HashMap::new(),
            engines: HashMap::new(),
            endpoint_ids_by_node: HashMap::new(),
            space_ids: HashMap::new(),
            device_ids: HashMap::new(),
        }
    }

    async fn run(&mut self, actions: &[TopologyAction<'_>]) {
        for action in actions {
            match *action {
                TopologyAction::Start { node } => self.start(node).await,
                TopologyAction::Create { node } => self.create(node).await,
                TopologyAction::Join { sponsor, joiner } => self.join(sponsor, joiner).await,
                TopologyAction::Remove { sponsor, target } => self.remove(sponsor, target).await,
                TopologyAction::Restart { node } => self.restart(node).await,
                TopologyAction::Stop { node } => self.stop(node).await,
                TopologyAction::Decide { node, choice } => {
                    self.decide_pending_change(node, choice).await
                }
                TopologyAction::AssertSnapshot {
                    node,
                    active_members,
                    pending_choices,
                } => {
                    self.assert_snapshot(node, active_members, pending_choices)
                        .await
                }
                TopologyAction::AssertDiagnostics {
                    node,
                    effective_members,
                    pending_conflicts,
                    pending_effects,
                } => {
                    self.assert_diagnostics(
                        node,
                        effective_members,
                        pending_conflicts,
                        pending_effects,
                    )
                    .await
                }
                TopologyAction::Partition { left, right } => self.partition(left, right).await,
                TopologyAction::PartitionGroups { groups } => {
                    self.partition_groups(groups, None).await
                }
                TopologyAction::GroupedBridge {
                    groups,
                    left,
                    right,
                } => self.partition_groups(groups, Some((left, right))).await,
                TopologyAction::Bridge {
                    left,
                    right,
                    left_group,
                    right_group,
                } => {
                    self.bridge(left, right, left_group, right_group).await;
                }
                TopologyAction::Ring { nodes, isolated } => self.ring(nodes, isolated).await,
                TopologyAction::Chain { nodes, offline } => self.chain(nodes, offline).await,
                TopologyAction::Heal { nodes } => self.heal(nodes).await,
                TopologyAction::ResolveConflict { node, branch_from } => {
                    self.resolve_conflict(node, branch_from).await
                }
            }
        }
    }

    async fn start(&mut self, node: &str) {
        assert!(
            !self.engines.contains_key(node),
            "node {node} started twice"
        );
        let harness = DeviceHarness::new(self.rendezvous_base_url.clone());
        let engine = harness.start().await;
        let endpoint_id = query_endpoint_id(&engine, node).await;
        self.harnesses.insert(node.to_owned(), harness);
        self.engines.insert(node.to_owned(), engine);
        self.endpoint_ids_by_node
            .insert(node.to_owned(), endpoint_id);
    }

    async fn stop(&mut self, node: &str) {
        let engine = self
            .engines
            .remove(node)
            .unwrap_or_else(|| panic!("node {node} is not started"));
        engine
            .shutdown(SHUTDOWN_TIMEOUT)
            .await
            .unwrap_or_else(|error| panic!("node {node} shutdown failed: {error}"));
    }

    async fn restart(&mut self, node: &str) {
        let engine = self
            .engines
            .remove(node)
            .unwrap_or_else(|| panic!("node {node} is not started"));
        engine
            .shutdown(SHUTDOWN_TIMEOUT)
            .await
            .unwrap_or_else(|error| panic!("node {node} shutdown failed: {error}"));
        let restarted = self
            .harnesses
            .get(node)
            .unwrap_or_else(|| panic!("node {node} has no harness"))
            .start()
            .await;
        self.engines.insert(node.to_owned(), restarted);
        self.wait_for_membership_ready(node).await;
    }

    async fn wait_for_membership_ready(&self, node: &str) {
        let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
        loop {
            match self
                .engine(node)
                .execute(Operation::QueryDeviceGroupChoices)
                .await
            {
                Ok(OperationResult::DeviceGroupChoices(_)) => return,
                Ok(_) => panic!("node {node} returned an unexpected device group result"),
                Err(error)
                    if error.code() == 1211
                        && error.is_retryable()
                        && tokio::time::Instant::now() < deadline =>
                {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Err(error) => panic!("node {node} did not become membership-ready: {error}"),
            }
        }
    }

    async fn create(&mut self, node: &str) {
        let engine = self.engine(node);
        let (space_id, device_id) = create_space(engine, node).await;
        self.space_ids.insert(node.to_owned(), space_id);
        self.device_ids.insert(node.to_owned(), device_id);
    }

    async fn join(&mut self, sponsor: &str, joiner: &str) {
        let invitation = issue_invitation(self.engine(sponsor)).await;
        self.join_with_invitation(sponsor, joiner, invitation).await;
    }

    async fn join_with_invitation(&mut self, sponsor: &str, joiner: &str, full_invitation: String) {
        let space_id = self
            .space_ids
            .get(sponsor)
            .unwrap_or_else(|| panic!("sponsor {sponsor} has no space"))
            .clone();
        let joined =
            join_with_invitation(self.engine(joiner), joiner, &space_id, full_invitation).await;
        self.space_ids.insert(joiner.to_owned(), space_id);
        self.device_ids
            .insert(joiner.to_owned(), joined.self_device_id);
    }

    async fn remove(&self, sponsor: &str, target: &str) {
        let target_device_id = self
            .device_ids
            .get(target)
            .unwrap_or_else(|| panic!("target {target} has no device id"))
            .clone();
        let initial_members = self.diagnostics(sponsor).await.effective_member_count;
        let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
        loop {
            let result = self
                .engine(sponsor)
                .execute(Operation::RemoveMember(RemoveMemberInput {
                    device_id: target_device_id.clone(),
                }))
                .await;
            if matches!(&result, Err(error) if error.code() == 1393 && error.is_retryable()) {
                return;
            }
            if matches!(&result, Err(error) if error.code() == 1394)
                && self.diagnostics(sponsor).await.effective_member_count == initial_members
                && tokio::time::Instant::now() < deadline
            {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
            let result =
                result.unwrap_or_else(|error| panic!("node {sponsor} remove failed: {error}"));
            assert!(matches!(result, OperationResult::DeviceTrust(_)));
            return;
        }
    }

    async fn partition(&self, left: &[&str], right: &[&str]) {
        let left_ids = self.endpoint_ids(left).await;
        let right_ids = self.endpoint_ids(right).await;
        for node in left {
            self.set_partition(node, right_ids.clone()).await;
        }
        for node in right {
            self.set_partition(node, left_ids.clone()).await;
        }
    }

    async fn partition_groups(&self, groups: &[&[&str]], bridge: Option<(&str, &str)>) {
        let all = groups
            .iter()
            .flat_map(|group| group.iter().copied())
            .collect::<Vec<_>>();
        let unique = all
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(all.len(), unique.len(), "partition groups must be disjoint");
        if let Some((left, right)) = bridge {
            assert!(
                groups.iter().any(|group| group.contains(&left))
                    && groups.iter().any(|group| group.contains(&right)),
                "grouped bridge endpoints must belong to partition groups"
            );
        }
        for group in groups {
            for node in *group {
                let bridge_peer = bridge.and_then(|(left, right)| {
                    if *node == left {
                        Some(right)
                    } else if *node == right {
                        Some(left)
                    } else {
                        None
                    }
                });
                let blocked = all
                    .iter()
                    .copied()
                    .filter(|candidate| {
                        candidate != node
                            && !group.contains(candidate)
                            && Some(*candidate) != bridge_peer
                    })
                    .collect::<Vec<_>>();
                self.set_partition(node, self.endpoint_ids(&blocked).await)
                    .await;
            }
        }
    }

    async fn bridge(&self, left: &str, right: &str, left_group: &[&str], right_group: &[&str]) {
        assert!(left_group.contains(&left));
        assert!(right_group.contains(&right));
        let left_blocked = right_group
            .iter()
            .copied()
            .filter(|node| *node != right)
            .collect::<Vec<_>>();
        let right_blocked = left_group
            .iter()
            .copied()
            .filter(|node| *node != left)
            .collect::<Vec<_>>();
        self.set_partition(left, self.endpoint_ids(&left_blocked).await)
            .await;
        self.set_partition(right, self.endpoint_ids(&right_blocked).await)
            .await;
    }

    async fn ring(&self, nodes: &[&str], isolated: &[&str]) {
        assert!(nodes.len() >= 4, "ring requires at least four nodes");
        let all = nodes
            .iter()
            .chain(isolated.iter())
            .copied()
            .collect::<Vec<_>>();
        for (index, node) in nodes.iter().enumerate() {
            let previous = nodes[(index + nodes.len() - 1) % nodes.len()];
            let next = nodes[(index + 1) % nodes.len()];
            let blocked = all
                .iter()
                .copied()
                .filter(|candidate| {
                    candidate != node && *candidate != previous && *candidate != next
                })
                .collect::<Vec<_>>();
            self.set_partition(node, self.endpoint_ids(&blocked).await)
                .await;
        }
        for node in isolated {
            let blocked = all
                .iter()
                .copied()
                .filter(|candidate| candidate != node)
                .collect::<Vec<_>>();
            self.set_partition(node, self.endpoint_ids(&blocked).await)
                .await;
        }
    }

    async fn chain(&self, nodes: &[&str], offline: &[&str]) {
        assert!(nodes.len() >= 2, "chain requires at least two online nodes");
        let all = nodes
            .iter()
            .chain(offline.iter())
            .copied()
            .collect::<Vec<_>>();
        for (index, node) in nodes.iter().enumerate() {
            let previous = index.checked_sub(1).map(|previous| nodes[previous]);
            let next = nodes.get(index + 1).copied();
            let blocked = all
                .iter()
                .copied()
                .filter(|candidate| {
                    candidate != node && Some(*candidate) != previous && Some(*candidate) != next
                })
                .collect::<Vec<_>>();
            self.set_partition(node, self.endpoint_ids(&blocked).await)
                .await;
        }
    }

    async fn heal(&self, nodes: &[&str]) {
        for node in nodes {
            self.set_partition(node, Vec::new()).await;
        }
    }

    async fn endpoint_ids(&self, nodes: &[&str]) -> Vec<[u8; 32]> {
        nodes
            .iter()
            .map(|node| {
                *self
                    .endpoint_ids_by_node
                    .get(*node)
                    .unwrap_or_else(|| panic!("node {node} has no endpoint id"))
            })
            .collect()
    }

    async fn set_partition(&self, node: &str, blocked_endpoint_ids: Vec<[u8; 32]>) {
        let expected_count = blocked_endpoint_ids.len();
        let result = self
            .engine(node)
            .execute_dev(uc_engine::DevOperation::SetNetworkPartition {
                blocked_endpoint_ids,
            })
            .await
            .unwrap_or_else(|error| panic!("node {node} partition update failed: {error}"));
        assert_eq!(
            result,
            uc_engine::DevOperationResult::NetworkPartitionUpdated {
                blocked_peer_count: expected_count,
            }
        );
    }

    async fn send(&self, sender: &str, receiver: &str, text: &str) -> uc_engine::SendReportSummary {
        let receiver_id = self
            .device_ids
            .get(receiver)
            .unwrap_or_else(|| panic!("receiver {receiver} has no device id"));
        let result = self
            .engine(sender)
            .execute(Operation::SendText(SendTextInput {
                text: text.to_owned(),
                target_devices: vec![receiver_id.clone()],
            }))
            .await
            .unwrap_or_else(|error| panic!("node {sender} send failed: {error}"));
        let OperationResult::EntrySent(report) = result else {
            panic!("node {sender} returned an unexpected send result");
        };
        report
    }

    async fn wait_for_paired_peer(&self, sender: &str, receiver: &str) {
        let receiver_id = self
            .device_ids
            .get(receiver)
            .unwrap_or_else(|| panic!("receiver {receiver} has no device id"));
        let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
        loop {
            match self
                .engine(sender)
                .execute(Operation::QueryPeerConnections)
                .await
            {
                Ok(OperationResult::PeerConnections(peers))
                    if peers
                        .iter()
                        .any(|peer| &peer.peer_id == receiver_id && peer.is_paired) =>
                {
                    return;
                }
                Ok(OperationResult::PeerConnections(_)) => {}
                Ok(_) => panic!("node {sender} returned an unexpected peer connection result"),
                Err(error) if error.is_retryable() && tokio::time::Instant::now() < deadline => {}
                Err(error) => panic!("node {sender} peer connection query failed: {error}"),
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "same-branch peer {sender}-{receiver} did not become paired"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn diagnostics(&self, node: &str) -> uc_engine::MembershipDiagnosticsSummary {
        let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
        loop {
            match self
                .engine(node)
                .execute(Operation::QueryMembershipDiagnostics)
                .await
            {
                Ok(OperationResult::MembershipDiagnostics(summary)) => return summary,
                Ok(_) => panic!("node {node} returned an unexpected diagnostics result"),
                Err(error) if error.is_retryable() && tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Err(error) => panic!("node {node} diagnostics failed: {error}"),
            }
        }
    }

    async fn device_group_choices(&self, node: &str) -> uc_engine::DeviceGroupChoicesSummary {
        let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
        loop {
            match self
                .engine(node)
                .execute(Operation::QueryDeviceGroupChoices)
                .await
            {
                Ok(OperationResult::DeviceGroupChoices(summary)) => return summary,
                Ok(_) => panic!("node {node} returned an unexpected device group result"),
                Err(error) if error.is_retryable() && tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Err(error) => panic!("node {node} device group query failed: {error}"),
            }
        }
    }

    async fn device_group_choices_for_branch(
        &self,
        node: &str,
        branch_id: &str,
    ) -> uc_engine::DeviceGroupChoicesSummary {
        let choice_id = format!("b:{branch_id}");
        let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
        loop {
            let choices = self.device_group_choices(node).await;
            if choices.issues.iter().any(|issue| {
                issue.issue_id.starts_with("c:")
                    && issue
                        .choices
                        .iter()
                        .any(|choice| choice.choice_id == choice_id)
            }) {
                return choices;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "node {node} did not offer the requested branch"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn resolve_conflict(&self, node: &str, branch_from: &str) {
        let target = self.diagnostics(branch_from).await.branch_id;
        let choice_id = format!("b:{target}");
        let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
        loop {
            let choices = self.device_group_choices_for_branch(node, &target).await;
            let issue = choices
                .issues
                .iter()
                .find(|issue| {
                    issue.issue_id.starts_with("c:")
                        && issue
                            .choices
                            .iter()
                            .any(|choice| choice.choice_id == choice_id)
                })
                .unwrap_or_else(|| panic!("node {node} has no branch conflict"));
            assert!(issue
                .choices
                .iter()
                .any(|choice| choice.choice_id == choice_id));
            let result = match self
                .engine(node)
                .execute(Operation::ChooseDeviceGroup(ChooseDeviceGroupInput {
                    issue_id: issue.issue_id.clone(),
                    choice_id: choice_id.clone(),
                    expected_revision: choices.revision,
                    confirm_local_removal: false,
                }))
                .await
            {
                Ok(result) => result,
                Err(error) if error.is_retryable() && tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
                Err(error) => panic!("node {node} conflict resolution failed: {error}"),
            };
            let OperationResult::DeviceGroupChosen(result) = result else {
                panic!("node {node} returned an unexpected conflict resolution result");
            };
            match result.outcome {
                uc_engine::DeviceGroupChoiceOutcomeSummary::Pending
                | uc_engine::DeviceGroupChoiceOutcomeSummary::Completed
                | uc_engine::DeviceGroupChoiceOutcomeSummary::AlreadyCompleted => return,
                uc_engine::DeviceGroupChoiceOutcomeSummary::StateChanged => {
                    assert!(
                        tokio::time::Instant::now() < deadline,
                        "node {node} conflict selection revision did not stabilize"
                    );
                }
                outcome => panic!("node {node} conflict selection returned {outcome:?}"),
            }
        }
    }

    async fn wait_for_pending_change(&self, nodes: &[&str]) {
        let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
        loop {
            let mut ready = true;
            for node in nodes {
                if !self
                    .device_group_choices(node)
                    .await
                    .issues
                    .iter()
                    .any(|issue| issue.issue_id.starts_with("p:"))
                {
                    ready = false;
                }
            }
            if ready {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "pending removal did not reach every decision node"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn wait_for_branch_conflict(&self, nodes: &[&str]) {
        let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
        loop {
            let mut ready = true;
            for node in nodes {
                if !self
                    .device_group_choices(node)
                    .await
                    .issues
                    .iter()
                    .any(|issue| issue.issue_id.starts_with("c:"))
                {
                    ready = false;
                }
            }
            if ready {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "branch conflict did not reach every bridge endpoint"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn wait_for_stable_pending_effects(&self, nodes: &[&str]) -> Vec<u32> {
        const REQUIRED_STABLE_SAMPLES: usize = 10;
        let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
        let mut previous = None;
        let mut stable_samples = 0;
        loop {
            let mut state = Vec::with_capacity(nodes.len());
            for node in nodes {
                state.push(self.diagnostics(node).await.pending_effect_count);
            }
            if previous.as_ref() == Some(&state) {
                stable_samples += 1;
                if stable_samples >= REQUIRED_STABLE_SAMPLES {
                    return state;
                }
            } else {
                previous = Some(state);
                stable_samples = 0;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "membership pending effects did not stabilize"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn decide_pending_change(&self, node: &str, choice: PendingChangeChoice) {
        let choice_id = match choice {
            PendingChangeChoice::Apply => "apply",
            PendingChangeChoice::Keep => "keep",
        };
        let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
        loop {
            let choices = self.device_group_choices(node).await;
            let issue = choices
                .issues
                .iter()
                .find(|issue| issue.issue_id.starts_with("p:"))
                .unwrap_or_else(|| panic!("node {node} has no pending removal"));
            let result = match self
                .engine(node)
                .execute(Operation::ChooseDeviceGroup(ChooseDeviceGroupInput {
                    issue_id: issue.issue_id.clone(),
                    choice_id: choice_id.to_owned(),
                    expected_revision: choices.revision,
                    confirm_local_removal: false,
                }))
                .await
            {
                Ok(result) => result,
                Err(error) if error.is_retryable() && tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
                Err(error) => panic!("node {node} decision failed: {error}"),
            };
            let OperationResult::DeviceGroupChosen(result) = result else {
                panic!("node {node} returned an unexpected decision result");
            };
            match result.outcome {
                uc_engine::DeviceGroupChoiceOutcomeSummary::Completed
                | uc_engine::DeviceGroupChoiceOutcomeSummary::AlreadyCompleted => return,
                uc_engine::DeviceGroupChoiceOutcomeSummary::StateChanged => {
                    assert!(
                        tokio::time::Instant::now() < deadline,
                        "node {node} pending decision revision did not stabilize"
                    );
                }
                outcome => panic!("node {node} pending decision returned {outcome:?}"),
            }
        }
    }

    async fn apply_local_removal_with_confirmation(&self, node: &str) {
        let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
        loop {
            let choices = self.device_group_choices(node).await;
            let issue = choices
                .issues
                .iter()
                .find(|issue| issue.issue_id.starts_with("p:"))
                .unwrap_or_else(|| panic!("node {node} has no pending local removal"));
            let submit = |confirm_local_removal| {
                self.engine(node)
                    .execute(Operation::ChooseDeviceGroup(ChooseDeviceGroupInput {
                        issue_id: issue.issue_id.clone(),
                        choice_id: "apply".to_owned(),
                        expected_revision: choices.revision,
                        confirm_local_removal,
                    }))
            };

            let first = submit(false)
                .await
                .unwrap_or_else(|error| panic!("node {node} local removal prompt failed: {error}"));
            let OperationResult::DeviceGroupChosen(first) = first else {
                panic!("node {node} returned an unexpected local removal prompt result");
            };
            if first.outcome == uc_engine::DeviceGroupChoiceOutcomeSummary::StateChanged
                && tokio::time::Instant::now() < deadline
            {
                continue;
            }
            assert_eq!(
                first.outcome,
                uc_engine::DeviceGroupChoiceOutcomeSummary::LocalDeviceConfirmationRequired
            );

            let confirmed = submit(true).await.unwrap_or_else(|error| {
                panic!("node {node} local removal confirmation failed: {error}")
            });
            let OperationResult::DeviceGroupChosen(confirmed) = confirmed else {
                panic!("node {node} returned an unexpected local removal confirmation result");
            };
            match confirmed.outcome {
                uc_engine::DeviceGroupChoiceOutcomeSummary::Completed
                | uc_engine::DeviceGroupChoiceOutcomeSummary::AlreadyCompleted => return,
                uc_engine::DeviceGroupChoiceOutcomeSummary::StateChanged
                    if tokio::time::Instant::now() < deadline => {}
                outcome => panic!("node {node} local removal confirmation returned {outcome:?}"),
            }
        }
    }

    async fn wait_for_equivalent_branch(&self, nodes: &[&str], effective_members: u32) {
        self.wait_for_equivalent_branch_named(nodes, effective_members, "unnamed topology phase")
            .await;
    }

    async fn wait_for_equivalent_branch_named(
        &self,
        nodes: &[&str],
        effective_members: u32,
        phase: &str,
    ) {
        let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
        loop {
            let mut snapshots = Vec::with_capacity(nodes.len());
            for node in nodes {
                snapshots.push(self.diagnostics(node).await);
            }
            let first = snapshots
                .first()
                .unwrap_or_else(|| panic!("branch equivalence requires at least one node"));
            if snapshots.iter().all(|snapshot| {
                snapshot.branch_id == first.branch_id
                    && snapshot.head_event_id == first.head_event_id
                    && snapshot.effective_member_count == effective_members
            }) {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "nodes did not converge to the expected branch during {phase}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn wait_for_group_epoch(&self, nodes: &[&str], expected_epoch: u64) {
        self.wait_for_group_epoch_named(nodes, expected_epoch, "unnamed topology phase")
            .await;
    }

    async fn wait_for_group_epoch_named(&self, nodes: &[&str], expected_epoch: u64, phase: &str) {
        let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
        let mut observed_epochs = Vec::new();
        loop {
            let mut all_match = true;
            observed_epochs.clear();
            for node in nodes {
                let epoch = self.diagnostics(node).await.group_epoch;
                observed_epochs.push(epoch);
                if epoch != expected_epoch {
                    all_match = false;
                }
            }
            if all_match {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "nodes did not reach group epoch {expected_epoch} during {phase}; observed={observed_epochs:?}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn assert_snapshot(
        &self,
        node: &str,
        expected_active_members: usize,
        expected_pending_choices: usize,
    ) {
        let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
        loop {
            let observation = match self
                .engine(node)
                .execute(Operation::QueryDeviceGroupChoices)
                .await
            {
                Ok(OperationResult::DeviceGroupChoices(summary)) => {
                    let active_members = summary
                        .device_trust
                        .devices
                        .iter()
                        .filter(|device| {
                            device.membership == uc_engine::DeviceMembershipSummary::Active
                        })
                        .count();
                    let observation = format!(
                        "active={active_members}, issues={}, revision={}",
                        summary.issues.len(),
                        summary.revision
                    );
                    if active_members == expected_active_members
                        && summary.issues.len() == expected_pending_choices
                    {
                        return;
                    }
                    observation
                }
                Ok(_) => "unexpected result".to_owned(),
                Err(error) => format!("error={error}"),
            };
            assert!(
                tokio::time::Instant::now() < deadline,
                "node {node} did not reach the expected public snapshot; last observation: {observation}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn assert_diagnostics(
        &self,
        node: &str,
        expected_effective_members: u32,
        expected_pending_conflicts: u32,
        expected_pending_effects: u32,
    ) {
        let result = self
            .engine(node)
            .execute(Operation::QueryMembershipDiagnostics)
            .await
            .unwrap_or_else(|error| panic!("node {node} diagnostics failed: {error}"));
        let OperationResult::MembershipDiagnostics(summary) = result else {
            panic!("node {node} returned an unexpected diagnostics result");
        };

        assert_eq!(summary.branch_id.len(), 64);
        assert_eq!(summary.head_event_id.len(), 64);
        assert!(summary.group_epoch > 0);
        assert_eq!(summary.effective_member_count, expected_effective_members);
        assert_eq!(summary.pending_conflict_count, expected_pending_conflicts);
        assert_eq!(summary.pending_effect_count, expected_pending_effects);
        assert!(summary.transition_phases.is_empty());
    }

    fn engine(&self, node: &str) -> &Engine {
        self.engines
            .get(node)
            .unwrap_or_else(|| panic!("node {node} is not started"))
    }

    async fn shutdown(&self) {
        for engine in self.engines.values() {
            engine
                .shutdown(SHUTDOWN_TIMEOUT)
                .await
                .expect("shut down topology node");
        }
    }
}

// 分区门必须同时拒绝新连接并关闭已存在连接；Heal 后使用同一 Engine 恢复通信。
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn topology_partition_blocks_all_iroh_channels_until_healed() {
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let mut topology = MembershipTopology::new(rendezvous.uri());
    topology
        .run(&[
            TopologyAction::Start { node: "A" },
            TopologyAction::Start { node: "B" },
            TopologyAction::Create { node: "A" },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "B",
            },
            TopologyAction::Partition {
                left: &["A"],
                right: &["B"],
            },
        ])
        .await;

    let blocked_text = "partitioned transfer must stay isolated";
    let blocked = topology.send("A", "B", blocked_text).await;
    assert_eq!(blocked.total_accepted, 0);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!receiver_has_exact_text(topology.engine("B"), blocked_text).await);

    topology
        .run(&[TopologyAction::Heal { nodes: &["A", "B"] }])
        .await;
    wait_for_peer_refresh(topology.engine("A"), "A after heal").await;
    wait_for_peer_refresh(topology.engine("B"), "B after heal").await;
    let healed_text = "healed transfer succeeds";
    let healed = topology.send("A", "B", healed_text).await;
    assert_eq!(healed.total_accepted, 1);
    wait_for_received_text(topology.engine("B"), healed_text).await;
    topology.shutdown().await;
}

// F0：共同 head 分区后由两个 Sponsor 分别准入新设备，必须形成隔离的 sibling 分支。
#[tokio::test(flavor = "multi_thread", worker_threads = 10)]
async fn f0_partitioned_sponsors_create_isolated_sibling_branches() {
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let mut topology = MembershipTopology::new(rendezvous.uri());
    topology
        .run(&[
            TopologyAction::Start { node: "A" },
            TopologyAction::Start { node: "B" },
            TopologyAction::Start { node: "C" },
            TopologyAction::Start { node: "D" },
            TopologyAction::Start { node: "E" },
            TopologyAction::Create { node: "A" },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "B",
            },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "C",
            },
        ])
        .await;
    topology
        .wait_for_equivalent_branch(&["A", "B", "C"], 3)
        .await;
    let baseline_a = topology.diagnostics("A").await;
    let baseline_b = topology.diagnostics("B").await;
    assert_eq!(baseline_a.branch_id, baseline_b.branch_id);
    assert_eq!(baseline_a.head_event_id, baseline_b.head_event_id);

    topology
        .run(&[
            TopologyAction::Partition {
                left: &["A", "C", "D"],
                right: &["B", "E"],
            },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "D",
            },
            TopologyAction::Join {
                sponsor: "B",
                joiner: "E",
            },
        ])
        .await;

    let branch_a = topology.diagnostics("A").await;
    let branch_b = topology.diagnostics("B").await;
    assert_ne!(branch_a.branch_id, branch_b.branch_id);
    assert_ne!(branch_a.head_event_id, branch_b.head_event_id);
    assert_eq!(branch_a.effective_member_count, 4);
    assert_eq!(branch_b.effective_member_count, 4);
    assert!(branch_a.group_epoch > baseline_a.group_epoch);
    assert!(branch_b.group_epoch > baseline_b.group_epoch);
    assert_eq!(
        branch_a.group_epoch,
        topology.diagnostics("D").await.group_epoch
    );
    assert_eq!(
        branch_b.group_epoch,
        topology.diagnostics("E").await.group_epoch
    );

    let left_text = "F0 left branch transfer";
    assert_eq!(topology.send("A", "D", left_text).await.total_accepted, 1);
    wait_for_received_text(topology.engine("D"), left_text).await;
    let right_text = "F0 right branch transfer";
    assert_eq!(topology.send("B", "E", right_text).await.total_accepted, 1);
    wait_for_received_text(topology.engine("E"), right_text).await;
    let isolated_text = "F0 cross branch transfer must fail";
    assert_eq!(
        topology.send("A", "E", isolated_text).await.total_accepted,
        0
    );
    assert!(!receiver_has_exact_text(topology.engine("E"), isolated_text).await);

    topology
        .run(&[TopologyAction::Heal {
            nodes: &["A", "B", "C", "D", "E"],
        }])
        .await;
    for node in ["A", "B", "C", "D", "E"] {
        wait_for_peer_refresh(topology.engine(node), node).await;
    }
    topology.assert_snapshot("A", 4, 1).await;
    topology.assert_snapshot("B", 4, 1).await;
    let healed_a = topology.diagnostics("A").await;
    let healed_b = topology.diagnostics("B").await;
    assert_ne!(healed_a.branch_id, healed_b.branch_id);
    assert_eq!(healed_a.pending_conflict_count, 1);
    assert_eq!(healed_b.pending_conflict_count, 1);
    let healed_isolated_text = "F0 healed sibling branches remain isolated";
    assert_eq!(
        topology
            .send("A", "E", healed_isolated_text)
            .await
            .total_accepted,
        0
    );
    assert!(!receiver_has_exact_text(topology.engine("E"), healed_isolated_text).await);
    topology.shutdown().await;
}

// 声明式拓扑脚本只能通过稳定 Engine operation 观察和推进节点。
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn offline_member_returns_after_repeated_invitation_cancellation_and_new_join() {
    let rendezvous = mount_rendezvous().await;
    let mut topology = MembershipTopology::new(rendezvous.uri());
    topology
        .run(&[
            TopologyAction::Start { node: "A" },
            TopologyAction::Start { node: "B" },
            TopologyAction::Start { node: "C" },
            TopologyAction::Create { node: "A" },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "B",
            },
        ])
        .await;
    topology
        .wait_for_equivalent_branch_named(&["A", "B"], 2, "initial members")
        .await;
    topology.stop("B").await;
    let before = topology.diagnostics("A").await;
    for _ in 0..3 {
        issue_invitation(topology.engine("A")).await;
        topology
            .engine("A")
            .execute(Operation::CancelInvitation)
            .await
            .unwrap();
        let OperationResult::SetupState(setup) = topology
            .engine("A")
            .execute(Operation::QuerySetupState)
            .await
            .unwrap()
        else {
            panic!("expected setup state");
        };
        assert!(setup.current_invitation.is_none());
        let after = topology.diagnostics("A").await;
        assert_eq!(after.head_event_id, before.head_event_id);
        assert_eq!(after.group_epoch, before.group_epoch);
        assert_eq!(after.effective_member_count, 2);
    }
    topology.join("A", "C").await;
    let returning = topology.harnesses.get("B").unwrap().start().await;
    topology.engines.insert("B".to_owned(), returning);
    topology
        .wait_for_equivalent_branch_named(
            &["A", "B", "C"],
            3,
            "offline member catches new admission",
        )
        .await;
    let epoch = topology.diagnostics("A").await.group_epoch;
    topology.wait_for_group_epoch(&["B", "C"], epoch).await;
    for node in ["A", "B", "C"] {
        wait_for_peer_refresh(topology.engine(node), node).await;
    }
    let text = "returning member after cancelled invitations";
    assert_eq!(topology.send("A", "B", text).await.total_accepted, 1);
    wait_for_received_text(topology.engine("B"), text).await;
    let reverse = "returning member can send";
    assert_eq!(topology.send("B", "C", reverse).await.total_accepted, 1);
    wait_for_received_text(topology.engine("C"), reverse).await;
    topology.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn topology_script_builds_a_two_node_space_through_public_operations() {
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let mut topology = MembershipTopology::new(rendezvous.uri());

    topology
        .run(&[
            TopologyAction::Start { node: "A" },
            TopologyAction::Start { node: "B" },
            TopologyAction::Create { node: "A" },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "B",
            },
            TopologyAction::AssertSnapshot {
                node: "B",
                active_members: 2,
                pending_choices: 0,
            },
            TopologyAction::AssertDiagnostics {
                node: "B",
                effective_members: 2,
                pending_conflicts: 0,
                pending_effects: 0,
            },
        ])
        .await;
    topology.shutdown().await;
    uc_engine::flush_test_tracing();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn offline_member_catches_multiple_removals_without_blocking_new_invitations() {
    let rendezvous = mount_rendezvous().await;
    let mut topology = MembershipTopology::new(rendezvous.uri());
    for node in ["A", "B", "C", "D"] {
        topology.start(node).await;
    }
    topology.create("A").await;
    for node in ["B", "C", "D"] {
        topology.join("A", node).await;
    }
    topology
        .wait_for_equivalent_branch_named(&["A", "B", "C", "D"], 4, "initial four members")
        .await;
    for node in ["B", "C", "D"] {
        topology.stop(node).await;
    }
    for node in ["C", "D"] {
        topology.remove("A", node).await;
        issue_invitation(topology.engine("A")).await;
        topology
            .engine("A")
            .execute(Operation::CancelInvitation)
            .await
            .unwrap();
    }
    topology.restart("A").await;
    issue_invitation(topology.engine("A")).await;
    let returning = topology.harnesses.get("B").unwrap().start().await;
    topology.engines.insert("B".to_owned(), returning);
    for _ in 0..2 {
        topology.wait_for_pending_change(&["B"]).await;
        topology
            .decide_pending_change("B", PendingChangeChoice::Apply)
            .await;
        if topology.diagnostics("B").await.effective_member_count == 2 {
            break;
        }
    }
    topology
        .wait_for_equivalent_branch_named(&["A", "B"], 2, "returning member accepts removals")
        .await;
    let epoch = topology.diagnostics("A").await.group_epoch;
    topology.wait_for_group_epoch(&["B"], epoch).await;
    for node in ["A", "B"] {
        wait_for_peer_refresh(topology.engine(node), node).await;
    }
    let text = "multiple offline removals recovered";
    assert_eq!(topology.send("A", "B", text).await.total_accepted, 1);
    wait_for_received_text(topology.engine("B"), text).await;
    topology.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "显式完整配对 trace 验收：独占进程级 OTLP receiver，使用 --ignored 精确运行"]
async fn uninterrupted_admission_uses_one_trace() {
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
    let started_at = SystemTime::now();
    let started = Instant::now();

    join_with_invitation(&joiner, "Joiner", &space_id, invitation).await;
    wait_for_active_member_count(&sponsor, 2).await;
    wait_for_active_member_count(&joiner, 2).await;
    let elapsed = started.elapsed();
    sponsor
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("stop sponsor");
    joiner
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("stop joiner");
    uc_engine::flush_test_tracing();

    let evidence_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let trace_evidence = loop {
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
    };

    assert_eq!(trace_evidence.flow_ids.len(), 1);
    assert_eq!(trace_evidence.lifecycle_root_count, 1);
    assert_eq!(trace_evidence.invalid_pair_count, 0);
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
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "显式重启链路验收：独占进程级 OTLP receiver，使用 --ignored 精确运行"]
async fn in_flight_admission_restart_uses_new_traces_and_one_flow() {
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
    let benchmark_mode = std::env::var("UC_PAIRING_OBSERVABILITY_BENCHMARK").ok();
    let telemetry = if matches!(
        benchmark_mode.as_deref(),
        Some("none" | "disabled" | "unavailable")
    ) {
        None
    } else {
        let telemetry = MockServer::start().await;
        for endpoint in ["/v1/traces", "/v1/logs"] {
            Mock::given(method("POST"))
                .and(path(endpoint))
                .respond_with(ResponseTemplate::new(200))
                .mount(&telemetry)
                .await;
        }
        Some(telemetry)
    };
    match benchmark_mode.as_deref() {
        Some("none") => {}
        Some("disabled") => assert!(uc_engine::init_test_tracing_without_remote()),
        Some("unavailable") => assert!(uc_engine::init_test_tracing_with_otlp(
            "http://127.0.0.1:1/v1/traces",
            "http://127.0.0.1:1/v1/logs",
        )),
        Some("healthy") | None => {
            let telemetry = telemetry.as_ref().expect("healthy OTLP receiver");
            assert!(uc_engine::init_test_tracing_with_otlp(
                &format!("{}/v1/traces", telemetry.uri()),
                &format!("{}/v1/logs", telemetry.uri()),
            ));
        }
        Some(other) => panic!("unknown observability benchmark mode: {other}"),
    }
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
    if let Some(mode) = benchmark_mode {
        eprintln!(
            "UC_PAIRING_OBSERVABILITY_RESULT mode={mode} elapsed_us={}",
            elapsed.as_micros()
        );
        assert!(elapsed < Duration::from_secs(30));
        return;
    }
    let telemetry = telemetry.expect("performance gate OTLP receiver");
    let evidence_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let trace_evidence = loop {
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
    };
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
    assert!(
        trace_evidence.local_elapsed < PAIRING_HOT_PATH_BUDGET,
        "two-device local pairing work took {:?}, network-related time {:?}, end-to-end {:?}, local budget {:?}",
        trace_evidence.local_elapsed,
        trace_evidence.network_elapsed,
        elapsed,
        PAIRING_HOT_PATH_BUDGET,
    );
}

struct PairingTraceEvidence {
    local_elapsed: Duration,
    network_elapsed: Duration,
    client_count: usize,
    paired_server_count: usize,
    paired_admission_endpoint_count: usize,
    lifecycle_root_count: usize,
    invalid_pair_count: usize,
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
    let mut invalid_pair_count = clients
        .iter()
        .filter(|client| {
            let root =
                lifecycle_root_ids.get(&(client.trace_id.clone(), client.parent_span_id.clone()));
            client.parent_span_id.is_empty()
                || root.is_none()
                || root.and_then(|span| otlp_string_attribute(&span.attributes, "uc.flow.id"))
                    != otlp_string_attribute(&client.attributes, "uc.flow.id")
                || otlp_string_attribute(&client.attributes, "uc.flow.id").is_none()
                || !otlp_span_succeeded(client)
        })
        .count()
        + paired_servers
            .iter()
            .filter(|(_, server)| {
                otlp_string_attribute(&server.attributes, "uc.flow.id").is_some()
                    || !otlp_span_succeeded(server)
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

fn unix_time_ns(time: SystemTime) -> u64 {
    u64::try_from(
        time.duration_since(UNIX_EPOCH)
            .expect("wall clock after epoch")
            .as_nanos(),
    )
    .unwrap_or(u64::MAX)
}

fn otlp_span_succeeded(span: &opentelemetry_proto::tonic::trace::v1::Span) -> bool {
    span.status.as_ref().is_some_and(|status| {
        status.code == opentelemetry_proto::tonic::trace::v1::status::StatusCode::Ok as i32
    })
}

fn otlp_string_attribute<'a>(
    attributes: &'a [opentelemetry_proto::tonic::common::v1::KeyValue],
    key: &str,
) -> Option<&'a str> {
    attributes
        .iter()
        .find(|attribute| attribute.key == key)
        .and_then(|attribute| attribute.value.as_ref())
        .and_then(|value| value.value.as_ref())
        .and_then(|value| match value {
            OtlpValue::StringValue(value) => Some(value.as_str()),
            _ => None,
        })
}

async fn wait_for_active_member_count(engine: &Engine, expected: u32) {
    let deadline = tokio::time::Instant::now() + ADMISSION_WAIT_TIMEOUT;
    loop {
        if let Ok(OperationResult::MembershipDiagnostics(summary)) =
            engine.execute(Operation::QueryMembershipDiagnostics).await
        {
            if summary.effective_member_count == expected {
                return;
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "active member count did not reach {expected}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

// 新设备只经过稳定 JoinSpace 入口，并最终形成可查询的活动 Space。
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn fresh_device_join_completes_through_stable_operations() {
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let first_harness = DeviceHarness::new(rendezvous.uri());
    let joiner_harness = DeviceHarness::new(rendezvous.uri());
    let first = first_harness.start().await;
    let joiner = joiner_harness.start().await;
    let first_space_id = create_space(&first, "First Sponsor").await.0;

    join_through(&first, &joiner, "Joining Device", &first_space_id).await;

    for engine in [&first, &joiner] {
        engine
            .shutdown(SHUTDOWN_TIMEOUT)
            .await
            .expect("shut down join routing engine");
    }
}

// 新成员加入后，已经在 Space 中的成员也必须收到同一更新并能联系新成员。
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn existing_member_receives_the_new_member_update() {
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let sponsor_harness = DeviceHarness::new(rendezvous.uri());
    let existing_harness = DeviceHarness::new(rendezvous.uri());
    let newcomer_harness = DeviceHarness::new(rendezvous.uri());
    let sponsor = sponsor_harness.start().await;
    let existing = existing_harness.start().await;
    let newcomer = newcomer_harness.start().await;
    let space_id = create_space(&sponsor, "Sponsor").await.0;

    let existing_id = join_through(&sponsor, &existing, "Existing Member", &space_id)
        .await
        .self_device_id;
    wait_for_active_member_count(&sponsor, 2).await;
    wait_for_active_member_count(&existing, 2).await;
    let newcomer_id = join_through(&sponsor, &newcomer, "New Member", &space_id)
        .await
        .self_device_id;

    for engine in [&sponsor, &existing, &newcomer] {
        wait_for_active_member_count(engine, 3).await;
    }
    tokio::join!(
        automatic_connections::wait_eligible(&existing, &newcomer_id),
        automatic_connections::wait_eligible(&newcomer, &existing_id),
    );
    automatic_connections::wait_online(&existing, &newcomer_id).await;
    automatic_connections::wait_online(&newcomer, &existing_id).await;
    let text = "existing member sees newcomer";
    let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
    loop {
        let sent = existing
            .execute(Operation::SendText(SendTextInput {
                text: text.to_owned(),
                target_devices: vec![newcomer_id.clone()],
            }))
            .await
            .expect("existing member sends to newcomer");
        let OperationResult::EntrySent(report) = sent else {
            panic!("unexpected send result");
        };
        if report.total_accepted == 1 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "existing member did not establish delivery to the newcomer"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    wait_for_received_text(&newcomer, text).await;

    for engine in [&sponsor, &existing, &newcomer] {
        engine
            .shutdown(SHUTDOWN_TIMEOUT)
            .await
            .expect("shut down three-member engine");
    }
    uc_engine::flush_test_tracing();
}

// 已完成设置的设备通过同一 JoinSpace 入口切换到另一个 Space。
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn existing_device_switches_space_through_stable_operations() {
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let first_harness = DeviceHarness::new(rendezvous.uri());
    let joiner_harness = DeviceHarness::new(rendezvous.uri());
    let second_harness = DeviceHarness::new(rendezvous.uri());
    let first = first_harness.start().await;
    let joiner = joiner_harness.start().await;
    let second = second_harness.start().await;
    let first_space_id = create_space(&first, "First Sponsor").await.0;
    let second_space_id = create_space(&second, "Second Sponsor").await.0;
    join_through(&first, &joiner, "Joining Device", &first_space_id).await;

    join_through(&second, &joiner, "Joining Device", &second_space_id).await;

    for engine in [&first, &joiner, &second] {
        engine
            .shutdown(SHUTDOWN_TIMEOUT)
            .await
            .expect("shut down space switch engine");
    }
}

// 加入完成后重启 Joiner，持久化准入状态必须足以恢复成员权限并接收正文。
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn completed_admission_survives_restart_and_allows_transfer() {
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let sponsor_harness = DeviceHarness::new(rendezvous.uri());
    let joiner_harness = DeviceHarness::new(rendezvous.uri());
    let sponsor = sponsor_harness.start().await;
    let joiner = joiner_harness.start().await;
    let space_id = create_space(&sponsor, "Sponsor").await.0;
    let joiner_id = join_through(&sponsor, &joiner, "Joiner", &space_id)
        .await
        .self_device_id;
    joiner
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shut down admitted joiner");

    let restarted_joiner = joiner_harness.start().await;
    wait_for_peer_refresh(&sponsor, "sponsor").await;
    wait_for_peer_refresh(&restarted_joiner, "joiner").await;
    let text = "admission survives restart";
    sponsor
        .execute(Operation::SendText(SendTextInput {
            text: text.to_owned(),
            target_devices: vec![joiner_id],
        }))
        .await
        .expect("send text to restarted joiner");
    wait_for_received_text(&restarted_joiner, text).await;

    sponsor
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shut down sponsor");
    restarted_joiner
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shut down restarted joiner");
}

async fn wait_for_peer_refresh(engine: &Engine, label: &str) {
    let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
    loop {
        if let Ok(OperationResult::PeerConnectionsRefreshed(report)) =
            engine.execute(Operation::RefreshPeerConnections).await
        {
            if report.total > 0
                && report.online == report.total
                && report.offline == 0
                && report.errors == 0
            {
                return;
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{label} peer connection refresh timed out"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn query_endpoint_id(engine: &Engine, node: &str) -> [u8; 32] {
    let result = engine
        .execute_dev(uc_engine::DevOperation::QueryNetworkEndpointId)
        .await
        .unwrap_or_else(|error| panic!("node {node} endpoint query failed: {error}"));
    let uc_engine::DevOperationResult::NetworkEndpointId(endpoint_id) = result else {
        panic!("node {node} returned an unexpected endpoint result");
    };
    endpoint_id
}

async fn create_space(engine: &Engine, device_name: &str) -> (String, String) {
    let created = match engine
        .execute(Operation::CreateSpace(CreateSpaceInput {
            device_name: Some(device_name.to_owned()),
            passphrase: SecretString::new(PASSPHRASE),
            passphrase_confirmation: SecretString::new(PASSPHRASE),
        }))
        .await
        .expect("create space")
    {
        OperationResult::SpaceCreated {
            space_id,
            self_device_id,
            ..
        } => (space_id, self_device_id),
        other => panic!("unexpected create result: {other:?}"),
    };
    let OperationResult::SetupState(setup) = engine
        .execute(Operation::QuerySetupState)
        .await
        .expect("query created space")
    else {
        panic!("unexpected setup result after create");
    };
    assert_eq!(setup.space_id.as_deref(), Some(created.0.as_str()));
    created
}

struct JoinResult {
    self_device_id: String,
}

async fn join_through(
    sponsor: &Engine,
    joiner: &Engine,
    device_name: &str,
    expected_space_id: &str,
) -> JoinResult {
    let full_invitation = issue_invitation(sponsor).await;
    join_with_invitation(joiner, device_name, expected_space_id, full_invitation).await
}

async fn issue_invitation(sponsor: &Engine) -> String {
    issue_invitation_named(sponsor, "sponsor").await
}

async fn issue_invitation_named(sponsor: &Engine, sponsor_label: &str) -> String {
    let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
    loop {
        match sponsor.execute(Operation::IssueInvitation).await {
            Ok(OperationResult::InvitationIssued {
                full_invitation, ..
            }) => return full_invitation,
            Ok(_) => panic!("unexpected invitation result"),
            Err(error) if error.is_retryable() && tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(error) => panic!(
                "node {sponsor_label} issue admission invitation: {error}; retryable={}",
                error.is_retryable()
            ),
        }
    }
}

async fn join_with_invitation(
    joiner: &Engine,
    device_name: &str,
    expected_space_id: &str,
    full_invitation: String,
) -> JoinResult {
    let OperationResult::JoinSpace(status) = joiner
        .execute(Operation::JoinSpace(JoinSpaceInput {
            invitation_code: full_invitation,
            device_name: Some(device_name.to_owned()),
            passphrase: SecretString::new(PASSPHRASE),
            preserve_unreadable_history: false,
        }))
        .await
        .expect("start admission join")
    else {
        panic!("unexpected join result");
    };
    wait_for_completed_join(joiner, device_name, status, expected_space_id).await
}

async fn wait_for_completed_join(
    engine: &Engine,
    device_name: &str,
    mut status: JoinSpaceStatusSummary,
    expected_space_id: &str,
) -> JoinResult {
    let deadline = tokio::time::Instant::now() + ADMISSION_WAIT_TIMEOUT;
    loop {
        match status {
            JoinSpaceStatusSummary::Active { joined_space, .. } => {
                assert_eq!(joined_space.space_id, expected_space_id);
                return JoinResult {
                    self_device_id: joined_space.self_device_id,
                };
            }
            JoinSpaceStatusSummary::Rejected { reason, .. } => {
                panic!("admission was rejected: {reason:?}")
            }
            JoinSpaceStatusSummary::Pending { .. } => {}
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "space admission timed out for {device_name}; last status: {status:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
        let snapshot = match engine.execute(Operation::QueryDeviceGroupChoices).await {
            Ok(OperationResult::DeviceGroupChoices(summary)) => summary.device_trust,
            Ok(_) => panic!("unexpected device trust result"),
            Err(_) => continue,
        };
        if let Some(current_join) = snapshot.current_join {
            status = current_join;
            continue;
        }
        let setup = match engine.execute(Operation::QuerySetupState).await {
            Ok(OperationResult::SetupState(setup)) => setup,
            Ok(_) => panic!("unexpected setup state result"),
            Err(_) => continue,
        };
        if setup.has_completed && setup.space_id.as_deref() == Some(expected_space_id) {
            let device = match engine.execute(Operation::QueryLocalDevice).await {
                Ok(OperationResult::LocalDevice(device)) => device,
                Ok(_) => panic!("unexpected local device result"),
                Err(_) => continue,
            };
            return JoinResult {
                self_device_id: device.device_id,
            };
        }
    }
}

async fn wait_for_received_text(engine: &Engine, expected_text: &str) {
    let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
    loop {
        if receiver_has_exact_text(engine, expected_text).await {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "content delivery timed out"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn receiver_has_exact_text(engine: &Engine, expected_text: &str) -> bool {
    let OperationResult::HistoryEntries(entries) = engine
        .execute(Operation::ListHistoryEntries(ListHistoryEntriesInput {
            limit: 100,
            offset: 0,
        }))
        .await
        .expect("list history entries")
    else {
        panic!("unexpected history list result");
    };
    for entry in entries {
        match engine
            .execute(Operation::GetHistoryEntry(HistoryEntryInput {
                entry_id: entry.entry_id,
            }))
            .await
            .expect("get history entry")
        {
            OperationResult::HistoryEntry(detail) if detail.content == expected_text => {
                return true
            }
            OperationResult::HistoryEntry(_) => {}
            other => panic!("unexpected history detail result: {other:?}"),
        }
    }
    false
}
