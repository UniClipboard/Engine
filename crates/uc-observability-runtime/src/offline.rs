//! Bounded local OTLP exchange records. No plaintext history is written to disk.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::local_recording::LocalRecordingState;
use crate::process_identity::birth_identity;
use opentelemetry::Context;
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::{any_value, AnyValue, KeyValue};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::metrics::v1::{
    metric, number_data_point, Gauge, Metric, NumberDataPoint, ResourceMetrics, ScopeMetrics, Sum,
};
use opentelemetry_proto::tonic::resource::v1::Resource as ProtoResource;
use opentelemetry_proto::transform::common::tonic::ResourceAttributesWithSchema;
use opentelemetry_proto::transform::trace::tonic::group_spans_by_resource_and_scope;
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::trace::{Span, SpanData, SpanProcessor};
use opentelemetry_sdk::Resource;
use serde::Serialize;
use serde_json::{json, Value};
use std::path::PathBuf;

#[path = "offline/storage.rs"]
mod storage;
pub use storage::OfflineHistoryStorage;
use storage::RecoveryRecord;
use sysinfo::{Pid, Process, ProcessRefreshKind, ProcessesToUpdate, System};
use uuid::Uuid;

const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_RECORD_BYTES: usize = 64 * 1024;
const RETENTION: Duration = Duration::from_secs(1800);

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OfflineDiagnosticSnapshot {
    pub metrics: String,
    pub logs: String,
    pub traces: String,
    pub summary: Value,
}

struct Entry {
    at: Instant,
    wall_ms: u64,
    signal: usize,
    line: String,
}
#[derive(Default)]
struct History {
    entries: VecDeque<Entry>,
    bytes: usize,
    evicted: u64,
    rejected: u64,
}

pub(crate) struct OfflineHistory {
    history: Mutex<History>,
    resource: ProtoResource,
    run_id: String,
    started: AtomicBool,
    stopped: AtomicBool,
    sample_request: Mutex<Option<SyncSender<SyncSender<()>>>>,
    storage: Mutex<Option<OfflineHistoryStorage>>,
    persistence: Mutex<&'static str>,
    persistence_trimmed: AtomicU64,
    recovery: Mutex<Option<RecoveryRecord>>,
    host_registration_failures: AtomicU64,
    hosts: Mutex<HashMap<u32, HostProcess>>,
}

