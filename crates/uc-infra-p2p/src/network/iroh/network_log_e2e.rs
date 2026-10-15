//! 独立网络日志的真实双进程验收。
//!
//! 服务端与客户端是同一测试可执行文件的两个子进程，各自安装进程唯一的观测运行时；
//! 客户端经 Engine 自己的两条拨号路径连接服务端的 iroh Router。编排测试读取两端的
//! 实际落盘文件，不检查内存状态。
//!
//! 子进程通过环境变量配置：`UC_NLOG_ROLE`（`server`/`client`）、`UC_NLOG_SCENARIO`、
//! `UC_NLOG_MODE`（`audit` 只记录第三方事件的元数据，用于审查调用点；`runtime` 安装真实运行时）、
//! `UC_NLOG_NETWORK`（`1` 时配置独立网络日志）、`UC_NLOG_OUT`（场景工件目录）。
//! 容量预算是本验收显式给出的测试参数，不是产品默认值。

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iroh::endpoint::{presets, Connection};
use iroh::protocol::{AcceptError, ProtocolHandler, Router};
use iroh::{Endpoint, EndpointAddr, RelayMode};
use serde_json::Value;
use tracing::field::{Field, Visit};
use tracing::instrument::Instrumented;
use tracing::{Instrument, Subscriber};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::Layer;
use uc_observability_contract::diagnostics::connectivity::AddressInputSource;
use uc_observability_runtime::{
    DeploymentEnvironment, HostLogLayer, LocalLogConfig, NetworkLogBudget, NetworkLogConfig,
    NetworkProcessRole, ObservabilityConfig, ObservabilityResource, OperatingSystem,
    ProcessObservabilityRuntime,
};

use super::connect::connect_with_staggered_retry;
use super::space_admission::dial_for_network_log_e2e as admission_connect;
use super::SPACE_ADMISSION_ALPN;

const SERVED_ALPN: &[u8] = b"uc-network-log/e2e/served/1";
const UNSERVED_ALPN: &[u8] = b"uc-network-log/e2e/unserved-canary/1";
const BUSINESS_SPAN: &str = "e2e_business";
/// 测试进程里唯一的业务 span 目标；host 层只看见它，看不见 Engine 合同目标。
const BUSINESS_TARGET: &str = "e2e.business";

#[derive(Debug)]
struct Hold;

impl ProtocolHandler for Hold {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        connection.closed().await;
        Ok(())
    }
}

/// 只记录第三方事件元数据与字段（审查调用点用，仅在本机回环、不进入产品）。
struct AuditLayer {
    out: Mutex<std::fs::File>,
}

struct FieldDump(Vec<(String, String)>);

impl Visit for FieldDump {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0.push((field.name().to_owned(), format!("{value:?}")));
    }
}

impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for AuditLayer {
    fn on_event(&self, event: &tracing::Event<'_>, ctx: Context<'_, S>) {
        use std::io::Write;
        let metadata = event.metadata();
        let mut fields = FieldDump(Vec::new());
        event.record(&mut fields);
        let spans: Vec<String> = ctx
            .event_scope(event)
            .map(|scope| scope.from_root().map(|s| s.name().to_owned()).collect())
            .unwrap_or_default();
        let line = serde_json::json!({
            "level": metadata.level().as_str(),
            "target": metadata.target(),
            "module_path": metadata.module_path(),
            "file": metadata.file(),
            "line": metadata.line(),
            "fields": fields.0,
            "spans": spans,
        });
        if let Ok(mut file) = self.out.lock() {
            let _ = writeln!(file, "{line}");
        }
    }
}

/// 宿主输出的替身：记录业务 span 何时关闭，并统计宿主输出收到的第三方网络事件数。
#[derive(Clone, Default)]
struct HostProbe {
    business_closed_at: Arc<Mutex<Option<Instant>>>,
    third_party_events: Arc<Mutex<u64>>,
}

impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for HostProbe {
    fn on_close(&self, id: tracing::span::Id, ctx: Context<'_, S>) {
        if ctx
            .span(&id)
            .is_some_and(|span| span.name() == BUSINESS_SPAN)
        {
            *self.business_closed_at.lock().expect("closed") = Some(Instant::now());
        }
    }

    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let metadata = event.metadata();
        let first = |path: &str| path.split("::").next().unwrap_or_default().to_owned();
        let target = first(metadata.target());
        let module = metadata.module_path().map(first).unwrap_or_default();
        if ["iroh", "noq", "noq_proto", "iroh_relay"]
            .iter()
            .any(|name| target == *name || module == *name)
        {
            *self.third_party_events.lock().expect("events") += 1;
        }
    }
}

fn out_root() -> PathBuf {
    PathBuf::from(std::env::var("UC_NLOG_OUT").expect("UC_NLOG_OUT"))
}

