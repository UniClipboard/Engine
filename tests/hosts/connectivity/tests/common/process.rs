//! 两类协议共用独立进程、配对与固定宿主夹具。

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

pub(crate) const WAIT_TIMEOUT: Duration = Duration::from_secs(30);
const EXPIRES_AT_MS: i64 = 2_000_000_000_000;

#[derive(Default)]
struct TicketState {
    tickets: HashMap<String, String>,
}

type TicketVault = Arc<Mutex<TicketState>>;

struct CreatePairing(TicketVault);

impl Respond for CreatePairing {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).expect("pairing create JSON");
        let ticket = body["sponsorTicket"]
            .as_str()
            .expect("sponsor ticket")
            .to_owned();
        let mut state = lock_ticket_vault(&self.0);
        state.tickets.insert(ticket.clone(), ticket.clone());
        ResponseTemplate::new(200).set_body_json(json!({
            "code": ticket,
            "expiresAtMs": EXPIRES_AT_MS,
        }))
    }
}

struct ResolvePairing(TicketVault);

impl Respond for ResolvePairing {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).expect("pairing resolve JSON");
        let code = body["code"].as_str().expect("pairing code");
        let state = lock_ticket_vault(&self.0);
        let ticket = state.tickets.get(code).cloned().expect("registered ticket");
        ResponseTemplate::new(200).set_body_json(json!({
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

pub(crate) async fn mount_rendezvous() -> MockServer {
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

pub(crate) struct HostProcess {
    child: Child,
    input: ChildStdin,
    replies: Receiver<Value>,
    reader: Option<JoinHandle<()>>,
    pub(crate) root: TempDir,
    pub(crate) host_info: Value,
    rendezvous: String,
    binary: PathBuf,
    legacy_group_updates: bool,
    bind_port: Option<u16>,
}

impl HostProcess {
    pub(crate) fn start(rendezvous: &str) -> Self {
        Self::start_binary(
            rendezvous,
            PathBuf::from(env!("CARGO_BIN_EXE_uc-connectivity-host")),
            false,
            None,
        )
    }
    pub(crate) fn start_binary(
        rendezvous: &str,
        binary: PathBuf,
        legacy_group_updates: bool,
        bind_port: Option<u16>,
    ) -> Self {
        let root = TempDir::new().expect("host root");
        let (child, input, replies, reader, host_info) = Self::spawn(
            root.path(),
            rendezvous,
            None,
            &binary,
            legacy_group_updates,
            bind_port,
        );
        let mut host = Self {
            child,
            input,
            replies,
            reader: Some(reader),
            root,
            host_info,
            rendezvous: rendezvous.to_owned(),
            binary,
            legacy_group_updates,
            bind_port,
        };
        if let Ok(url) = std::env::var("UC_MEMBERSHIP_HISTORY_TEST_RELAY") {
            host.request(json!({"command":"relay_config", "url":url}));
            // 与隔离网络宿主相同：持久化 relay 设置后重启，实际 Endpoint 使用新配置。
            host.restart();
            let deadline = Instant::now() + WAIT_TIMEOUT;
            loop {
                host.request(json!({"command":"flush"}));
                if relay_connected(host.root.path()) {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "local relay must connect before issuing an invitation"
                );
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        host
    }

    fn spawn(
        root: &std::path::Path,
        rendezvous: &str,
        secure_storage: Option<Value>,
        binary: &PathBuf,
        legacy_group_updates: bool,
        bind_port: Option<u16>,
    ) -> (Child, ChildStdin, Receiver<Value>, JoinHandle<()>, Value) {
        let (child, input, replies, reader, ready) = spawn_host(
            root,
            rendezvous,
            secure_storage,
            binary,
            legacy_group_updates,
            bind_port,
        );
        assert_eq!(
            ready["ready"], true,
            "host did not start: code={}, state={}",
            ready["code"], ready["startup"]["state"]
        );
        (child, input, replies, reader, ready["host"].clone())
    }

    pub(crate) fn request(&mut self, request: Value) -> Value {
        let deadline = Instant::now() + WAIT_TIMEOUT;
        let command = request["command"].as_str().unwrap_or("missing");
        loop {
            writeln!(self.input, "{request}").expect("write host request");
            self.input.flush().expect("flush host request");
            let response = self.read_response();
            if let Some(ok) = response.get("ok") {
                return ok.clone();
            }
            // runtime 世代切换的 1103 在动作前返回；只读查询的 1211/1212 按原期限等待可用。
            if (response["code"] == 1103
                || (response["code"] == 1211
                    && matches!(command, "eligibility" | "membership_diagnostics"))
                || (response["code"] == 1212 && command == "membership_diagnostics"))
                && matches!(
                    command,
                    "invite"
                        | "eligibility"
                        | "set_membership_history_held"
                        | "arm_group_update_tamper"
                        | "opportunity"
                        | "membership_diagnostics"
                        | "set_network_partition"
                        | "network_endpoint"
                )
                && Instant::now() < deadline
            {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            panic!("host request {command} failed: {response}");
        }
    }

    pub(crate) fn read_response(&mut self) -> Value {
        next_reply(&self.replies)
    }

    #[cfg(feature = "branch-recovery-fault-tests")]
    pub(crate) fn send_without_waiting(&mut self, request: Value) {
        writeln!(self.input, "{request}").expect("write host request");
        self.input.flush().expect("flush host request");
    }

    #[cfg(feature = "branch-recovery-fault-tests")]
    pub(crate) fn has_response_within(&self, timeout: Duration) -> bool {
        match self.replies.recv_timeout(timeout) {
            Ok(_) => true,
            Err(mpsc::RecvTimeoutError::Timeout) => false,
            Err(mpsc::RecvTimeoutError::Disconnected) => panic!("host disconnected"),
        }
    }

    pub(crate) fn shutdown(&mut self) {
        let _ = self.request(json!({ "command": "shutdown" }));
        let status = self.child.wait().expect("wait for host");
        assert!(status.success(), "host exit: {status}");
    }

    pub(crate) fn restart(&mut self) {
        self.restart_with_crash(false);
    }

    pub(crate) fn restart_with_crash(&mut self, crash: bool) {
        let secure_storage = self.request(json!({ "command": "secure_storage" }));
        if crash {
            // 不执行 Engine shutdown；模拟进程丢失，恢复只能依赖已提交的 SQLite 与宿主密钥。
            self.child
                .kill()
                .expect("kill host at durable recovery boundary");
        } else {
            let _ = self.request(json!({ "command": "shutdown" }));
        }
        let status = self.child.wait().expect("wait for stopped host");
        assert_eq!(
            status.success(),
            !crash,
            "host exit before restart: {status}"
        );
        self.join_reader();
        let (child, input, replies, reader, host_info) = Self::spawn(
            self.root.path(),
            &self.rendezvous,
            Some(secure_storage),
            &self.binary,
            self.legacy_group_updates,
            self.bind_port,
        );
        self.child = child;
        self.input = input;
        self.replies = replies;
        self.reader = Some(reader);
        self.host_info = host_info;
    }

    /// 发现验收先让双方全部退出，再换端口启动，排除旧连接和有效旧地址。
    #[allow(dead_code)] // 其他协议测试复用此文件，但不调用发现专属重启。
    pub(crate) fn stop_for_discovery_restart(&mut self) -> Value {
        let storage = self.request(json!({"command": "secure_storage"}));
        self.shutdown();
        self.join_reader();
        storage
    }

    #[allow(dead_code)] // 仅发现对照场景更换双栈端口。
    pub(crate) fn resume_on_port(&mut self, storage: Value, port: u16) {
        self.bind_port = Some(port);
        let (child, input, replies, reader, host_info) = Self::spawn(
            self.root.path(),
            &self.rendezvous,
            Some(storage),
            &self.binary,
            self.legacy_group_updates,
            self.bind_port,
        );
        self.child = child;
        self.input = input;
        self.replies = replies;
        self.reader = Some(reader);
        self.host_info = host_info;
    }

    /// 宿主退出后其 stdout 关闭，读取线程随之结束。
    fn join_reader(&mut self) {
        if let Some(reader) = self.reader.take() {
            reader.join().expect("host output reader");
        }
    }
}

type SpawnedHost = (Child, ChildStdin, Receiver<Value>, JoinHandle<()>, Value);

/// 启动宿主并原样交回首个启动应答，不判定成功与否。
fn spawn_host(
    root: &Path,
    rendezvous: &str,
    secure_storage: Option<Value>,
    binary: &PathBuf,
    legacy_group_updates: bool,
    bind_port: Option<u16>,
) -> SpawnedHost {
    let mut command = Command::new(binary);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    #[cfg(feature = "branch-recovery-fault-tests")]
    command.env(
        "UC_BRANCH_RECOVERY_TEST_GATE",
        root.join("branch-recovery-test-gate"),
    );
    let mut child = command.spawn().expect("start connectivity host");
    let mut input = child.stdin.take().expect("host input");
    let output = child.stdout.take().expect("host output");
    let mut start = json!({
        "root": root,
        "rendezvous": rendezvous,
        "relay": std::env::var_os("UC_MEMBERSHIP_HISTORY_TEST_RELAY").is_some()
            || std::env::var_os("UC_TEST_CLOUD_DISCOVERY").is_some(),
        "legacy_group_updates": legacy_group_updates,
        "bind_port": bind_port,
    });
    if let Some(secure_storage) = secure_storage {
        start["secure_storage"] = secure_storage;
    }
    writeln!(input, "{start}").expect("start host request");
    input.flush().expect("flush start request");
    let (replies, reader) = read_protocol_replies(output);
    let ready = next_reply(&replies);
    (child, input, replies, reader, ready)
}

/// 在调用方提供的 profile 目录上运行的宿主进程。
///
/// 启动应答原样交回（包括失败码）：同一 profile 的多个进程先后或重叠启动时，启动结果本身就是被测对象。
// 只有 profile 启动重叠场景使用；共用本文件的其他测试二进制不构造它。
#[allow(dead_code)]
pub(crate) struct ProfileHost {
    child: Child,
    input: Option<ChildStdin>,
    replies: Receiver<Value>,
    reader: Option<JoinHandle<()>>,
}

#[allow(dead_code)]
#[allow(dead_code)]
impl ProfileHost {
    pub(crate) fn launch(
        root: &Path,
        rendezvous: &str,
        secure_storage: Option<Value>,
    ) -> (Self, Value) {
        let (child, input, replies, reader, ready) = spawn_host(
            root,
            rendezvous,
            secure_storage,
            &PathBuf::from(env!("CARGO_BIN_EXE_uc-connectivity-host")),
            false,
            None,
        );
        let host = Self {
            child,
            input: Some(input),
            replies,
            reader: Some(reader),
        };
        (host, ready)
    }

    /// 只写出指令，不等待应答；用于让关闭与另一进程的启动重叠。
    pub(crate) fn send(&mut self, request: Value) {
        let input = self.input.as_mut().expect("profile host input is open");
        writeln!(input, "{request}").expect("write host request");
        input.flush().expect("flush host request");
    }

    pub(crate) fn reply(&mut self) -> Value {
        next_reply(&self.replies)
    }

    pub(crate) fn request(&mut self, request: Value) -> Value {
        let command = request["command"].as_str().unwrap_or("missing").to_owned();
        self.send(request);
        let response = self.reply();
        match response.get("ok") {
            Some(ok) => ok.clone(),
            None => panic!("host request {command} failed: {response}"),
        }
    }

    /// 等待进程退出；启动失败的宿主会自行退出，已就绪的宿主须先收到 `shutdown`。
    pub(crate) fn wait_exit(mut self) -> std::process::ExitStatus {
        drop(self.input.take());
        let status = self.child.wait().expect("wait for profile host");
        if let Some(reader) = self.reader.take() {
            reader.join().expect("profile host output reader");
        }
        status
    }
}

impl Drop for ProfileHost {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

/// 只采样既有脱敏诊断，确认自定义 relay 已被实际 Endpoint 使用。
pub(crate) fn relay_connected(root: &Path) -> bool {
    std::fs::read_dir(root.join("logs"))
        .expect("host diagnostic directory")
        .filter_map(Result::ok)
        .any(|entry| {
            std::fs::read_to_string(entry.path())
                .unwrap_or_default()
                .lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .any(|row| {
                    row["fields"]["event.name"] == "relay.status.observed"
                        && row["fields"]["connected_count"]
                            .as_u64()
                            .is_some_and(|count| count > 0)
                })
        })
}

impl Drop for HostProcess {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

pub(crate) fn wait_for_phase(host: &mut HostProcess, expected: &str) -> Value {
    let deadline = Instant::now() + WAIT_TIMEOUT;
    loop {
        let status = host.request(json!({ "command": "eligibility" }));
        let update = &status["device_trust"]["space_device_update"];
        if update["phase"] == expected {
            return update.clone();
        }
        assert!(
            Instant::now() < deadline,
            "space device update did not reach {expected}: {update}; deliveries: {:?}",
            status["device_trust"]["devices"]
                .as_array()
                .map(|devices| devices
                    .iter()
                    .filter_map(|device| device
                        .get("group_update")
                        .filter(|update| !update.is_null()))
                    .collect::<Vec<_>>())
        );
        std::thread::yield_now();
    }
}

pub(crate) fn wait_for_active_join(host: &mut HostProcess) {
    let deadline = Instant::now() + WAIT_TIMEOUT;
    loop {
        let choices = host.request(json!({"command": "eligibility"}));
        let status = &choices["device_trust"]["current_join"]["status"];
        if status == "active" {
            return;
        }
        assert!(
            status != "rejected" && status != "terminated" && Instant::now() < deadline,
            "join must complete before membership recovery starts: {status}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

pub(crate) fn join_host(sponsor: &mut HostProcess, joiner: &mut HostProcess, name: &str) {
    let invitation = sponsor.request(json!({"command": "invite"}))["invitation"].clone();
    joiner.request(json!({"command": "join", "name": name, "invitation": invitation}));
    wait_for_active_join(joiner);
}

/// 持续读取宿主 stdout，只把控制应答交给测试。
///
/// 宿主与 Engine 的系统日志共用 stdout（非 Apple 平台的系统输出写 JSON 到 stdout），控制应答以
/// `uc_connectivity` 标记区分。必须持续读取：测试在两次请求之间不读时，日志会写满管道并阻塞宿主。
fn read_protocol_replies(output: ChildStdout) -> (Receiver<Value>, JoinHandle<()>) {
    let (sender, replies) = mpsc::channel();
    let reader = thread::spawn(move || {
        for line in BufReader::new(output).lines() {
            let Ok(line) = line else {
                return;
            };
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if value["uc_connectivity"] == 1 && sender.send(value).is_err() {
                return;
            }
        }
    });
    (replies, reader)
}

fn next_reply(replies: &Receiver<Value>) -> Value {
    replies
        .recv_timeout(WAIT_TIMEOUT)
        .expect("host must reply before the wait deadline")
}
