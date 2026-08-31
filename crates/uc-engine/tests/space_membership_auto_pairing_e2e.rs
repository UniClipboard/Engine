#![cfg(feature = "dev-tools")]

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use tempfile::TempDir;
use uc_engine::{
    CreateSpaceInput, Engine, EngineConfig, HistoryEntryInput, HostCapabilities,
    HostCapabilityError, HostCapabilityErrorCategory, HostClipboard, HostClipboardSnapshot,
    HostDirectories, HostFileAccess, HostFileHandle, HostFileMetadata, HostSecureStorage,
    JoinSpaceInput, JoinSpaceStatusSummary, ListHistoryEntriesInput, Operation, OperationResult,
    SecretString, SendTextInput,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const PASSPHRASE: &str = "space-membership-e2e-passphrase";
const WAIT_TIMEOUT: Duration = Duration::from_secs(60);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(15);
const EXPIRES_AT_MS: i64 = 2_000_000_000_000;

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
        let root = self.root.path();
        let host = HostCapabilities::new(
            HostDirectories::new(
                root.join("private"),
                root.join("cache"),
                root.join("temporary"),
                root.join("logs"),
            ),
            Box::new(self.secure_storage.clone()),
            Box::new(EmptyClipboard),
            Box::new(EmptyFiles),
        );
        let config = EngineConfig::new("1.1.0")
            .with_rendezvous_base_url(self.rendezvous_base_url.clone())
            .with_test_relay_fallback(true);
        let (engine, _events) = Engine::start(config, host)
            .await
            .expect("start complete engine");
        engine
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
    AssertSnapshot {
        node: &'a str,
        active_members: usize,
        pending_choices: usize,
    },
}

struct MembershipTopology {
    rendezvous_base_url: String,
    harnesses: HashMap<String, DeviceHarness>,
    engines: HashMap<String, Engine>,
    space_ids: HashMap<String, String>,
}

impl MembershipTopology {
    fn new(rendezvous_base_url: String) -> Self {
        Self {
            rendezvous_base_url,
            harnesses: HashMap::new(),
            engines: HashMap::new(),
            space_ids: HashMap::new(),
        }
    }

    async fn run(&mut self, actions: &[TopologyAction<'_>]) {
        for action in actions {
            match *action {
                TopologyAction::Start { node } => self.start(node).await,
                TopologyAction::Create { node } => self.create(node).await,
                TopologyAction::Join { sponsor, joiner } => self.join(sponsor, joiner).await,
                TopologyAction::AssertSnapshot {
                    node,
                    active_members,
                    pending_choices,
                } => {
                    self.assert_snapshot(node, active_members, pending_choices)
                        .await
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
        self.harnesses.insert(node.to_owned(), harness);
        self.engines.insert(node.to_owned(), engine);
    }

    async fn create(&mut self, node: &str) {
        let engine = self.engine(node);
        let (space_id, _) = create_space(engine, node).await;
        self.space_ids.insert(node.to_owned(), space_id);
    }

    async fn join(&self, sponsor: &str, joiner: &str) {
        let space_id = self
            .space_ids
            .get(sponsor)
            .unwrap_or_else(|| panic!("sponsor {sponsor} has no space"));
        join_through(self.engine(sponsor), self.engine(joiner), joiner, space_id).await;
    }

    async fn assert_snapshot(
        &self,
        node: &str,
        expected_active_members: usize,
        expected_pending_choices: usize,
    ) {
        let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
        loop {
            if let Ok(OperationResult::DeviceGroupChoices(summary)) = self
                .engine(node)
                .execute(Operation::QueryDeviceGroupChoices)
                .await
            {
                let active_members = summary
                    .device_trust
                    .devices
                    .iter()
                    .filter(|device| {
                        device.membership == uc_engine::DeviceMembershipSummary::Active
                    })
                    .count();
                if active_members == expected_active_members
                    && summary.issues.len() == expected_pending_choices
                {
                    return;
                }
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "node {node} did not reach the expected public snapshot"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
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

// 声明式拓扑脚本只能通过稳定 Engine operation 观察和推进节点。
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
        ])
        .await;
    topology.shutdown().await;
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
        if engine
            .execute(Operation::RefreshPeerConnections)
            .await
            .is_ok()
        {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{label} peer connection refresh timed out"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
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
    let OperationResult::InvitationIssued {
        full_invitation, ..
    } = sponsor
        .execute(Operation::IssueInvitation)
        .await
        .expect("issue admission invitation")
    else {
        panic!("unexpected invitation result");
    };
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
    wait_for_completed_join(joiner, status, expected_space_id).await
}

async fn wait_for_completed_join(
    engine: &Engine,
    mut status: JoinSpaceStatusSummary,
    expected_space_id: &str,
) -> JoinResult {
    let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
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
            "space admission timed out"
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