async fn bind_endpoint(alpns: Vec<Vec<u8>>) -> Endpoint {
    let endpoint = Endpoint::builder(presets::Minimal)
        .alpns(alpns)
        .relay_mode(RelayMode::Disabled)
        .clear_address_lookup()
        .bind()
        .await
        .expect("bind endpoint");
    for _ in 0..200 {
        if !endpoint.addr().addrs.is_empty() {
            return endpoint;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("endpoint never published direct addresses");
}

/// 本验收显式给出的预算；产品默认值需用户确认，这里不引用。
fn test_budget() -> NetworkLogBudget {
    let number = |name: &str, default: u64| -> u64 {
        std::env::var(name)
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(default)
    };
    NetworkLogBudget {
        record_bytes: 8 * 1024,
        queue_records: usize::try_from(number("UC_NLOG_QUEUE_RECORDS", 512)).expect("queue"),
        file_bytes: number("UC_NLOG_FILE_BYTES", 4 * 1024 * 1024),
        run_bytes: number("UC_NLOG_RUN_BYTES", 16 * 1024 * 1024),
        root_bytes: number("UC_NLOG_ROOT_BYTES", 64 * 1024 * 1024),
        retention: Duration::from_secs(number("UC_NLOG_RETENTION_SECS", 7 * 24 * 3600)),
    }
}

struct ChildRuntime {
    handle: Option<uc_observability_runtime::ProcessObservabilityHandle>,
    probe: HostProbe,
    stages: Mutex<Vec<Value>>,
}

impl ChildRuntime {
    fn new(
        handle: Option<uc_observability_runtime::ProcessObservabilityHandle>,
        probe: HostProbe,
    ) -> Self {
        Self {
            handle,
            probe,
            stages: Mutex::new(Vec::new()),
        }
    }

    /// 记录一个阶段的状态和刷新结果；刷新同时是写入恢复的重试点。
    fn stage(&self, name: &str) {
        let Some(handle) = &self.handle else { return };
        let export = handle.prepare_network_log_export(Duration::from_secs(2));
        let status = handle.query_network_log_status();
        self.stages.lock().expect("stages").push(serde_json::json!({
            "name": name,
            "status": status.as_ref().ok(),
            "status_error": status.as_ref().err().map(ToString::to_string),
            "flush": export.as_ref().ok().map(|report| report.flush),
            "files": export.as_ref().ok().map(|report| report.files.len()),
        }));
    }
}

/// 客户端目录名可由编排方指定，使同一服务端下的多个客户端进程互不覆盖工件。
fn role_name(role: &str) -> String {
    match (role, std::env::var("UC_NLOG_CLIENT_NAME")) {
        ("client", Ok(name)) => name,
        _ => role.to_owned(),
    }
}

fn log_root(role_dir: &Path) -> PathBuf {
    std::env::var("UC_NLOG_LOGROOT").map_or_else(|_| role_dir.join("logroot"), PathBuf::from)
}

fn install_child_subscriber(role: &str) -> ChildRuntime {
    let root = out_root().join(role_name(role));
    std::fs::create_dir_all(&root).expect("role dir");
    let probe = HostProbe::default();
    match std::env::var("UC_NLOG_MODE").as_deref() {
        Ok("audit") => {
            let file = std::fs::File::create(root.join("audit.jsonl")).expect("audit file");
            let subscriber = tracing_subscriber::registry().with(AuditLayer {
                out: Mutex::new(file),
            });
            tracing::subscriber::set_global_default(subscriber).expect("subscriber");
            ChildRuntime::new(None, probe)
        }
        Ok("runtime") => {
            let mut config = ObservabilityConfig::new(
                ObservabilityResource::new(
                    "1.1.0",
                    DeploymentEnvironment::Test,
                    OperatingSystem::Other,
                    "test",
                )
                .expect("resource"),
            )
            .with_local_logs(LocalLogConfig::new(root.join("logs")));
            if std::env::var("UC_NLOG_NETWORK").as_deref() == Ok("1") {
                let network_role = if role == "server" {
                    NetworkProcessRole::Daemon
                } else {
                    NetworkProcessRole::MobileApp
                };
                config = config.with_network_logs(
                    NetworkLogConfig::new(log_root(&root), network_role, test_budget())
                        .expect("network log config")
                        .with_known_alpns([
                            String::from_utf8_lossy(SERVED_ALPN).into_owned(),
                            String::from_utf8_lossy(SPACE_ADMISSION_ALPN).into_owned(),
                        ]),
                );
            }
            let host: Vec<HostLogLayer> = vec![Box::new(probe.clone())];
            let outcome = ProcessObservabilityRuntime::install_with_host_layers(config, host)
                .expect("install observability runtime");
            ChildRuntime::new(Some(outcome.handle()), probe)
        }
        other => panic!("unsupported mode {other:?}"),
    }
}

/// 关闭前把本进程的状态、导出摘要和宿主探针结果落成工件。
fn write_child_report(role: &str, runtime: &ChildRuntime, business_span_pinned_ms: Option<u128>) {
    let root = out_root().join(role_name(role));
    let Some(handle) = &runtime.handle else {
        return;
    };
    let _ = ProcessObservabilityRuntime::flush_local_logs(Duration::from_secs(2));
    let status = handle.query_network_log_status().ok();
    let export = handle
        .prepare_network_log_export(Duration::from_secs(2))
        .ok();
    let report = serde_json::json!({
        "network_status": status,
        "network_status_error": handle.query_network_log_status().err().map(|error| error.to_string()),
        "network_export": export,
        "stages": runtime.stages.lock().expect("stages").clone(),
        "host_third_party_events": *runtime.probe.third_party_events.lock().expect("events"),
        "business_span_pinned_ms": business_span_pinned_ms.map(|ms| u64::try_from(ms).unwrap_or(u64::MAX)),
    });
    std::fs::write(
        root.join("report.json"),
        serde_json::to_vec_pretty(&report).expect("report"),
    )
    .expect("write report");
}

async fn run_server() {
    let runtime = install_child_subscriber("server");
    let endpoint = bind_endpoint(vec![SERVED_ALPN.to_vec(), SPACE_ADMISSION_ALPN.to_vec()]).await;
    let router = Router::builder(endpoint.clone())
        .accept(SERVED_ALPN, Hold)
        .accept(SPACE_ADMISSION_ALPN, Hold)
        .spawn();
    let addr_path = out_root().join("server.addr.json");
    std::fs::write(
        addr_path.with_extension("tmp"),
        serde_json::to_vec(&endpoint.addr()).expect("encode addr"),
    )
    .expect("write addr");
    std::fs::rename(addr_path.with_extension("tmp"), &addr_path).expect("publish addr");
    let stop = out_root().join("server.stop");
    let started = Instant::now();
    while !stop.exists() && started.elapsed() < Duration::from_secs(60) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    write_child_report("server", &runtime, None);
    let _ = router.shutdown().await;
}

/// 业务 span 的调用点由测试手工构造，module_path 不属于 Engine 自有 crate，宿主层才看得见它。
static BUSINESS_CALLSITE: tracing::callsite::DefaultCallsite =
    tracing::callsite::DefaultCallsite::new(&BUSINESS_METADATA);
static BUSINESS_METADATA: tracing::Metadata<'static> = tracing::Metadata::new(
    BUSINESS_SPAN,
    BUSINESS_TARGET,
    tracing::Level::INFO,
    Some("network_log_e2e.rs"),
    Some(1),
    Some("e2e_host_application"),
    tracing::field::FieldSet::new(&[], tracing::callsite::Identifier(&BUSINESS_CALLSITE)),
    tracing::metadata::Kind::SPAN,
);

fn business_span() -> tracing::Span {
    let interest = BUSINESS_CALLSITE.interest();
    if interest.is_never()
        || (interest.is_sometimes()
            && !tracing::dispatcher::get_default(|dispatch| dispatch.enabled(&BUSINESS_METADATA)))
    {
        return tracing::Span::none();
    }
    tracing::Span::new(
        &BUSINESS_METADATA,
        &BUSINESS_METADATA.fields().value_set(&[]),
    )
}

/// 在业务 span 内拨号：拨号返回、span 释放后，业务 span 不能被网络库的后台 driver 继续持有。
fn business_scope<F: std::future::Future>(future: F) -> (Instrumented<F>, tracing::Span) {
    let span = business_span();
    (future.instrument(span.clone()), span)
}

/// 经 Engine 的通用拨号路径建立一条连接，短暂保持后关闭。
async fn dial_once(endpoint: &Arc<Endpoint>, addr: &EndpointAddr) {
    let connection = connect_with_staggered_retry(
        Arc::clone(endpoint),
        addr.clone(),
        SERVED_ALPN,
        "clipboard",
        AddressInputSource::Provided,
    )
    .await
    .expect("dial connects");
    tokio::time::sleep(Duration::from_millis(120)).await;
    connection.close(0u32.into(), b"done");
}

fn own_run_directory(runtime: &ChildRuntime) -> PathBuf {
    let status = runtime
        .handle
        .as_ref()
        .and_then(|handle| handle.query_network_log_status().ok())
        .expect("network log status");
    log_root(&out_root().join(role_name("client")))
        .join("network")
        .join(status.role.as_str())
        .join(status.run_id)
}

async fn run_client(scenario: &str) {
    let runtime = install_child_subscriber("client");
    let addr_path = out_root().join("server.addr.json");
    let addr: EndpointAddr =
        serde_json::from_slice(&std::fs::read(addr_path).expect("server addr")).expect("addr");
    let endpoint = Arc::new(bind_endpoint(Vec::new()).await);
    let mut pinned_ms = None;
    match scenario {
        "connected" | "admission_connected" => {
            let admission = scenario == "admission_connected";
            let dial = {
                let endpoint = Arc::clone(&endpoint);
                let addr = addr.clone();
                async move {
                    if admission {
                        // discarded-source[no-information]: the harness only needs success or failure
                        admission_connect(&endpoint, addr).await.map_err(|_| ())
                    } else {
                        connect_with_staggered_retry(
                            endpoint,
                            addr,
                            SERVED_ALPN,
                            "clipboard",
                            AddressInputSource::Provided,
                        )
                        .await
                        // discarded-source[no-information]: the harness only needs success or failure
                        .map_err(|_| ())
                    }
                }
            };
            let (dial, span) = business_scope(dial);
            let connection = dial.await.expect("dial connects");
            drop(span);
            let dropped_at = Instant::now();
            // 连接保持打开：driver 仍在运行，但不应持有已经结束的业务 span。
            tokio::time::sleep(Duration::from_millis(400)).await;
            pinned_ms = Some(
                match *runtime.probe.business_closed_at.lock().expect("closed") {
                    Some(closed_at) => closed_at.saturating_duration_since(dropped_at).as_millis(),
                    None => u128::MAX,
                },
            );
            connection.close(0u32.into(), b"done");
        }
        "unserved_alpn" => {
            let result = connect_with_staggered_retry(
                Arc::clone(&endpoint),
                addr,
                UNSERVED_ALPN,
                "clipboard",
                AddressInputSource::Provided,
            )
            .await;
            assert!(result.is_err(), "unserved ALPN must fail the handshake");
        }
        "fs_faults" => {
            dial_once(&endpoint, &addr).await;
            runtime.stage("before_fault");
            let run_directory = own_run_directory(&runtime);
            // 目录被外部删除：下一次写入或刷新重建目录并继续。
            std::fs::remove_dir_all(&run_directory).expect("remove run directory");
            dial_once(&endpoint, &addr).await;
            tokio::time::sleep(Duration::from_millis(200)).await;
            runtime.stage("directory_removed");
            // 目录位置被普通文件占住：写入失败、状态不可用，业务拨号不受影响。
            std::fs::remove_dir_all(&run_directory).expect("remove recreated directory");
            std::fs::write(&run_directory, b"blocker").expect("block path");
            dial_once(&endpoint, &addr).await;
            tokio::time::sleep(Duration::from_millis(200)).await;
            runtime.stage("path_blocked");
            // 占位被清除：显式刷新是重试点，立即恢复。
            std::fs::remove_file(&run_directory).expect("unblock path");
            runtime.stage("path_unblocked");
            dial_once(&endpoint, &addr).await;
            tokio::time::sleep(Duration::from_millis(200)).await;
            runtime.stage("recovered");
        }
        "startup_blocked" => {
            // 日志根被普通文件占住：启动时无法建目录，业务拨号照常完成。
            dial_once(&endpoint, &addr).await;
            runtime.stage("blocked_at_start");
            let blocker = std::env::var("UC_NLOG_LOGROOT").expect("blocked log root");
            std::fs::remove_file(blocker).expect("unblock log root");
            // 退避期内到达的记录会被丢弃并计数；显式刷新是重试点，先恢复再产生新记录。
            runtime.stage("unblocked");
            dial_once(&endpoint, &addr).await;
            tokio::time::sleep(Duration::from_millis(200)).await;
            runtime.stage("recovered");
        }
        "burst" => {
            let dials = (0..30).map(|_| dial_once(&endpoint, &addr));
            futures_util::future::join_all(dials).await;
            tokio::time::sleep(Duration::from_millis(300)).await;
            runtime.stage("after_burst");
        }
        "detailed" => {
            let handle = runtime.handle.as_ref().expect("runtime");
            runtime.stage("standard_before");
            let first = handle
                .start_network_capture(Duration::from_secs(3))
                .expect("start capture");
            let second = handle
                .start_network_capture(Duration::from_secs(900))
                .expect("repeat start");
            let bogus = handle.stop_network_capture("not-the-capture-id");
            let too_long = handle.start_network_capture(Duration::from_secs(901));
            std::fs::write(
                out_root().join(role_name("client")).join("capture.json"),
                serde_json::to_vec_pretty(&serde_json::json!({
                    "first": first,
                    "second": second,
                    "bogus_stop": bogus.err().map(|error| error.to_string()),
                    "too_long": too_long.err().map(|error| error.to_string()),
                }))
                .expect("capture json"),
            )
            .expect("write capture.json");
            dial_once(&endpoint, &addr).await;
            runtime.stage("detailed_open");
            tokio::time::sleep(Duration::from_millis(3_300)).await;
            runtime.stage("detailed_expired");
            dial_once(&endpoint, &addr).await;
            tokio::time::sleep(Duration::from_millis(200)).await;
            runtime.stage("after_expiry");
        }
        "hold" => {
            dial_once(&endpoint, &addr).await;
            runtime.stage("holding");
            let run_id = runtime
                .handle
                .as_ref()
                .and_then(|handle| handle.query_network_log_status().ok())
                .map(|status| status.run_id)
                .expect("run id");
            std::fs::write(out_root().join(role_name("client")).join("holding"), run_id)
                .expect("holding marker");
            let release = out_root().join(format!("{}.release", role_name("client")));
            let started = Instant::now();
            while !release.exists() && started.elapsed() < Duration::from_secs(60) {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
        other => panic!("unknown scenario {other}"),
    }
    // 给后台 driver 与关闭事件留出时间进入 subscriber。
    tokio::time::sleep(Duration::from_millis(500)).await;
    write_child_report("client", &runtime, pinned_ms);
    endpoint.close().await;
}

/// 子进程入口：不设角色环境变量时立即返回，因此普通测试运行不受影响。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn child_process() {
    match std::env::var("UC_NLOG_ROLE").as_deref() {
        Ok("server") => run_server().await,
        Ok("client") => {
            let scenario = std::env::var("UC_NLOG_SCENARIO").expect("scenario");
            run_client(&scenario).await;
        }
        _ => {}
    }
}

struct Run<'a> {
    out: &'a Path,
    scenario: &'a str,
    mode: &'a str,
    network: bool,
    extra_env: &'a [(&'a str, String)],
}

