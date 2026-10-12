use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const WAIT_TIMEOUT: Duration = Duration::from_secs(30);
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

struct HostProcess {
    child: Child,
    input: ChildStdin,
    replies: Receiver<Value>,
    reader: Option<JoinHandle<()>>,
    root: TempDir,
    rendezvous: String,
    binary: PathBuf,
    legacy_group_updates: bool,
}

impl HostProcess {
    fn start(rendezvous: &str) -> Self {
        Self::start_binary(
            rendezvous,
            PathBuf::from(env!("CARGO_BIN_EXE_uc-connectivity-host")),
            false,
        )
    }
    fn start_binary(rendezvous: &str, binary: PathBuf, legacy_group_updates: bool) -> Self {
        let root = TempDir::new().expect("host root");
        let (child, input, replies, reader) =
            Self::spawn(root.path(), rendezvous, None, &binary, legacy_group_updates);
        Self {
            child,
            input,
            replies,
            reader: Some(reader),
            root,
            rendezvous: rendezvous.to_owned(),
            binary,
            legacy_group_updates,
        }
    }

    fn spawn(
        root: &std::path::Path,
        rendezvous: &str,
        secure_storage: Option<Value>,
        binary: &PathBuf,
        legacy_group_updates: bool,
    ) -> (Child, ChildStdin, Receiver<Value>, JoinHandle<()>) {
        let mut child = Command::new(binary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("start connectivity host");
        let mut input = child.stdin.take().expect("host input");
        let output = child.stdout.take().expect("host output");
        let mut start = json!({
            "root": root,
            "rendezvous": rendezvous,
            "relay": false,
            "legacy_group_updates": legacy_group_updates,
        });
        if let Some(secure_storage) = secure_storage {
            start["secure_storage"] = secure_storage;
        }
        writeln!(input, "{start}").expect("start host request");
        input.flush().expect("flush start request");
        let (replies, reader) = read_protocol_replies(output);
        let ready = next_reply(&replies);
        assert_eq!(ready["ready"], true, "host did not start: {ready}");
        (child, input, replies, reader)
    }

    fn request(&mut self, request: Value) -> Value {
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

    fn read_response(&mut self) -> Value {
        next_reply(&self.replies)
    }

    fn shutdown(&mut self) {
        let _ = self.request(json!({ "command": "shutdown" }));
        let status = self.child.wait().expect("wait for host");
        assert!(status.success(), "host exit: {status}");
    }

    fn restart(&mut self) {
        self.restart_with_crash(false);
    }

    fn restart_with_crash(&mut self, crash: bool) {
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
        let (child, input, replies, reader) = Self::spawn(
            self.root.path(),
            &self.rendezvous,
            Some(secure_storage),
            &self.binary,
            self.legacy_group_updates,
        );
        self.child = child;
        self.input = input;
        self.replies = replies;
        self.reader = Some(reader);
    }

    /// 宿主退出后其 stdout 关闭，读取线程随之结束。
    fn join_reader(&mut self) {
        if let Some(reader) = self.reader.take() {
            reader.join().expect("host output reader");
        }
    }
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

fn wait_for_phase(host: &mut HostProcess, expected: &str) -> Value {
    let deadline = Instant::now() + WAIT_TIMEOUT;
    loop {
        let status = host.request(json!({ "command": "eligibility" }));
        let update = &status["device_trust"]["space_device_update"];
        if update["phase"] == expected {
            return update.clone();
        }
        assert!(
            Instant::now() < deadline,
            "space device update did not reach {expected}: {update}"
        );
        std::thread::yield_now();
    }
}

// 独立进程只以公开查询判断恢复完成；测试事件只负责精确等待真实回复。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retryable_failure_recovers_the_public_space_device_update() {
    let rendezvous = mount_rendezvous().await;
    let mut sponsor = HostProcess::start(&rendezvous.uri());
    let mut joiner = HostProcess::start(&rendezvous.uri());
    sponsor.request(json!({ "command": "create", "name": "Sponsor" }));
    let invitation = sponsor.request(json!({ "command": "invite" }))["invitation"]
        .as_str()
        .expect("invitation")
        .to_owned();
    let baseline = sponsor.request(json!({
        "command": "arm_membership_history_failures",
        "failure": "retryable",
        "count": 1,
    }))["after_sequence"]
        .as_u64()
        .expect("event baseline");
    joiner.request(json!({
        "command": "join",
        "name": "Joiner",
        "invitation": invitation,
    }));
    let failed = sponsor.request(json!({
        "command": "wait_space_work_event",
        "kind": "membership_history_sync_retryable_failure",
        "after_sequence": baseline,
    }));
    let failed_sequence = failed["sequence"].as_u64().expect("failure sequence");
    wait_for_phase(&mut sponsor, "retryable_failure");
    sponsor.request(json!({
        "command": "wait_space_work_event",
        "kind": "membership_history_sync_reply_received",
        "after_sequence": failed_sequence,
    }));
    let completed = wait_for_phase(&mut sponsor, "completed");
    assert_eq!(completed["reason"], Value::Null);
    assert_eq!(completed["recovery"], Value::Null);
    assert_eq!(completed["next_retry_at_ms"], Value::Null);
    sponsor.restart();
    let restarted = wait_for_phase(&mut sponsor, "completed");
    assert_eq!(restarted["next_retry_at_ms"], Value::Null);
    sponsor.shutdown();
    joiner.shutdown();
}

// 需处理故障必须持续可见，只有测试进程明确解除后才允许公开状态恢复。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn attention_failure_stays_visible_until_recovery_then_completes() {
    let rendezvous = mount_rendezvous().await;
    let mut sponsor = HostProcess::start(&rendezvous.uri());
    let mut joiner = HostProcess::start(&rendezvous.uri());
    sponsor.request(json!({ "command": "create", "name": "Sponsor" }));
    let invitation = sponsor.request(json!({ "command": "invite" }))["invitation"]
        .as_str()
        .expect("invitation")
        .to_owned();
    let baseline = sponsor.request(json!({
        "command": "arm_membership_history_failures",
        "failure": "needs_attention",
        "count": 1_024,
    }))["after_sequence"]
        .as_u64()
        .expect("event baseline");
    joiner.request(json!({
        "command": "join",
        "name": "Joiner",
        "invitation": invitation,
    }));
    let failed = sponsor.request(json!({
        "command": "wait_space_work_event",
        "kind": "membership_history_sync_needs_attention",
        "after_sequence": baseline,
    }));
    let failed_sequence = failed["sequence"].as_u64().expect("failure sequence");
    let attention = wait_for_phase(&mut sponsor, "needs_attention");
    assert_eq!(attention["reason"], "device_state_rejected");
    assert_ne!(
        sponsor.request(json!({ "command": "eligibility" }))["device_trust"]["space_device_update"]
            ["phase"],
        "completed"
    );
    sponsor.request(json!({ "command": "clear_membership_history_failures" }));
    sponsor.request(json!({ "command": "opportunity" }));
    sponsor.request(json!({
        "command": "wait_space_work_event",
        "kind": "membership_history_sync_reply_received",
        "after_sequence": failed_sequence,
    }));
    wait_for_phase(&mut sponsor, "completed");
    sponsor.shutdown();
    joiner.shutdown();
}

// 可重试失败和下次时间必须跨进程保存，重启后由 Engine 自己继续同一恢复流程。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retryable_failure_recovers_after_sponsor_restart() {
    let rendezvous = mount_rendezvous().await;
    let mut sponsor = HostProcess::start(&rendezvous.uri());
    let mut joiner = HostProcess::start(&rendezvous.uri());
    sponsor.request(json!({ "command": "create", "name": "Sponsor" }));
    let invitation = sponsor.request(json!({ "command": "invite" }))["invitation"]
        .as_str()
        .expect("invitation")
        .to_owned();
    let baseline = sponsor.request(json!({
        "command": "arm_membership_history_failures",
        "failure": "retryable",
        "count": 1,
    }))["after_sequence"]
        .as_u64()
        .expect("event baseline");
    joiner.request(json!({
        "command": "join",
        "name": "Joiner",
        "invitation": invitation,
    }));
    sponsor.request(json!({
        "command": "wait_space_work_event",
        "kind": "membership_history_sync_retryable_failure",
        "after_sequence": baseline,
    }));
    let before_restart = wait_for_phase(&mut sponsor, "retryable_failure");
    assert!(before_restart["next_retry_at_ms"].as_i64().is_some());
    sponsor.restart();
    let after_restart = wait_for_phase(&mut sponsor, "retryable_failure");
    assert_eq!(
        after_restart["next_retry_at_ms"],
        before_restart["next_retry_at_ms"]
    );
    sponsor.request(json!({
        "command": "wait_space_work_event",
        "kind": "membership_history_sync_reply_received",
        "after_sequence": 0,
    }));
    wait_for_phase(&mut sponsor, "completed");
    sponsor.shutdown();
    joiner.shutdown();
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

fn wait_for_active_join(host: &mut HostProcess) {
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

/// 三个独立 Engine 进程、真实 SQLite/MLS/QUIC：纯联系不能解除拒绝，正确历史和实际 ACK 才能。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_process_group_rejection_recovers_only_after_verified_history() {
    let rendezvous = mount_rendezvous().await;
    let mut a = HostProcess::start(&rendezvous.uri());
    let mut b = HostProcess::start(&rendezvous.uri());
    let mut c = HostProcess::start(&rendezvous.uri());
    a.request(json!({"command": "create", "name": "A"}));
    let invitation = a.request(json!({"command": "invite"}))["invitation"].clone();
    b.request(json!({"command": "join", "name": "B", "invitation": invitation}));
    wait_for_active_join(&mut b);
    wait_for_phase(&mut a, "completed");
    wait_for_phase(&mut b, "completed");
    let peer =
        b.request(json!({"command": "eligibility"}))["device_trust"]["local_device_id"].clone();
    assert!(peer.is_string());
    for host in [&mut a, &mut c] {
        host.request(json!({"command": "set_membership_history_held", "held": true, "peer": peer}));
    }
    a.request(json!({"command": "arm_group_update_tamper", "count": 1}));
    let invitation = a.request(json!({"command": "invite"}))["invitation"].clone();
    c.request(json!({"command": "join", "name": "C", "invitation": invitation}));
    wait_for_active_join(&mut c);
    a.request(json!({"command": "wait_space_work_event", "kind": "group_update_tampered", "after_sequence": 0}));
    let rejected = wait_for_phase(&mut a, "needs_attention");
    assert_eq!(rejected["reason"], "device_security_update_rejected");
    b.restart();
    b.request(json!({"command": "opportunity"}));
    // 摘要与实际重新认证允许完成，但没有正确后缀的阶段仍持续可见。
    for _ in 0..5 {
        assert_eq!(
            a.request(json!({"command": "eligibility"}))["device_trust"]["space_device_update"]
                ["phase"],
            "needs_attention"
        );
        std::thread::sleep(Duration::from_millis(400));
    }
    for host in [&mut a, &mut c] {
        host.request(json!({"command": "set_membership_history_held", "held": false}));
    }
    a.request(json!({"command": "opportunity"}));
    b.request(json!({"command": "opportunity"}));
    wait_for_phase(&mut a, "completed");
    b.request(json!({"command": "wait_space_work_event", "kind": "group_update_applied", "after_sequence": 0}));
    a.shutdown();
    b.shutdown();
    c.shutdown();
}

async fn fixed_main_group_interop(legacy: bool) {
    let rendezvous = mount_rendezvous().await;
    let old = PathBuf::from(
        std::env::var_os("UC_GROUP_UPDATE_OLD_HOST")
            .expect("fixed main host binary must be provided"),
    );
    let current = PathBuf::from(env!("CARGO_BIN_EXE_uc-connectivity-host"));
    let mut a = HostProcess::start_binary(&rendezvous.uri(), old, false);
    let mut b = HostProcess::start_binary(&rendezvous.uri(), current.clone(), legacy);
    let mut c = HostProcess::start_binary(&rendezvous.uri(), current.clone(), legacy);
    let mut d = HostProcess::start_binary(&rendezvous.uri(), current, legacy);
    a.request(json!({"command": "create", "name": "OldA"}));
    let invitation = a.request(json!({"command": "invite"}))["invitation"].clone();
    b.request(json!({"command": "join", "name": "NewB", "invitation": invitation}));
    wait_for_active_join(&mut b);
    wait_for_phase(&mut a, "completed");
    wait_for_phase(&mut b, "completed");
    // 新 B 生成更新，保留旧 A：真实新→旧发送和旧 ACK。
    let invitation = b.request(json!({"command": "invite"}))["invitation"].clone();
    c.request(json!({"command": "join", "name": "NewC", "invitation": invitation}));
    wait_for_active_join(&mut c);
    wait_for_phase(&mut b, "completed");
    wait_for_phase(&mut a, "completed");
    // 旧 A 再生成更新，保留新 B/C：真实旧→新发送与新 ACK。
    let invitation = a.request(json!({"command": "invite"}))["invitation"].clone();
    d.request(json!({"command": "join", "name": "NewD", "invitation": invitation}));
    wait_for_active_join(&mut d);
    wait_for_phase(&mut a, "completed");
    wait_for_phase(&mut b, "completed");
    let events = b.request(json!({"command": "space_work_events"}));
    let kinds = if legacy {
        [
            "group_update_legacy_wire_sent",
            "group_update_legacy_wire_acknowledged",
        ]
    } else {
        [
            "group_update_current_wire_sent",
            "group_update_current_wire_acknowledged",
        ]
    };
    for kind in kinds {
        assert!(
            events
                .as_array()
                .unwrap()
                .iter()
                .any(|event| event["kind"] == kind),
            "actual wire must prove each direction of the selected group protocol: {kind}"
        );
    }
    a.shutdown();
    b.shutdown();
    c.shutdown();
    d.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "需要固定 main 二进制 UC_GROUP_UPDATE_OLD_HOST；独立兼容验收显式执行"]
async fn fixed_main_group_update_v2_interoperates_both_directions() {
    fixed_main_group_interop(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "需要固定 main 二进制 UC_GROUP_UPDATE_OLD_HOST；独立兼容验收显式执行"]
async fn fixed_main_group_update_v1_interoperates_both_directions() {
    fixed_main_group_interop(true).await;
}

fn join_host(sponsor: &mut HostProcess, joiner: &mut HostProcess, name: &str) {
    let invitation = sponsor.request(json!({"command": "invite"}))["invitation"].clone();
    joiner.request(json!({"command": "join", "name": name, "invitation": invitation}));
    wait_for_active_join(joiner);
}

fn wait_for_branch(hosts: &mut [&mut HostProcess], count: u32) -> u64 {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let snapshots = hosts
            .iter_mut()
            .map(|host| host.request(json!({"command": "membership_diagnostics"})))
            .collect::<Vec<_>>();
        if snapshots.iter().all(|snapshot| {
            snapshot["head_event_id"] == snapshots[0]["head_event_id"]
                && snapshot["branch_id"] == snapshots[0]["branch_id"]
                && snapshot["effective_member_count"] == count
                && snapshot["pending_effect_count"] == 0
        }) {
            return snapshots[0]["group_epoch"].as_u64().unwrap();
        }
        assert!(
            Instant::now() < deadline,
            "separate processes must finish verified branch and actual local effects"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// 独立进程重现真实 3→5 缺口、持久需求重启、原发送者离线与第三方连续恢复。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn separate_process_missing_predecessor_survives_restart_and_source_shutdown() {
    let rendezvous = mount_rendezvous().await;
    let mut a = HostProcess::start(&rendezvous.uri());
    let mut b = HostProcess::start(&rendezvous.uri());
    let mut c = HostProcess::start(&rendezvous.uri());
    let mut d = HostProcess::start(&rendezvous.uri());
    let mut e = HostProcess::start(&rendezvous.uri());
    a.request(json!({"command": "create", "name": "A"}));
    join_host(&mut a, &mut b, "B");
    join_host(&mut a, &mut c, "C");
    let before = wait_for_branch(&mut [&mut a, &mut b, &mut c], 3);
    let b_peer =
        b.request(json!({"command": "eligibility"}))["device_trust"]["local_device_id"].clone();
    let a_endpoint = a.request(json!({"command": "network_endpoint"}));
    let b_endpoint = b.request(json!({"command": "network_endpoint"}));
    a.request(json!({"command": "set_network_partition", "endpoint_ids": [b_endpoint]}));
    b.request(json!({"command": "set_network_partition", "endpoint_ids": [a_endpoint]}));
    b.request(json!({"command": "set_membership_history_held", "held": true}));
    for host in [&mut c, &mut d, &mut e] {
        host.request(
            json!({"command": "set_membership_history_held", "held": true, "peer": b_peer}),
        );
    }
    join_host(&mut a, &mut d, "D");
    wait_for_branch(&mut [&mut a, &mut c, &mut d], 4);
    join_host(&mut c, &mut e, "E");
    let target = wait_for_branch(&mut [&mut c, &mut d, &mut e], 5);
    let missing = b.request(json!({"command": "wait_space_work_event", "kind": "group_update_missing_predecessor_observed", "after_sequence": 0}));
    // ACK 3 在恢复需求持久提交之后发送；跨过这个边界再 kill，而不是只看到下层缺口错误。
    b.request(json!({"command": "wait_space_work_event", "kind": "group_update_current_wire_acknowledged", "after_sequence": missing["sequence"]}));
    assert_eq!(
        b.request(json!({"command": "membership_diagnostics"}))["group_epoch"],
        before
    );
    a.shutdown();
    b.restart_with_crash(true);
    assert_eq!(
        b.request(json!({"command": "membership_diagnostics"}))["group_epoch"],
        before
    );
    for host in [&mut c, &mut d, &mut e] {
        host.request(json!({"command": "set_membership_history_held", "held": false}));
    }
    assert_eq!(
        wait_for_branch(&mut [&mut b, &mut c, &mut d, &mut e], 5),
        target
    );
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let choices = c.request(json!({"command": "eligibility"}));
        let peer = choices["device_trust"]["devices"]
            .as_array()
            .unwrap()
            .iter()
            .find(|device| device["device_id"] == b_peer)
            .expect("B remains a current member");
        if peer["group_update"].is_null() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "C must receive actual ACK for B's recovered update"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    b.request(json!({"command": "wait_space_work_event", "kind": "group_update_applied", "after_sequence": 0}));
    b.shutdown();
    c.shutdown();
    d.shutdown();
    e.shutdown();
}
