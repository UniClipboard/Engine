//! 持续采样与已结束 span 的文件流提交。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::diagnostic_store::{encode_record, METRICS, RECORD_BYTES, TRACES};
use crate::local_file::LocalFileRuntime;
use crate::local_recording::LocalRecordingState;
use crate::process_identity::birth_identity;
use crate::remote_health::prepare_diagnostic_span;
use opentelemetry::Context;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::{any_value, AnyValue, KeyValue};
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

use sysinfo::{Pid, Process, ProcessRefreshKind, ProcessesToUpdate, System};
use uuid::Uuid;

pub(crate) struct DiagnosticStream {
    file: Option<Weak<LocalFileRuntime>>,
    resource: ProtoResource,
    run_id: String,
    started: AtomicBool,
    stopped: AtomicBool,
    sample_request: Mutex<Option<SyncSender<SyncSender<()>>>>,
    host_registration_failures: AtomicU64,
    hosts: Mutex<HashMap<u32, HostProcess>>,
}

impl DiagnosticStream {
    pub(crate) fn new(
        resource: &Resource,
        file: Option<&Arc<LocalFileRuntime>>,
        run: Uuid,
    ) -> Arc<Self> {
        let run_id = run.to_string();
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
            file: file.map(Arc::downgrade),
            resource: ProtoResource {
                attributes,
                ..Default::default()
            },
            run_id,
            started: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
            sample_request: Mutex::new(None),
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

    pub(crate) fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
    }

    fn push(&self, signal: usize, record: &impl Serialize) {
        if let Some(file) = self.file.as_ref().and_then(Weak::upgrade) {
            file.record(if signal == 0 { METRICS } else { TRACES }, record);
        }
    }

    pub(crate) fn prepare(&self, deadline: Duration) -> Value {
        let (done, completed) = mpsc::sync_channel(1);
        let refresh = self
            .sample_request
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|tx| tx.try_send(done).is_ok())
            && completed
                .recv_timeout(deadline.min(Duration::from_secs(1)))
                .is_ok();
        let host_coverage: Vec<Value> = self.hosts.lock().unwrap_or_else(PoisonError::into_inner).iter().map(|(pid, host)| json!({"pid": pid, "role": host.role, "state": host.state, "processCreationTime": host.start})).collect();
        json!({"snapshotRefresh": if refresh { "completed" } else { "unavailable_or_timeout" },
            "coverage": if self.started.load(Ordering::Acquire) { "registered_processes" } else { "not_started" },
            "runId": self.run_id, "hostCoverage": host_coverage,
            "hostRegistrationFailures": self.host_registration_failures.load(Ordering::Relaxed),
            "stream": self.file.as_ref().and_then(Weak::upgrade).map(|file| file.stream_summary()),
            "traceCoverage": "completed spans only", "metricScope": "per_process_no_application_total"})
    }
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
    weak: Weak<DiagnosticStream>,
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

pub(crate) struct DiagnosticSpanProcessor {
    history: Arc<DiagnosticStream>,
    resource: ResourceAttributesWithSchema,
}
impl DiagnosticSpanProcessor {
    pub(crate) fn new(history: Arc<DiagnosticStream>, resource: &Resource) -> Self {
        Self {
            history,
            resource: resource.into(),
        }
    }
}
impl std::fmt::Debug for DiagnosticSpanProcessor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DiagnosticSpanProcessor")
    }
}
impl SpanProcessor for DiagnosticSpanProcessor {
    fn on_start(&self, _: &mut Span, _: &Context) {}
    fn on_end(&self, mut span: SpanData) {
        let Some(file) = self.history.file.as_ref().and_then(Weak::upgrade) else {
            return;
        };
        let Some(_slot) = file.encoding_slot() else {
            return;
        };
        if !matches!(prepare_diagnostic_span(&mut span), Ok(true)) {
            file.record_rejection();
            return;
        }
        let mut resource_spans = group_spans_by_resource_and_scope(vec![span], &self.resource);
        for resource in &mut resource_spans {
            resource.resource = Some(self.history.resource.clone());
        }
        match encode_record(&ExportTraceServiceRequest { resource_spans }, RECORD_BYTES) {
            Some(bytes) => file.writer().write_owned(bytes, TRACES, false),
            None => file.record_rejection(),
        }
    }
    fn force_flush(&self) -> OTelSdkResult {
        Ok(())
    }
    fn shutdown_with_timeout(&self, _: Duration) -> OTelSdkResult {
        Ok(())
    }
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
    history: &DiagnosticStream,
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