fn spawn_child(run: &Run<'_>, role: &str) -> Child {
    let test_name = format!(
        "{}::child_process",
        module_path!().split_once("::").map_or("", |(_, rest)| rest)
    );
    let mut command = Command::new(std::env::current_exe().expect("test executable"));
    command
        .args([
            test_name.as_str(),
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("UC_NLOG_ROLE", role)
        .env("UC_NLOG_SCENARIO", run.scenario)
        .env("UC_NLOG_MODE", run.mode)
        .env("UC_NLOG_NETWORK", if run.network { "1" } else { "0" })
        .env("UC_NLOG_OUT", run.out)
        .stdout(Stdio::from(
            std::fs::File::create(run.out.join(format!("{role}.stdout.txt"))).expect("stdout log"),
        ))
        .stderr(Stdio::from(
            std::fs::File::create(run.out.join(format!("{role}.stderr.txt"))).expect("stderr log"),
        ));
    for (name, value) in run.extra_env {
        command.env(name, value);
    }
    command.spawn().expect("spawn child")
}

fn wait_for(path: &Path, limit: Duration) {
    let started = Instant::now();
    while !path.exists() {
        assert!(
            started.elapsed() < limit,
            "{} never appeared",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// 在一个干净的工件目录里跑一个场景：服务端先起，客户端执行场景，随后停止服务端。
fn run_scenario(run: &Run<'_>) {
    // 工件目录可能是上一次留下的：先清空，保证每个角色只有本次运行的目录。
    let _ = std::fs::remove_dir_all(run.out);
    std::fs::create_dir_all(run.out).expect("scenario dir");
    let mut server = spawn_child(run, "server");
    wait_for(&run.out.join("server.addr.json"), Duration::from_secs(20));
    let mut client = spawn_child(run, "client");
    let client_status = client.wait().expect("client exit");
    std::fs::write(run.out.join("server.stop"), b"").expect("stop server");
    let server_status = server.wait().expect("server exit");
    assert!(client_status.success(), "client child failed");
    assert!(server_status.success(), "server child failed");
}

#[test]
#[ignore = "审查工具：把第三方事件元数据写入 UC_NETWORK_LOG_E2E_OUT"]
fn audit_third_party_events() {
    let root =
        PathBuf::from(std::env::var("UC_NETWORK_LOG_E2E_OUT").expect("UC_NETWORK_LOG_E2E_OUT"));
    for scenario in ["connected", "unserved_alpn", "admission_connected"] {
        run_scenario(&Run {
            out: &root.join(scenario),
            scenario,
            mode: "audit",
            network: false,
            extra_env: &[],
        });
    }
}

// ---------- 工件读取与断言 ----------

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).expect("read json")).expect("parse json")
}

fn jsonl(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .expect("read jsonl")
        .lines()
        .map(|line| serde_json::from_str(line).expect("jsonl line"))
        .collect()
}

/// 某个角色的网络日志运行目录（每个进程只有一个）。
fn network_run_directory(out: &Path, role: &str) -> PathBuf {
    let network = out.join(role).join("logroot").join("network");
    let mut runs = Vec::new();
    for role_dir in std::fs::read_dir(&network).expect("network dir").flatten() {
        if !role_dir.path().is_dir() {
            continue;
        }
        for run in std::fs::read_dir(role_dir.path())
            .expect("role dir")
            .flatten()
        {
            if run.path().is_dir() {
                runs.push(run.path());
            }
        }
    }
    assert_eq!(runs.len(), 1, "{role} must own exactly one run directory");
    runs.remove(0)
}

fn network_records(run: &Path) -> Vec<Value> {
    let mut names: Vec<_> = std::fs::read_dir(run)
        .expect("run dir")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("network.") && name.ends_with(".jsonl"))
        .collect();
    names.sort();
    names
        .iter()
        .flat_map(|name| jsonl(&run.join(name)))
        .collect()
}

fn business_records(out: &Path, role: &str) -> Vec<Value> {
    let logs = out.join(role).join("logs");
    let mut files: Vec<_> = std::fs::read_dir(&logs)
        .expect("logs dir")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("engine.") && name.ends_with(".jsonl"))
        })
        .collect();
    files.sort();
    files.iter().flat_map(|path| jsonl(path)).collect()
}

