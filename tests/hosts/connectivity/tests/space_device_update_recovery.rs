use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex, MutexGuard};
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
    output: BufReader<ChildStdout>,
    root: TempDir,
    rendezvous: String,
}

impl HostProcess {
    fn start(rendezvous: &str) -> Self {
        let root = TempDir::new().expect("host root");
        let (child, input, output) = Self::spawn(root.path(), rendezvous, None);
        Self {
            child,
            input,
            output,
            root,
            rendezvous: rendezvous.to_owned(),
        }
    }

    fn spawn(
        root: &std::path::Path,
        rendezvous: &str,
        secure_storage: Option<Value>,
    ) -> (Child, ChildStdin, BufReader<ChildStdout>) {
        let mut child = Command::new(env!("CARGO_BIN_EXE_uc-connectivity-host"))
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
        });
        if let Some(secure_storage) = secure_storage {
            start["secure_storage"] = secure_storage;
        }
        writeln!(input, "{start}").expect("start host request");
        input.flush().expect("flush start request");
        let mut output = BufReader::new(output);
        let mut line = String::new();
        output.read_line(&mut line).expect("read host readiness");
        let ready: Value = serde_json::from_str(&line).expect("host readiness JSON");
        assert_eq!(ready["ready"], true, "host did not start: {ready}");
        (child, input, output)
    }

    fn request(&mut self, request: Value) -> Value {
        writeln!(self.input, "{request}").expect("write host request");
        self.input.flush().expect("flush host request");
        let response = self.read_response();
        response
            .get("ok")
            .cloned()
            .unwrap_or_else(|| panic!("host request failed: {response}"))
    }

    fn read_response(&mut self) -> Value {
        let mut line = String::new();
        self.output.read_line(&mut line).expect("read host reply");
        serde_json::from_str(&line).expect("host reply JSON")
    }

    fn shutdown(&mut self) {
        let _ = self.request(json!({ "command": "shutdown" }));
        let status = self.child.wait().expect("wait for host");
        assert!(status.success(), "host exit: {status}");
    }

    fn restart(&mut self) {
        let secure_storage = self.request(json!({ "command": "secure_storage" }));
        let _ = self.request(json!({ "command": "shutdown" }));
        let status = self.child.wait().expect("wait for stopped host");
        assert!(status.success(), "host exit before restart: {status}");
        let (child, input, output) =
            Self::spawn(self.root.path(), &self.rendezvous, Some(secure_storage));
        self.child = child;
        self.input = input;
        self.output = output;
    }
}

impl Drop for HostProcess {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
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
