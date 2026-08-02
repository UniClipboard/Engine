#![cfg(feature = "dev-tools")]

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use tempfile::TempDir;
use uc_engine::{
    CreateSpaceInput, DevOperation, DevOperationResult, DeviceSummary, Engine, EngineConfig,
    HistoryEntryInput, HostCapabilities, HostCapabilityError, HostCapabilityErrorCategory,
    HostClipboard, HostClipboardSnapshot, HostDirectories, HostFileAccess, HostFileHandle,
    HostFileMetadata, HostSecureStorage, JoinSpaceInput, ListHistoryEntriesInput,
    MembershipConvergenceStateSummary, Operation, OperationResult, RecoverSessionInput,
    SecretString, SendTargetOutcome, SendTextInput,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const PASSPHRASE: &str = "space-membership-e2e-passphrase";
const WAIT_TIMEOUT: Duration = Duration::from_secs(60);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(15);
const EXPIRES_AT_MS: i64 = 2_000_000_000_000;

#[derive(Clone, Default)]
struct MemorySecureStorage {
    values: Arc<Mutex<HashMap<String, Vec<u8>>>>,
}

impl MemorySecureStorage {
    fn values(&self) -> MutexGuard<'_, HashMap<String, Vec<u8>>> {
        match self.values.lock() {
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
        let config = EngineConfig::new("space-membership-e2e")
            .with_rendezvous_base_url(self.rendezvous_base_url.clone());
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

struct CreatePairing {
    vault: TicketVault,
}

impl Respond for CreatePairing {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: serde_json::Value =
            serde_json::from_slice(&request.body).expect("pairing create request must be JSON");
        let ticket = body["sponsorTicket"]
            .as_str()
            .expect("sponsor ticket missing")
            .to_owned();
        let mut state = lock_ticket_vault(&self.vault);
        state.next_code += 1;
        let code = format!("E2E0-A{:03}", state.next_code);
        state.tickets.insert(code.clone(), ticket);
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": code,
            "expiresAtMs": EXPIRES_AT_MS,
        }))
    }
}

struct ResolvePairing {
    vault: TicketVault,
}