fn events(records: &[Value]) -> Vec<&str> {
    records
        .iter()
        .filter_map(|record| record["event"].as_str())
        .collect()
}

fn has_event(records: &[Value], event: &str, field: &str, value: &str) -> bool {
    records
        .iter()
        .any(|record| record["event"] == event && record["f"][field].as_str() == Some(value))
}

/// 把服务端地址 JSON 里所有字符串收集为“不应出现在网络日志里”的金丝雀。
fn address_canaries(addr: &Value, into: &mut Vec<String>) {
    match addr {
        Value::String(text) => into.push(text.clone()),
        Value::Array(items) => items.iter().for_each(|item| address_canaries(item, into)),
        Value::Object(map) => map.values().for_each(|item| address_canaries(item, into)),
        _ => {}
    }
}

fn contains_ipv4(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index].is_ascii_digit() {
            let start = index;
            while index < bytes.len() && (bytes[index].is_ascii_digit() || bytes[index] == b'.') {
                index += 1;
            }
            let candidate = &text[start..index];
            if candidate.matches('.').count() >= 3
                && candidate
                    .split('.')
                    .filter(|part| !part.is_empty() && part.len() <= 3)
                    .count()
                    >= 4
            {
                return true;
            }
        } else {
            index += 1;
        }
    }
    false
}