impl OfflineHistory {
    pub(crate) fn new(resource: &Resource) -> Arc<Self> {
        let run_id = Uuid::new_v4().to_string();
        // Select resource attributes explicitly. SDK defaults can contain host identity.
        let mut attributes = vec![
            attribute("service.name", "uniclipboard"),
            attribute("service.instance.id", &run_id),
            attribute("uc.process.role", "daemon"),
        ];
        for key in ["service.version", "os.type", "host.arch"] {
            if let Some((_, value)) = resource.iter().find(|(k, _)| k.as_str() == key) {
                attributes.push(attribute(key, &value.to_string()));
            }
        }
        let mut system = System::new();
        let pid = Pid::from_u32(std::process::id());
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::nothing().with_cpu(),
        );
        attributes.push(KeyValue {
            key_strindex: 0,
            key: "process.pid".into(),
            value: Some(AnyValue {
                value: Some(any_value::Value::IntValue(i64::from(std::process::id()))),
            }),
        });
        if let Some(process) = system.process(pid) {
            attributes.push(attribute(
                "process.creation.time",
                &process.start_time().to_string(),
            ));
        }
        Arc::new(Self {
            history: Mutex::new(History::default()),
            resource: ProtoResource {
                attributes,
                ..Default::default()
            },
            run_id,
            started: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
            sample_request: Mutex::new(None),
            storage: Mutex::new(None),
            persistence: Mutex::new("key_unavailable"),
            persistence_trimmed: AtomicU64::new(0),
            recovery: Mutex::new(None),
            host_registration_failures: AtomicU64::new(0),
            hosts: Mutex::new(HashMap::new()),
        })
    }

    pub(crate) fn start(
        self: &Arc<Self>,
        recording: Weak<LocalRecordingState>,
    ) -> std::io::Result<()> {
        if self.started.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let weak = Arc::downgrade(self);
        let (tx, rx) = mpsc::sync_channel(1);
        *self
            .sample_request
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(tx);
        if let Err(error) = thread::Builder::new()
            .name("uc-process-metrics".into())
            .spawn(move || sample_process(weak, rx, recording))
        {
            self.started.store(false, Ordering::Release);
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn register_host(&self, pid: u32, role: &str) -> std::io::Result<()> {
        match self.register_host_identity(pid, role) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.host_registration_failures
                    .fetch_add(1, Ordering::Relaxed);
                Err(error)
            }
        }
    }

    fn register_host_identity(&self, pid: u32, role: &str) -> std::io::Result<()> {
        if !matches!(role, "gui" | "quick-panel" | "webview") || pid == std::process::id() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid diagnostic process registration",
            ));
        }
        let identity = birth_identity(pid)?;
        let mut system = System::new();
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[Pid::from_u32(pid)]),
            true,
            ProcessRefreshKind::nothing()
                .with_cpu()
                .with_exe(sysinfo::UpdateKind::Always),
        );
        let process = system.process(Pid::from_u32(pid)).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "registered diagnostic process is unavailable",
            )
        })?;
        let executable = process
            .exe()
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "registered diagnostic process identity is unavailable",
                )
            })?
            .to_path_buf();
        if birth_identity(pid)? != identity {
            return Err(std::io::Error::other(
                "diagnostic process changed during registration",
            ));
        }
        let mut hosts = self.hosts.lock().unwrap_or_else(PoisonError::into_inner);
        if hosts.get(&pid).is_some_and(|known| {
            known.state == "active"
                && known.identity == identity
                && known.start == process.start_time()
                && known.executable == executable
                && known.role == role
        }) {
            return Ok(());
        }
        hosts.retain(|_, host| host.state == "active");
        if hosts.len() >= 32 {
            return Err(std::io::Error::other(
                "diagnostic process registration quota exceeded",
            ));
        }
        let mut resource = self.resource.clone();
        resource.attributes.retain(|a| {
            !matches!(
                a.key.as_str(),
                "service.instance.id" | "uc.process.role" | "process.pid" | "process.creation.time"
            )
        });
        resource.attributes.extend([
            attribute("service.instance.id", &Uuid::new_v4().to_string()),
            attribute("uc.process.role", role),
            attribute("process.creation.time", &process.start_time().to_string()),
            KeyValue {
                key_strindex: 0,
                key: "process.pid".into(),
                value: Some(AnyValue {
                    value: Some(any_value::Value::IntValue(i64::from(pid))),
                }),
            },
        ]);
        hosts.insert(
            pid,
            HostProcess {
                identity,
                start: process.start_time(),
                executable,
                role: role.to_owned(),
                resource,
                state: "active",
                previous: None,
            },
        );
        Ok(())
    }

    pub(crate) fn protect(&self, storage: OfflineHistoryStorage) {
        *self.storage.lock().unwrap_or_else(PoisonError::into_inner) = Some(storage);
    }

    pub(crate) fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
    }

    fn push(&self, signal: usize, record: &impl Serialize) {
        if !self.started.load(Ordering::Acquire) || self.stopped.load(Ordering::Acquire) {
            return;
        }
        let mut history = self.history.lock().unwrap_or_else(PoisonError::into_inner);
        let Ok(mut line) = serde_json::to_string(record) else {
            history.rejected += 1;
            return;
        };
        line.push('\n');
        if line.len() > MAX_RECORD_BYTES {
            history.rejected += 1;
            return;
        }
        let now = Instant::now();
        while history
            .entries
            .front()
            .is_some_and(|e| now.duration_since(e.at) > RETENTION)
            || history.bytes + line.len() > MAX_BYTES
        {
            if let Some(entry) = history.entries.pop_front() {
                history.bytes -= entry.line.len();
                history.evicted += 1;
            } else {
                break;
            }
        }
        history.bytes += line.len();
        history.entries.push_back(Entry {
            at: now,
            wall_ms: unix_ms(),
            signal,
            line,
        });
    }

    pub(crate) fn snapshot(&self) -> OfflineDiagnosticSnapshot {
        let (done, completed) = mpsc::sync_channel(1);
        let refresh = self
            .sample_request
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|tx| tx.try_send(done).is_ok())
            && completed.recv_timeout(Duration::from_secs(1)).is_ok();
        let host_coverage: Vec<Value> = self.hosts.lock().unwrap_or_else(PoisonError::into_inner).iter().map(|(pid, host)| json!({"pid": pid, "role": host.role, "state": host.state, "processCreationTime": host.start})).collect();
        let mut history = self.history.lock().unwrap_or_else(PoisonError::into_inner);
        while history
            .entries
            .front()
            .is_some_and(|e| e.at.elapsed() > RETENTION)
        {
            if let Some(entry) = history.entries.pop_front() {
                history.bytes -= entry.line.len();
                history.evicted += 1;
            }
        }
        let mut signals = [String::new(), String::new(), String::new()];
        for entry in &history.entries {
            signals[entry.signal].push_str(&entry.line);
        }
        let last_metric_age = history
            .entries
            .iter()
            .rev()
            .find(|entry| entry.signal == 0)
            .map(|entry| entry.at.elapsed());
        let summary = json!({"snapshotRefresh": if refresh { "completed" } else { "unavailable_or_timeout" }, "schemaVersion": 1, "coverage": if self.started.load(Ordering::Acquire) { "registered_processes" } else { "not_started" }, "metricScope": "per_process_no_application_total", "processMemorySemanticConventionVersion": "1.37.0", "processMemorySemanticConventionStability": "development", "hostCoverage": host_coverage, "hostRegistrationFailures": self.host_registration_failures.load(Ordering::Relaxed), "webviewAttribution": "unknown_unless_explicitly_registered_by_owner", "runId": self.run_id, "persistence": *self.persistence.lock().unwrap_or_else(PoisonError::into_inner), "retentionSeconds": RETENTION.as_secs(), "maxBytes": MAX_BYTES, "retainedBytes": history.bytes, "evictedRecords": history.evicted, "rejectedRecords": history.rejected, "persistenceTrimmedRecords": self.persistence_trimmed.load(Ordering::Acquire), "historyRecovery": self.recovery.lock().unwrap_or_else(PoisonError::into_inner).clone(), "lastMetricAgeMs": last_metric_age.map(|age| age.as_millis()), "metricsStale": last_metric_age.is_none_or(|age| age > Duration::from_secs(15)), "snapshotAt": chrono::Utc::now().to_rfc3339(), "lastRecordAgeMs": history.entries.back().map(|e| e.at.elapsed().as_millis()), "cpuUtilization": "single logical core fraction; uc.process.cpu.utilization, not normalized process.cpu.utilization", "memory": "process.memory.usage bytes; RSS on Unix, working set on Windows", "traceCoverage": "completed spans only; eviction may leave incomplete traces"});
        let [metrics, logs, traces] = signals;
        OfflineDiagnosticSnapshot {
            metrics,
            logs,
            traces,
            summary,
        }
    }

    pub(crate) fn log(&self, record: &Value) {
        let timestamp = record["timestamp"]
            .as_str()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .and_then(|t| t.timestamp_nanos_opt())
            .and_then(|t| u64::try_from(t).ok());
        let Some(time_unix_nano) = timestamp else {
            return;
        };
        let mut attributes = Vec::new();
        if let Some(fields) = record["fields"].as_object() {
            attributes.extend(
                fields
                    .iter()
                    .filter_map(|(key, value)| log_attribute(key, value)),
            );
        }
        // These owner-generated aliases are scoped by the original LocalRecordingState run_id.
        // Never copy arbitrary top-level input or replace this scope with the SDK resource ID.
        for key in LOCAL_CORRELATION_ATTRIBUTES {
            if let Some(value) = record.get(*key) {
                attributes.retain(|attribute| attribute.key != *key);
                attributes.extend(log_attribute(key, value));
            }
        }
        let log = LogRecord {
            time_unix_nano,
            observed_time_unix_nano: time_unix_nano,
            severity_text: record["level"].as_str().unwrap_or_default().into(),
            attributes,
            trace_id: decode_hex(record["trace_id"].as_str().unwrap_or_default(), 16),
            span_id: decode_hex(record["span_id"].as_str().unwrap_or_default(), 8),
            ..Default::default()
        };
        self.push(
            1,
            &ExportLogsServiceRequest {
                resource_logs: vec![ResourceLogs {
                    resource: Some(self.resource.clone()),
                    scope_logs: vec![ScopeLogs {
                        log_records: vec![log],
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
            },
        );
    }
}

// Approved metadata from LocalRecordingState::annotate/module_metadata and CapturePolicy::include.
const LOCAL_CORRELATION_ATTRIBUTES: &[&str] = &[
    "local_schema_version",
    "run_id",
    "monotonic_offset_ms",
    "engine_version",
    "host_version",
    "platform",
    "environment",
    "app_channel",
    "source_commit",
    "source_state",
    "peer_ref",
    "candidate_set_ref",
    "candidate_generation",
    "candidate_values_changed",
    "connection_id",
    "path_ref",
    "device_ref",
    "address_record_ref",
    "correlation_status",
    "connection_correlation_status",
    "record_correlation_status",
    "capture_mode",
    "capture_id",
    "capture_tail_completion",
];

fn log_attribute(key: &str, value: &Value) -> Option<KeyValue> {
    let value = match value {
        Value::String(value) => Some(any_value::Value::StringValue(value.clone())),
        Value::Bool(value) => Some(any_value::Value::BoolValue(*value)),
        Value::Number(value) => Some(any_value::Value::IntValue(value.as_i64()?)),
        Value::Null => None,
        _ => return None,
    };
    Some(KeyValue {
        key_strindex: 0,
        key: key.into(),
        value: Some(AnyValue { value }),
    })
}

fn decode_hex(value: &str, bytes: usize) -> Vec<u8> {
    if value.len() != bytes * 2 || !value.is_ascii() {
        return Vec::new();
    }
    (0..bytes)
        .map(|i| u8::from_str_radix(&value[i * 2..i * 2 + 2], 16))
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_default()
}

fn attribute(key: &str, value: &str) -> KeyValue {
    KeyValue {
        key_strindex: 0,
        key: key.into(),
        value: Some(AnyValue {
            value: Some(any_value::Value::StringValue(value.into())),
        }),
    }
}

fn gauge(name: &str, unit: &str, value: f64, at: u64, state: &str) -> Metric {
    Metric {
        name: name.into(),
        unit: unit.into(),
        data: Some(metric::Data::Gauge(Gauge {
            data_points: vec![NumberDataPoint {
                time_unix_nano: at,
                value: Some(number_data_point::Value::AsDouble(value)),
                attributes: vec![attribute("uc.sample.state", state)],
                ..Default::default()
            }],
        })),
        ..Default::default()
    }
}

fn sample_process(
    weak: Weak<OfflineHistory>,
    requests: mpsc::Receiver<SyncSender<()>>,
    recording: Weak<LocalRecordingState>,
) {
    let pid = Pid::from_u32(std::process::id());
    let mut system = System::new();
    let refresh = ProcessRefreshKind::nothing().with_cpu().with_memory();
    let mut previous: Option<(Instant, SystemTime, u64, u64)> = None;
    let mut reply: Option<SyncSender<()>> = None;
    loop {
        let Some(history) = weak.upgrade() else {
            return;
        };
        if history.stopped.load(Ordering::Acquire) {
            history.persist();
            return;
        }
        let now = Instant::now();
        let wall = SystemTime::now();
        system.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), true, refresh);
        if let Some(process) = system.process(pid) {
            record_process(
                &history,
                process,
                &history.resource,
                now,
                wall,
                &mut previous,
            );
        } else {
            previous = None;
        }
        {
            let mut hosts = history.hosts.lock().unwrap_or_else(PoisonError::into_inner);
            let pids: Vec<Pid> = hosts
                .iter()
                .filter(|(_, host)| host.state == "active")
                .map(|(pid, _)| Pid::from_u32(*pid))
                .collect();
            if !pids.is_empty() {
                system.refresh_processes_specifics(
                    ProcessesToUpdate::Some(&pids),
                    true,
                    refresh.with_exe(sysinfo::UpdateKind::Always),
                );
            }
            for (pid, host) in hosts.iter_mut() {
                if host.state != "active" {
                    continue;
                }
                let Some(process) = system.process(Pid::from_u32(*pid)) else {
                    host.state = "exited";
                    host.previous = None;
                    continue;
                };
                match birth_identity(*pid) {
                    Ok(identity) if identity == host.identity => {}
                    Ok(_) => {
                        host.state = "identity_changed";
                        host.previous = None;
                        continue;
                    }
                    Err(_) => {
                        host.state = "identity_unavailable";
                        host.previous = None;
                        continue;
                    }
                }
                if process.start_time() != host.start
                    || process.exe() != Some(host.executable.as_path())
                {
                    host.state = "identity_changed";
                    host.previous = None;
                    continue;
                }
                record_process(
                    &history,
                    process,
                    &host.resource,
                    now,
                    wall,
                    &mut host.previous,
                );
            }
        }
        history.persist();
        drop(history);
        if let Some(done) = reply.take() {
            let _ = done.try_send(());
        }
        match requests.recv_timeout(Duration::from_secs(
            if recording
                .upgrade()
                .is_some_and(|state| state.detailed_active())
            {
                1
            } else {
                5
            },
        )) {
            Ok(done) => reply = Some(done),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

pub(crate) struct OfflineSpanProcessor {
    history: Arc<OfflineHistory>,
    resource: ResourceAttributesWithSchema,
}
impl OfflineSpanProcessor {
    pub(crate) fn new(history: Arc<OfflineHistory>, resource: &Resource) -> Self {
        Self {
            history,
            resource: resource.into(),
        }
    }
}
impl std::fmt::Debug for OfflineSpanProcessor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OfflineSpanProcessor")
    }
}
impl SpanProcessor for OfflineSpanProcessor {
    fn on_start(&self, _: &mut Span, _: &Context) {}
    fn on_end(&self, span: SpanData) {
        let mut resource_spans = group_spans_by_resource_and_scope(vec![span], &self.resource);
        for resource in &mut resource_spans {
            resource.resource = Some(self.history.resource.clone());
        }
        self.history
            .push(2, &ExportTraceServiceRequest { resource_spans });
    }
    fn force_flush(&self) -> OTelSdkResult {
        Ok(())
    }
    fn shutdown_with_timeout(&self, _: Duration) -> OTelSdkResult {
        Ok(())
    }
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or_default()
}

struct HostProcess {
    identity: u128,
    start: u64,
    executable: PathBuf,
    role: String,
    resource: ProtoResource,
    state: &'static str,
    previous: Option<(Instant, SystemTime, u64, u64)>,
}

fn record_process(
    history: &OfflineHistory,
    process: &Process,
    resource: &ProtoResource,
    now: Instant,
    wall: SystemTime,
    previous: &mut Option<(Instant, SystemTime, u64, u64)>,
) {
    let cpu = process.accumulated_cpu_time();
    let start = process.start_time();
    let at = wall
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos().min(u64::MAX as u128) as u64)
        .unwrap_or_default();
    let mut state = "warming_up";
    let mut utilization = None;
    if let Some((last, last_wall, last_cpu, last_start)) = *previous {
        let elapsed = now.duration_since(last);
        if start != last_start || cpu < last_cpu {
            state = "process_restarted";
        } else if elapsed > Duration::from_secs(15)
            || wall
                .duration_since(last_wall)
                .ok()
                .is_none_or(|gap| gap > Duration::from_secs(15))
        {
            state = "gap";
        } else if elapsed >= Duration::from_secs(1) {
            state = "valid";
            utilization = Some((cpu - last_cpu) as f64 / 1000.0 / elapsed.as_secs_f64());
        }
    }
    let mut memory = gauge(
        "process.memory.usage",
        "By",
        process.memory() as f64,
        at,
        state,
    );
    if let Some(metric::Data::Gauge(mut value)) = memory.data.take() {
        for point in &mut value.data_points {
            point.start_time_unix_nano = start.saturating_mul(1_000_000_000);
        }
        memory.data = Some(metric::Data::Sum(Sum {
            data_points: value.data_points,
            aggregation_temporality: 2,
            is_monotonic: false,
        }));
    }
    let mut metrics = vec![
        memory,
        gauge("uc.process.cpu.time", "s", cpu as f64 / 1000.0, at, state),
    ];
    if let Some(value) = utilization {
        metrics.push(gauge("uc.process.cpu.utilization", "1", value, at, state));
    }
    history.push(
        0,
        &ExportMetricsServiceRequest {
            resource_metrics: vec![ResourceMetrics {
                resource: Some(resource.clone()),
                scope_metrics: vec![ScopeMetrics {
                    metrics,
                    ..Default::default()
                }],
                ..Default::default()
            }],
        },
    );
    if previous.is_none_or(|(last, _, _, _)| now.duration_since(last) >= Duration::from_secs(1)) {
        *previous = Some((now, wall, cpu, start));
    }
}