impl Respond for ResolvePairing {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: serde_json::Value =
            serde_json::from_slice(&request.body).expect("pairing resolve request must be JSON");
        let code = body["code"].as_str().expect("pairing code missing");
        let ticket = lock_ticket_vault(&self.vault)
            .tickets
            .get(code)
            .cloned()
            .expect("pairing code was not registered");
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
        .respond_with(CreatePairing {
            vault: Arc::clone(&vault),
        })
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/pairings/resolve"))
        .respond_with(ResolvePairing { vault })
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/pairings/consume"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    server
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn offline_members_learn_about_each_other_and_keep_syncing_after_restart() {
    let rendezvous = mount_rendezvous().await;
    let device_a = DeviceHarness::new(rendezvous.uri());
    let device_b = DeviceHarness::new(rendezvous.uri());
    let device_c = DeviceHarness::new(rendezvous.uri());

    let engine_a = device_a.start().await;
    let engine_b = device_b.start().await;
    let (space_id, a_id) = create_space(&engine_a, "Device A").await;
    let b_id = join_through(&engine_a, &engine_b, "Device B", &space_id).await;
    wait_for_members(&engine_a, &[&b_id]).await;
    wait_for_members(&engine_b, &[&a_id]).await;

    engine_a
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shut down A before C joins");

    let engine_c = device_c.start().await;
    let c_id = join_through(&engine_b, &engine_c, "Device C", &space_id).await;
    wait_for_members(&engine_b, &[&a_id, &c_id]).await;
    engine_b
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shut down sponsor B");

    let engine_a = device_a.start().await;
    assert_eq!(
        engine_a
            .execute(Operation::LockEncryption)
            .await
            .expect("lock A before membership recovery"),
        OperationResult::EncryptionLocked
    );
    assert!(
        engine_a
            .execute(Operation::QueryMembershipConvergence)
            .await
            .is_err(),
        "locked membership state must not be decrypted"
    );
    recover(&engine_a).await;
    wait_for_converged_members(&engine_a, &engine_c, &a_id, &c_id).await;

    send_and_verify(
        &engine_a,
        &engine_c,
        &c_id,
        "A to C after B is offline: first transfer",
    )
    .await;
    send_and_verify(
        &engine_c,
        &engine_a,
        &a_id,
        "C to A after B is offline: first transfer",
    )
    .await;

    engine_a
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shut down A before restart verification");
    engine_c
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shut down C before restart verification");

    let engine_a = device_a.start().await;
    let engine_c = device_c.start().await;
    recover(&engine_a).await;
    recover(&engine_c).await;
    wait_for_converged_members(&engine_a, &engine_c, &a_id, &c_id).await;

    send_and_verify(
        &engine_a,
        &engine_c,
        &c_id,
        "A to C after both restart: second transfer",
    )
    .await;
    send_and_verify(
        &engine_c,
        &engine_a,
        &a_id,
        "C to A after both restart: second transfer",
    )
    .await;

    engine_a
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("final A shutdown");
    engine_c
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("final C shutdown");
}

async fn create_space(engine: &Engine, device_name: &str) -> (String, String) {
    let result = engine
        .execute(Operation::CreateSpace(CreateSpaceInput {
            device_name: Some(device_name.to_owned()),
            passphrase: SecretString::new(PASSPHRASE),
            passphrase_confirmation: SecretString::new(PASSPHRASE),
        }))
        .await
        .expect("create space");
    match result {
        OperationResult::SpaceCreated {
            space_id,
            self_device_id,
            ..
        } => (space_id, self_device_id),
        other => panic!("unexpected create result: {other:?}"),
    }
}

async fn join_through(
    sponsor: &Engine,
    joiner: &Engine,
    device_name: &str,
    expected_space_id: &str,
) -> String {
    let addresses = sponsor
        .execute_dev(DevOperation::ListPairingInvitationAddresses)
        .await
        .expect("list sponsor invitation addresses");
    let DevOperationResult::PairingInvitationAddresses(mut addresses) = addresses else {
        panic!("unexpected invitation address result");
    };
    assert!(
        !addresses.is_empty(),
        "sponsor must expose at least one invitation address"
    );
    addresses.sort_by_key(|address| (!address.ip.is_loopback(), !address.ip.is_ipv4()));
    let mut last_error = None;
    for selected in addresses {
        let invitation = sponsor
            .execute_dev(DevOperation::IssueInvitationForAddress {
                address: selected.ip,
            })
            .await
            .expect("issue local invitation");
        let DevOperationResult::InvitationIssued(invitation) = invitation else {
            panic!("unexpected invitation result");
        };
        match joiner
            .execute(Operation::JoinSpace(JoinSpaceInput {
                invitation_code: invitation.code,
                device_name: Some(device_name.to_owned()),
                passphrase: SecretString::new(PASSPHRASE),
            }))
            .await
        {
            Ok(OperationResult::SpaceJoined {
                space_id,
                self_device_id,
                ..
            }) => {
                assert_eq!(space_id, expected_space_id);
                return self_device_id;
            }
            Ok(other) => panic!("unexpected join result: {other:?}"),
            Err(error) => last_error = Some(error),
        }
    }
    panic!("join space through every sponsor address failed: {last_error:?}");
}

async fn recover(engine: &Engine) {
    let result = engine
        .execute(Operation::RecoverSession(RecoverSessionInput {
            allow_secure_storage_unlock: true,
        }))
        .await
        .expect("recover persisted session");
    assert_eq!(
        result,
        OperationResult::SessionRecovered {
            unlocked: true,
            resumed: true,
        }
    );
}

async fn wait_for_members(engine: &Engine, expected_ids: &[&str]) {
    wait_until(WAIT_TIMEOUT, || async {
        let devices = list_devices(engine).await;
        expected_ids
            .iter()
            .all(|expected| devices.iter().any(|device| device.device_id == **expected))
    })
    .await;
}

async fn wait_for_converged_members(engine_a: &Engine, engine_c: &Engine, a_id: &str, c_id: &str) {
    wait_until(WAIT_TIMEOUT, || async {
        has_complete_membership(engine_a, c_id).await
            && has_complete_membership(engine_c, a_id).await
    })
    .await;
}

async fn has_complete_membership(engine: &Engine, peer_id: &str) -> bool {
    let devices = list_devices(engine).await;
    let has_peer = devices
        .iter()
        .any(|device| device.device_id == peer_id && device.online);
    let convergence = engine
        .execute(Operation::QueryMembershipConvergence)
        .await
        .expect("query membership convergence");
    matches!(
        convergence,
        OperationResult::MembershipConvergence(summary)
            if summary.state == MembershipConvergenceStateSummary::Complete
                && summary.pending_count == 0
                && summary.waiting_for_peer_count == 0
                && summary.waiting_for_update_count == 0
                && summary.version_incompatible_count == 0
                && summary.blocked_count == 0
                && summary.rejected_count == 0
    ) && has_peer
}

async fn list_devices(engine: &Engine) -> Vec<DeviceSummary> {
    match engine
        .execute(Operation::ListDevices)
        .await
        .expect("list devices")
    {
        OperationResult::Devices(devices) => devices,
        other => panic!("unexpected device list result: {other:?}"),
    }
}

async fn send_and_verify(sender: &Engine, receiver: &Engine, target_id: &str, text: &str) {
    let sent = sender
        .execute(Operation::SendText(SendTextInput {
            text: text.to_owned(),
            target_devices: vec![target_id.to_owned()],
        }))
        .await
        .expect("send text to converged member");
    let OperationResult::EntrySent(report) = sent else {
        panic!("unexpected send result: {sent:?}");
    };
    assert_eq!(report.total_accepted, 1);
    assert_eq!(report.total_errored, 0);
    assert_eq!(report.total_offline, 0);
    assert_eq!(report.per_target.len(), 1);
    assert_eq!(report.per_target[0].device_id, target_id);
    assert_eq!(report.per_target[0].outcome, SendTargetOutcome::Accepted);

    wait_until(WAIT_TIMEOUT, || async {
        receiver_has_exact_text(receiver, text).await
    })
    .await;
}

async fn receiver_has_exact_text(engine: &Engine, expected_text: &str) -> bool {
    let entries = match engine
        .execute(Operation::ListHistoryEntries(ListHistoryEntriesInput {
            limit: 100,
            offset: 0,
        }))
        .await
    {
        Ok(OperationResult::HistoryEntries(entries)) => entries,
        Ok(other) => panic!("unexpected history list result: {other:?}"),
        Err(error) => panic!("history list failed: {error}"),
    };
    for entry in entries {
        let detail = engine
            .execute(Operation::GetHistoryEntry(HistoryEntryInput {
                entry_id: entry.entry_id,
            }))
            .await;
        match detail {
            Ok(OperationResult::HistoryEntry(detail)) if detail.content == expected_text => {
                return true;
            }
            Ok(OperationResult::HistoryEntry(_)) => {}
            Ok(other) => panic!("unexpected history detail result: {other:?}"),
            Err(error) => panic!("history detail failed: {error}"),
        }
    }
    false
}

async fn wait_until<F, Fut>(timeout: Duration, mut predicate: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if predicate().await {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "condition did not become true before timeout"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