fn contains_ipv6(text: &str) -> bool {
    let mut run = 0;
    let mut colons = 0;
    for byte in text.bytes() {
        if byte.is_ascii_hexdigit() || byte == b':' {
            run += 1;
            colons += usize::from(byte == b':');
            if colons >= 4 && run >= 7 {
                return true;
            }
        } else {
            run = 0;
            colons = 0;
        }
    }
    false
}

/// 网络日志不得含地址、端点标识、测试金丝雀 ALPN 或任何 IP 字面量。
fn assert_no_leak(label: &str, run: &Path, canaries: &[String]) {
    let mut files: Vec<_> = std::fs::read_dir(run)
        .expect("run dir")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .collect();
    files.sort();
    assert!(!files.is_empty());
    for path in files {
        let text = std::fs::read_to_string(&path).expect("read file");
        let name = path
            .file_name()
            .expect("name")
            .to_string_lossy()
            .into_owned();
        for canary in canaries {
            assert!(
                canary.len() < 6 || !text.contains(canary.as_str()),
                "{label}/{name} leaks a canary value"
            );
        }
        assert!(
            !text.contains("unserved-canary"),
            "{label}/{name} leaks the ALPN canary"
        );
        if name != "run.json" {
            assert!(
                !contains_ipv4(&text),
                "{label}/{name} contains an IPv4 literal"
            );
            assert!(
                !contains_ipv6(&text),
                "{label}/{name} contains an IPv6 literal"
            );
        }
    }
}

/// 业务日志的签名：目标、级别与事件名的集合（计数会随重试时序变化）。
fn business_signature(records: &[Value]) -> std::collections::BTreeSet<String> {
    records
        .iter()
        .map(|record| {
            format!(
                "{}|{}|{}",
                record["target"].as_str().unwrap_or_default(),
                record["level"].as_str().unwrap_or_default(),
                record["fields"]["event.name"].as_str().unwrap_or_default()
            )
        })
        .collect()
}

fn artifact_root(name: &str) -> (PathBuf, Option<tempfile::TempDir>) {
    match std::env::var("UC_NETWORK_LOG_E2E_OUT") {
        Ok(path) => {
            let root = PathBuf::from(path).join(name);
            std::fs::create_dir_all(&root).expect("artifact dir");
            (root, None)
        }
        Err(_) => {
            let temporary = tempfile::tempdir().expect("temp artifact dir");
            (temporary.path().to_path_buf(), Some(temporary))
        }
    }
}

fn run_with_network(root: &Path, scenario: &str) -> (PathBuf, Vec<String>) {
    let out = root.join(scenario);
    run_scenario(&Run {
        out: &out,
        scenario,
        mode: "runtime",
        network: true,
        extra_env: &[],
    });
    let mut canaries = Vec::new();
    address_canaries(&read_json(&out.join("server.addr.json")), &mut canaries);
    (out, canaries)
}

fn assert_common(label: &str, out: &Path, canaries: &[String]) {
    for role in ["client", "server"] {
        let run = network_run_directory(out, role);
        assert_no_leak(&format!("{label}/{role}"), &run, canaries);
        let report = read_json(&out.join(role).join("report.json"));
        // 宿主输出（桌面控制台、Sentry 等）收不到第三方网络事件。
        assert_eq!(report["host_third_party_events"], 0, "{label}/{role}");
        let status = &report["network_status"];
        assert_eq!(status["state"], "ready", "{label}/{role}");
        let counters = &status["counters"];
        assert_eq!(counters["queue_dropped"], 0, "{label}/{role}");
        assert_eq!(counters["write_failed"], 0, "{label}/{role}");
        assert_eq!(counters["oversized"], 0, "{label}/{role}");
        assert_eq!(counters["accepted"], counters["written"], "{label}/{role}");
        assert_eq!(
            report["network_export"]["flush"], "completed",
            "{label}/{role}"
        );
        // 业务日志里没有任何第三方网络事件或网络作用域。
        for record in business_records(out, role) {
            let target = record["target"].as_str().unwrap_or_default();
            assert!(
                !["iroh", "noq", "uc.network"]
                    .iter()
                    .any(|prefix| target.starts_with(prefix)),
                "{label}/{role}: business log carries network target {target}"
            );
        }
        // 导出摘要里的 SHA-256 与磁盘文件一致。
        for file in report["network_export"]["files"].as_array().expect("files") {
            let name = file["name"].as_str().expect("name");
            let bytes = std::fs::read(run.join(name)).expect("segment");
            use sha2::{Digest, Sha256};
            let digest: String = Sha256::digest(&bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            assert_eq!(file["sha256"], digest.as_str(), "{label}/{role}/{name}");
        }
    }
}

#[test]
fn two_process_connection_produces_isolated_privacy_safe_network_logs() {
    let (root, _guard) = artifact_root("connection");
    let (out, canaries) = run_with_network(&root, "connected");
    assert_common("connected", &out, &canaries);

    let client = network_records(&network_run_directory(&out, "client"));
    let server = network_records(&network_run_directory(&out, "server"));
    // 通用拨号路径（原本整条静默）现在有第三方事件。
    assert!(
        has_event(
            &client,
            "connection.connecting",
            "alpn",
            "uc-network-log/e2e/served/1"
        ),
        "{:?}",
        events(&client)
    );
    assert!(
        has_event(&client, "connection.connected", "side", "Client"),
        "{:?}",
        events(&client)
    );
    assert!(
        has_event(&server, "connection.incoming", "addr_kind", "ip_v4")
            || has_event(&server, "connection.incoming", "addr_kind", "ip_v6"),
        "{:?}",
        events(&server)
    );
    assert!(
        has_event(&server, "connection.connected", "side", "Server"),
        "{:?}",
        events(&server)
    );

    // 业务 span 在拨号返回后按时结束，没有被网络 driver 持有。
    let report = read_json(&out.join("client").join("report.json"));
    let pinned = report["business_span_pinned_ms"]
        .as_u64()
        .expect("pinned ms");
    assert!(
        pinned < 200,
        "business span stayed open {pinned} ms after the dial returned"
    );

    // 同一流程在关闭网络日志时，业务记录的种类不变。
    let baseline = root.join("connected_baseline");
    run_scenario(&Run {
        out: &baseline,
        scenario: "connected",
        mode: "runtime",
        network: false,
        extra_env: &[],
    });
    assert_eq!(
        business_signature(&business_records(&out, "client")),
        business_signature(&business_records(&baseline, "client")),
        "enabling network logging changed the business log"
    );
    assert!(
        !baseline.join("client").join("logroot").exists(),
        "a disabled network log must not create any directory"
    );
}

#[test]
fn rejected_alpn_is_classified_on_the_server_without_the_peer_string() {
    let (root, _guard) = artifact_root("rejected-alpn");
    let (out, canaries) = run_with_network(&root, "unserved_alpn");
    assert_common("unserved_alpn", &out, &canaries);
    let client = network_records(&network_run_directory(&out, "client"));
    let server = network_records(&network_run_directory(&out, "server"));
    assert!(
        has_event(&client, "connection.connecting", "alpn", "unrecognized"),
        "{:?}",
        events(&client)
    );
    assert!(
        has_event(
            &server,
            "router.accepting_failed",
            "reason",
            "no_application_protocol"
        ),
        "{:?}",
        events(&server)
    );
    assert!(
        !events(&server).contains(&"connection.connected"),
        "{:?}",
        events(&server)
    );
}

#[test]
fn admission_dial_path_is_no_longer_silent() {
    let (root, _guard) = artifact_root("admission-dial");
    let (out, canaries) = run_with_network(&root, "admission_connected");
    assert_common("admission_connected", &out, &canaries);
    let client = network_records(&network_run_directory(&out, "client"));
    assert!(
        has_event(
            &client,
            "connection.connected",
            "alpn",
            "/uniclipboard/space-admission/1"
        ),
        "{:?}",
        events(&client)
    );
    let report = read_json(&out.join("client").join("report.json"));
    let pinned = report["business_span_pinned_ms"]
        .as_u64()
        .expect("pinned ms");
    assert!(
        pinned < 200,
        "business span stayed open {pinned} ms after the admission dial returned"
    );
}

// ---------- 故障、预算、限时采集与多进程 ----------

fn stage<'a>(report: &'a Value, name: &str) -> &'a Value {
    report["stages"]
        .as_array()
        .and_then(|stages| stages.iter().find(|stage| stage["name"] == name))
        .unwrap_or_else(|| panic!("stage {name} missing from {:?}", report["stages"]))
}

fn counter(stage: &Value, name: &str) -> u64 {
    stage["status"]["counters"][name]
        .as_u64()
        .unwrap_or_else(|| panic!("counter {name} missing in {:?}", stage["status"]))
}

fn client_report(out: &Path, name: &str) -> Value {
    read_json(&out.join(name).join("report.json"))
}

fn assert_business_has_no_network(out: &Path, name: &str) {
    for record in business_records(out, name) {
        let target = record["target"].as_str().unwrap_or_default();
        assert!(
            !["iroh", "noq", "uc.network"]
                .iter()
                .any(|prefix| target.starts_with(prefix)),
            "{name}: business log carries network target {target}"
        );
    }
}

#[test]
fn deleted_and_blocked_run_directories_recover_without_touching_business_logs() {
    let (root, _guard) = artifact_root("fs-faults");
    let out = root.join("fs_faults");
    run_scenario(&Run {
        out: &out,
        scenario: "fs_faults",
        mode: "runtime",
        network: true,
        extra_env: &[],
    });
    let report = client_report(&out, "client");
    let before = stage(&report, "before_fault");
    assert_eq!(before["status"]["state"], "ready");
    assert!(counter(before, "written") > 0);

    // 目录被删除：下一次写入重建目录，计数记录恢复次数，数据继续写入。
    let removed = stage(&report, "directory_removed");
    assert_eq!(removed["flush"], "completed");
    assert_eq!(removed["status"]["state"], "ready");
    assert!(counter(removed, "path_recovered") >= 1);
    assert!(counter(removed, "written") > counter(before, "written"));

    // 路径被文件占住：写入失败并计数，状态不可用，刷新报告失败。
    let blocked = stage(&report, "path_blocked");
    assert_eq!(blocked["status"]["state"], "unavailable");
    assert_ne!(blocked["flush"], "completed");
    assert!(counter(blocked, "write_failed") > 0);

    // 占位清除后，显式刷新就是重试点：立即恢复。
    let unblocked = stage(&report, "path_unblocked");
    assert_eq!(unblocked["flush"], "completed");
    assert_eq!(unblocked["status"]["state"], "ready");
    let recovered = stage(&report, "recovered");
    assert_eq!(recovered["status"]["state"], "ready");
    assert!(counter(recovered, "written") > counter(unblocked, "written"));

    // 恢复后的运行目录里每一行都完整，序号单调递增，且没有被业务日志污染。
    let run = network_run_directory(&out, "client");
    let records = network_records(&run);
    assert!(!records.is_empty());
    let sequences: Vec<u64> = records
        .iter()
        .map(|record| record["seq"].as_u64().expect("seq"))
        .collect();
    assert!(
        sequences.windows(2).all(|pair| pair[0] < pair[1]),
        "{sequences:?}"
    );
    assert_business_has_no_network(&out, "client");
    // 业务日志在整个故障期间保持可写。
    assert!(!business_records(&out, "client").is_empty());
}

#[test]
fn log_root_that_cannot_be_created_degrades_only_the_network_log_and_recovers() {
    let (root, _guard) = artifact_root("startup-blocked");
    let out = root.join("startup_blocked");
    let blocker = root.join("blocked-log-root");
    let _ = std::fs::remove_dir_all(&blocker);
    let _ = std::fs::remove_file(&blocker);
    std::fs::write(&blocker, b"not a directory").expect("blocker");
    run_scenario(&Run {
        out: &out,
        scenario: "startup_blocked",
        mode: "runtime",
        network: true,
        extra_env: &[("UC_NLOG_LOGROOT", blocker.display().to_string())],
    });
    let report = client_report(&out, "client");
    let blocked = stage(&report, "blocked_at_start");
    assert_eq!(blocked["status"]["state"], "unavailable");
    assert_ne!(blocked["flush"], "completed");
    // 启动失败不是永久失败：根目录可用后，同一个运行恢复。
    let recovered = stage(&report, "recovered");
    assert_eq!(recovered["status"]["state"], "ready");
    assert_eq!(recovered["flush"], "completed");
    assert!(counter(recovered, "written") > 0);
    // 业务日志与业务拨号全程不受影响。
    assert!(!business_records(&out, "client").is_empty());
    assert_business_has_no_network(&out, "client");
}

#[test]
fn saturated_queue_and_rate_limits_drop_with_counts_and_keep_files_intact() {
    let (root, _guard) = artifact_root("burst");
    let out = root.join("burst");
    run_scenario(&Run {
        out: &out,
        scenario: "burst",
        mode: "runtime",
        network: true,
        extra_env: &[("UC_NLOG_QUEUE_RECORDS", "1".to_owned())],
    });
    let report = client_report(&out, "client");
    let burst = stage(&report, "after_burst");
    assert_eq!(burst["status"]["state"], "ready");
    // 丢弃必须有计数：被限速或队列满的记录都不是静默消失。
    assert!(
        counter(burst, "queue_dropped") + counter(burst, "rate_limited") > 0,
        "{:?}",
        burst["status"]["counters"]
    );
    assert_eq!(counter(burst, "accepted"), counter(burst, "written"));
    let run = network_run_directory(&out, "client");
    let records = network_records(&run);
    assert_eq!(records.len() as u64, counter(burst, "written"));
    assert_business_has_no_network(&out, "client");
}

#[test]
fn small_run_budget_rotates_out_the_oldest_segments_only() {
    let (root, _guard) = artifact_root("budget");
    let out = root.join("budget");
    run_scenario(&Run {
        out: &out,
        scenario: "burst",
        mode: "runtime",
        network: true,
        extra_env: &[
            ("UC_NLOG_FILE_BYTES", "8192".to_owned()),
            ("UC_NLOG_RUN_BYTES", "16384".to_owned()),
        ],
    });
    let report = client_report(&out, "client");
    let burst = stage(&report, "after_burst");
    let run = network_run_directory(&out, "client");
    let mut sizes = Vec::new();
    for entry in std::fs::read_dir(&run).expect("run dir").flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("network.") {
            sizes.push(entry.metadata().expect("metadata").len());
        }
    }
    assert!(sizes.iter().all(|size| *size <= 8192), "{sizes:?}");
    assert!(sizes.iter().sum::<u64>() <= 16384, "{sizes:?}");
    assert!(
        counter(burst, "rotated_out_files") > 0,
        "{:?}",
        burst["status"]["counters"]
    );
    // 每个保留下来的分段都是完整的行，序号在保留的尾部里连续递增。
    let records = network_records(&run);
    let sequences: Vec<u64> = records
        .iter()
        .map(|record| record["seq"].as_u64().expect("seq"))
        .collect();
    assert!(
        sequences.windows(2).all(|pair| pair[0] < pair[1]),
        "{sequences:?}"
    );
    assert!(
        sequences[0] > 0,
        "the oldest records must have been rotated out"
    );
}

#[test]
fn detailed_capture_is_bounded_independent_and_not_extendable() {
    let (root, _guard) = artifact_root("detailed");
    let out = root.join("detailed");
    run_scenario(&Run {
        out: &out,
        scenario: "detailed",
        mode: "runtime",
        network: true,
        extra_env: &[],
    });
    let capture = read_json(&out.join("client").join("capture.json"));
    assert_eq!(capture["first"]["already_active"], false);
    assert_eq!(capture["second"]["already_active"], true);
    assert_eq!(
        capture["second"]["capture_id"],
        capture["first"]["capture_id"]
    );
    assert_eq!(
        capture["second"]["duration_ms"], 3000,
        "repeat start must not extend"
    );
    assert!(capture["bogus_stop"]
        .as_str()
        .is_some_and(|text| text.contains("does not match")));
    assert!(capture["too_long"]
        .as_str()
        .is_some_and(|text| text.contains("between 1 and 900")));

    let report = client_report(&out, "client");
    let standard = stage(&report, "standard_before");
    assert_eq!(standard["status"]["mode"], "standard");
    let open = stage(&report, "detailed_open");
    assert_eq!(open["status"]["mode"], "detailed");
    assert!(open["status"]["detailed_remaining_ms"]
        .as_u64()
        .is_some_and(|ms| ms > 0));
    let expired = stage(&report, "detailed_expired");
    assert_eq!(expired["status"]["mode"], "standard");
    assert!(expired["status"]["detailed_remaining_ms"].is_null());

    // 详细档的记录只出现在窗口内；窗口之外的拨号只留下标准档记录。
    let records = network_records(&network_run_directory(&out, "client"));
    let started = chrono::DateTime::parse_from_rfc3339(
        capture["first"]["started_at_utc"].as_str().expect("start"),
    )
    .expect("start time");
    let ended = started + chrono::Duration::milliseconds(3_000 + 400);
    let detailed: Vec<_> = records.iter().filter(|r| r["tier"] == "detailed").collect();
    assert!(!detailed.is_empty(), "{:?}", events(&records));
    for record in &detailed {
        let at =
            chrono::DateTime::parse_from_rfc3339(record["ts"].as_str().expect("ts")).expect("ts");
        assert!(
            at >= started && at <= ended,
            "detailed record outside the window"
        );
    }
    assert!(
        records.iter().any(|r| r["tier"] == "standard" && {
            let at =
                chrono::DateTime::parse_from_rfc3339(r["ts"].as_str().expect("ts")).expect("ts");
            at > ended
        }),
        "the dial after expiry must still leave standard records"
    );
    assert_no_leak("detailed", &network_run_directory(&out, "client"), &[]);
}

#[test]
fn standard_mode_never_writes_detailed_records_and_counts_what_it_filtered() {
    let (root, _guard) = artifact_root("standard-tier");
    let (out, _canaries) = run_with_network(&root, "connected");
    let report = client_report(&out, "client");
    assert!(report["network_status"]["counters"]["policy_filtered"]
        .as_u64()
        .is_some_and(|n| n > 0));
    for role in ["client", "server"] {
        let records = network_records(&network_run_directory(&out, role));
        assert!(records.iter().all(|r| r["tier"] == "standard"), "{role}");
    }
}

#[test]
fn unconfigured_network_log_reports_not_configured_instead_of_zero_activity() {
    let (root, _guard) = artifact_root("unconfigured");
    let out = root.join("connected");
    run_scenario(&Run {
        out: &out,
        scenario: "connected",
        mode: "runtime",
        network: false,
        extra_env: &[],
    });
    let report = client_report(&out, "client");
    assert!(report["network_status"].is_null());
    assert!(report["network_status_error"]
        .as_str()
        .is_some_and(|text| text.contains("not configured")));
}

/// 同一服务端下启动一个命名客户端子进程。
fn spawn_named_client(run: &Run<'_>, name: &str, extra: &[(&str, String)]) -> Child {
    let test_name = format!(
        "{}::child_process",
        module_path!().split_once("::").map_or("", |(_, rest)| rest)
    );
    let mut command = Command::new(std::env::current_exe().expect("test executable"));
    command
        .args([
            test_name.as_str(),
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("UC_NLOG_ROLE", "client")
        .env("UC_NLOG_CLIENT_NAME", name)
        .env("UC_NLOG_SCENARIO", run.scenario)
        .env("UC_NLOG_MODE", "runtime")
        .env("UC_NLOG_NETWORK", "1")
        .env("UC_NLOG_OUT", run.out)
        .stdout(Stdio::from(
            std::fs::File::create(run.out.join(format!("{name}.stdout.txt"))).expect("stdout"),
        ))
        .stderr(Stdio::from(
            std::fs::File::create(run.out.join(format!("{name}.stderr.txt"))).expect("stderr"),
        ));
    for (key, value) in run.extra_env.iter().chain(extra.iter()) {
        command.env(key, value);
    }
    command.spawn().expect("spawn named client")
}

fn run_directories(shared: &Path) -> Vec<PathBuf> {
    let mut runs = Vec::new();
    let Ok(roles) = std::fs::read_dir(shared.join("network")) else {
        return runs;
    };
    for role in roles.flatten().filter(|entry| entry.path().is_dir()) {
        for run in std::fs::read_dir(role.path()).expect("role").flatten() {
            if run.path().is_dir() {
                runs.push(run.path());
            }
        }
    }
    runs.sort();
    runs
}

#[test]
fn concurrent_processes_own_separate_runs_and_collection_never_removes_an_active_run() {
    let (root, _guard) = artifact_root("multi-process");
    let out = root.join("gc");
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).expect("out");
    let shared = out.join("shared-logroot");
    let env = vec![
        ("UC_NLOG_LOGROOT", shared.display().to_string()),
        ("UC_NLOG_FILE_BYTES", "8192".to_owned()),
        ("UC_NLOG_RUN_BYTES", "16384".to_owned()),
        ("UC_NLOG_ROOT_BYTES", "1000000".to_owned()),
        ("UC_NLOG_RETENTION_SECS", "1".to_owned()),
    ];
    let server_run = Run {
        out: &out,
        scenario: "hold",
        mode: "runtime",
        network: true,
        extra_env: &env,
    };
    let mut server = spawn_child(&server_run, "server");
    wait_for(&out.join("server.addr.json"), Duration::from_secs(20));

    // A 长期持有运行；B 与 A 同时存在，各自独占一个目录。
    let mut a = spawn_named_client(&server_run, "a", &[]);
    wait_for(&out.join("a").join("holding"), Duration::from_secs(20));
    std::thread::sleep(Duration::from_millis(1_600));
    let one_shot = Run {
        scenario: "connected",
        ..Run {
            out: &out,
            scenario: "connected",
            mode: "runtime",
            network: true,
            extra_env: &env,
        }
    };
    let mut b = spawn_named_client(&one_shot, "b", &[]);
    assert!(b.wait().expect("b").success());
    let during = run_directories(&shared);
    let a_run = jsonl_first_run_id(&out, "a");
    let b_run = jsonl_first_run_id(&out, "b");
    assert_ne!(a_run, b_run, "concurrent processes must not share a run");
    // A 的记录已经过了保留期，但它的 lease 仍被持有：B 的启动回收不能删它。
    assert!(
        during.iter().any(|path| path.ends_with(&a_run)),
        "an active run was removed by another process: {during:?}"
    );
    assert!(during.iter().any(|path| path.ends_with(&b_run)));

    // A 结束后，它和 B 都成为可回收的旧运行；C 的启动回收把它们删掉，自己的目录保留。
    std::fs::write(out.join("a.release"), b"").expect("release a");
    assert!(a.wait().expect("a").success());
    std::thread::sleep(Duration::from_millis(1_600));
    let mut c = spawn_named_client(&one_shot, "c", &[]);
    assert!(c.wait().expect("c").success());
    let after = run_directories(&shared);
    let c_run = jsonl_first_run_id(&out, "c");
    assert!(after.iter().any(|path| path.ends_with(&c_run)), "{after:?}");
    assert!(
        !after
            .iter()
            .any(|path| path.ends_with(&a_run) || path.ends_with(&b_run)),
        "inactive expired runs must be collected: {after:?}"
    );

    std::fs::write(out.join("server.stop"), b"").expect("stop server");
    assert!(server.wait().expect("server").success());
}

/// 客户端自己报告的运行标识（来自状态，不依赖目录名）；仍在持有的客户端用它写下的标记文件。
fn jsonl_first_run_id(out: &Path, name: &str) -> String {
    if let Ok(marker) = std::fs::read_to_string(out.join(name).join("holding")) {
        return marker;
    }
    client_report(out, name)["network_status"]["run_id"]
        .as_str()
        .expect("run id")
        .to_owned()
}
