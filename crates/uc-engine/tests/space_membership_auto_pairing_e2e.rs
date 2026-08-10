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
    HostFileMetadata, HostSecureStorage, JoinSpaceInput, ListHistoryEntriesInput, Operation,
    OperationResult, RecoverSessionInput, RemoveMemberInput, SecretString, SendTargetOutcome,
    SendTextInput, WorkspaceConvergencePhaseSummary, WorkspaceConvergenceSummary,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const PASSPHRASE: &str = "space-membership-e2e-passphrase";
const WAIT_TIMEOUT: Duration = Duration::from_secs(60);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(15);
const EXPIRES_AT_MS: i64 = 2_000_000_000_000;

fn init_test_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .with_test_writer()
        .try_init();
}

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
        self.start_with_relay_fallback(true).await
    }

    async fn start_local_only(&self) -> Engine {
        self.start_with_relay_fallback(false).await
    }

    async fn start_with_relay_fallback(&self, allow_relay_fallback: bool) -> Engine {
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
            .with_rendezvous_base_url(self.rendezvous_base_url.clone())
            .with_test_relay_fallback(allow_relay_fallback);
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
async fn members_converge_when_sponsor_stays_offline_after_joining_c() {
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
    assert_receive_ready(&engine_a, false).await;
    assert!(
        engine_a
            .execute(Operation::QueryWorkspaceConvergence)
            .await
            .is_err(),
        "locked membership state must not be decrypted"
    );
    recover(&engine_a).await;
    assert_receive_ready(&engine_a, true).await;
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

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn restarted_member_pairs_with_a_member_added_while_it_was_offline() {
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

    engine_b
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shut down B before C joins");

    let engine_c = device_c.start().await;
    let c_id = join_through(&engine_a, &engine_c, "Device C", &space_id).await;
    assert_receive_ready(&engine_c, true).await;
    wait_for_members(&engine_a, &[&b_id, &c_id]).await;
    wait_for_members(&engine_c, &[&a_id]).await;

    let engine_b = device_b.start().await;
    recover(&engine_b).await;
    wait_for_converged_members(&engine_b, &engine_c, &b_id, &c_id).await;

    send_and_verify(&engine_b, &engine_c, &c_id, "B to C after B restarts").await;
    send_and_verify(&engine_c, &engine_b, &b_id, "C to B after B restarts").await;

    for engine in [engine_a, engine_b, engine_c] {
        engine
            .shutdown(SHUTDOWN_TIMEOUT)
            .await
            .expect("final three-device shutdown");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn four_members_converge_through_an_online_relay_after_two_sponsors_leave() {
    let rendezvous = mount_rendezvous().await;
    let device_a = DeviceHarness::new(rendezvous.uri());
    let device_b = DeviceHarness::new(rendezvous.uri());
    let device_c = DeviceHarness::new(rendezvous.uri());
    let device_d = DeviceHarness::new(rendezvous.uri());

    let engine_a = device_a.start().await;
    let engine_b = device_b.start().await;
    let (space_id, a_id) = create_space(&engine_a, "Device A").await;
    let b_id = join_through(&engine_a, &engine_b, "Device B", &space_id).await;
    wait_for_members(&engine_a, &[&b_id]).await;

    engine_a
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shut down A");
    let engine_c = device_c.start().await;
    let _c_id = join_through(&engine_b, &engine_c, "Device C", &space_id).await;
    engine_b
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shut down B");

    let engine_d = device_d.start().await;
    let d_id = join_through(&engine_c, &engine_d, "Device D", &space_id).await;
    wait_for_members(&engine_c, &[&a_id, &d_id]).await;

    let engine_a = device_a.start().await;
    recover(&engine_a).await;
    wait_for_converged_members(&engine_a, &engine_d, &a_id, &d_id).await;
    send_and_verify(&engine_a, &engine_d, &d_id, "A to D through C").await;
    send_and_verify(&engine_d, &engine_a, &a_id, "D to A through C").await;

    for engine in [engine_a, engine_c, engine_d] {
        engine
            .shutdown(SHUTDOWN_TIMEOUT)
            .await
            .expect("final shutdown");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 10)]
#[ignore = "ADR-016 runtime: the pairing flow does not yet record admission changes into the workspace chain, so removal cannot run on an empty chain"]
async fn five_devices_restore_full_sync_after_two_completed_removals_and_rejoins() {
    let rendezvous = mount_rendezvous().await;
    let harnesses = (0..5)
        .map(|_| DeviceHarness::new(rendezvous.uri()))
        .collect::<Vec<_>>();
    let engine_a = harnesses[0].start().await;
    let engine_b = harnesses[1].start().await;
    let engine_c = harnesses[2].start().await;
    let engine_d = harnesses[3].start().await;
    let engine_e = harnesses[4].start().await;
    let (space_id, a_id) = create_space(&engine_a, "Device A").await;
    let b_id = join_through(&engine_a, &engine_b, "Device B", &space_id).await;
    let c_id = join_through(&engine_a, &engine_c, "Device C", &space_id).await;
    let d_id = join_through(&engine_a, &engine_d, "Device D", &space_id).await;
    let e_id = join_through(&engine_a, &engine_e, "Device E", &space_id).await;
    wait_for_members(&engine_a, &[&b_id, &c_id, &d_id, &e_id]).await;

    for (device_id, expected_member_count) in [(&b_id, 4), (&d_id, 3)] {
        engine_a
            .execute(Operation::RemoveMember(RemoveMemberInput {
                device_id: device_id.clone(),
            }))
            .await
            .expect("remove member");
        wait_until(WAIT_TIMEOUT, || async {
            let summary = workspace_convergence_summary(&engine_a).await;
            summary.phase != WorkspaceConvergencePhaseSummary::RecoveryRequired
                && summary.effective_member_count == expected_member_count
        })
        .await;
    }

    let b_rejoin = join_through_with_result(&engine_c, &engine_b, "Device B", &space_id).await;
    assert_eq!(b_rejoin.self_device_id, b_id);
    assert_eq!(b_rejoin.migrated_records, Some(0));
    let d_rejoin = join_through_with_result(&engine_e, &engine_d, "Device D", &space_id).await;
    assert_eq!(d_rejoin.self_device_id, d_id);
    assert_eq!(d_rejoin.migrated_records, Some(0));
    wait_for_full_workspace_sync(
        [
            (&engine_a, a_id.as_str()),
            (&engine_b, b_id.as_str()),
            (&engine_c, c_id.as_str()),
            (&engine_d, d_id.as_str()),
            (&engine_e, e_id.as_str()),
        ]
        .as_slice(),
    )
    .await;
    send_and_verify(&engine_b, &engine_d, &d_id, "B to D after rejoin").await;
    send_and_verify(&engine_d, &engine_b, &b_id, "D to B after rejoin").await;
    send_and_verify(&engine_a, &engine_e, &e_id, "A to E after rejoin").await;
    send_and_verify(&engine_e, &engine_c, &c_id, "E to C after rejoin").await;

    for engine in [engine_a, engine_b, engine_c, engine_d, engine_e] {
        engine
            .shutdown(SHUTDOWN_TIMEOUT)
            .await
            .expect("final shutdown");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn member_removal_converges_across_three_independent_engine_directories() {
    init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let device_a = DeviceHarness::new(rendezvous.uri());
    let device_b = DeviceHarness::new(rendezvous.uri());
    let device_c = DeviceHarness::new(rendezvous.uri());
    let engine_a = device_a.start_local_only().await;
    let engine_b = device_b.start_local_only().await;
    let engine_c = device_c.start_local_only().await;
    let (space_id, a_id) = create_space(&engine_a, "Device A").await;
    let b_id = join_through(&engine_a, &engine_b, "Device B", &space_id).await;
    let c_id = join_through(&engine_a, &engine_c, "Device C", &space_id).await;

    wait_for_members(&engine_a, &[&b_id, &c_id]).await;
    wait_for_members(&engine_b, &[&a_id]).await;
    wait_for_members(&engine_c, &[&a_id]).await;

    wait_until(WAIT_TIMEOUT, || async {
        workspace_convergence_summary(&engine_a)
            .await
            .effective_member_count
            == 3
    })
    .await;

    let submitted = engine_a
        .execute(Operation::RemoveMember(RemoveMemberInput {
            device_id: b_id.clone(),
        }))
        .await
        .expect("submit member removal");
    assert!(matches!(
        submitted,
        OperationResult::WorkspaceConvergence(_)
    ));

    let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
    loop {
        let a = workspace_convergence_summary(&engine_a).await;
        let c = workspace_convergence_summary(&engine_c).await;
        if a.effective_member_count == 2
            && c.effective_member_count == 2
            && a.convergence_digest == c.convergence_digest
            && a.convergence_digest.is_some()
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "member removal did not converge; A={a:?}; C={c:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    for engine in [engine_a, engine_b, engine_c] {
        engine
            .shutdown(SHUTDOWN_TIMEOUT)
            .await
            .expect("shut down member removal engine");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "ADR-016 runtime: the pairing flow does not yet record admission changes into the workspace chain, so removal cannot run on an empty chain"]
async fn completed_removal_can_continue_from_the_recovered_member_state() {
    init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let device_a = DeviceHarness::new(rendezvous.uri());
    let device_b = DeviceHarness::new(rendezvous.uri());
    let device_c = DeviceHarness::new(rendezvous.uri());
    let engine_a = device_a.start_local_only().await;
    let engine_b = device_b.start_local_only().await;
    let engine_c = device_c.start_local_only().await;
    let (space_id, a_id) = create_space(&engine_a, "Device A").await;
    let b_id = join_through(&engine_a, &engine_b, "Device B", &space_id).await;
    let c_id = join_through(&engine_a, &engine_c, "Device C", &space_id).await;

    wait_for_members(&engine_a, &[&b_id, &c_id]).await;
    wait_for_members(&engine_b, &[&a_id]).await;
    wait_for_members(&engine_c, &[&a_id]).await;

    engine_a
        .execute(Operation::RemoveMember(RemoveMemberInput {
            device_id: b_id.clone(),
        }))
        .await
        .expect("submit first removal");
    wait_until(WAIT_TIMEOUT, || async {
        let a = workspace_convergence_summary(&engine_a).await;
        let c = workspace_convergence_summary(&engine_c).await;
        a.effective_member_count == 2
            && c.effective_member_count == 2
            && a.convergence_digest == c.convergence_digest
            && a.convergence_digest.is_some()
    })
    .await;

    engine_a
        .execute(Operation::RemoveMember(RemoveMemberInput {
            device_id: c_id.clone(),
        }))
        .await
        .expect("submit successor removal");
    let successor = workspace_convergence_summary(&engine_a).await;
    assert_eq!(
        successor.effective_member_count, 1,
        "the successor intent must use only the recovered current members"
    );
    wait_until(WAIT_TIMEOUT, || async {
        let current = workspace_convergence_summary(&engine_a).await;
        current.phase != WorkspaceConvergencePhaseSummary::RecoveryRequired
            && current.effective_member_count == 1
    })
    .await;

    for engine in [engine_a, engine_b, engine_c] {
        engine
            .shutdown(SHUTDOWN_TIMEOUT)
            .await
            .expect("shut down member removal engine");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn offline_clipboard_delivery_reaches_the_receiver_after_it_restarts() {
    let rendezvous = mount_rendezvous().await;
    let device_a = DeviceHarness::new(rendezvous.uri());
    let device_b = DeviceHarness::new(rendezvous.uri());

    let engine_a = device_a.start().await;
    let engine_b = device_b.start().await;
    let (space_id, a_id) = create_space(&engine_a, "Device A").await;
    let b_id = join_through(&engine_a, &engine_b, "Device B", &space_id).await;
    wait_for_members(&engine_a, &[&b_id]).await;
    wait_for_members(&engine_b, &[&a_id]).await;

    engine_b
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("take B offline before sending");

    let text = "offline recovery must reach B exactly";
    let sent = engine_a
        .execute(Operation::SendText(SendTextInput {
            text: text.to_owned(),
            target_devices: vec![b_id.clone()],
        }))
        .await
        .expect("record an offline send attempt");
    let OperationResult::EntrySent(report) = sent else {
        panic!("unexpected offline send result: {sent:?}");
    };
    assert_eq!(report.total_accepted, 0);
    assert_eq!(report.total_offline, 1);

    engine_a
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("restart A with its saved offline delivery record");
    let engine_a = device_a.start().await;
    recover(&engine_a).await;

    let engine_b = device_b.start().await;
    recover(&engine_b).await;
    wait_until(WAIT_TIMEOUT, || async {
        receiver_has_exact_text(&engine_b, text).await
    })
    .await;

    engine_a
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shut down A after offline delivery recovery");
    engine_b
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shut down B after offline delivery recovery");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn offline_clipboard_delivery_only_sends_the_latest_content_when_the_receiver_returns() {
    let rendezvous = mount_rendezvous().await;
    let device_a = DeviceHarness::new(rendezvous.uri());
    let device_b = DeviceHarness::new(rendezvous.uri());

    let engine_a = device_a.start().await;
    let engine_b = device_b.start().await;
    let (space_id, a_id) = create_space(&engine_a, "Device A").await;
    let b_id = join_through(&engine_a, &engine_b, "Device B", &space_id).await;
    wait_for_members(&engine_a, &[&b_id]).await;
    wait_for_members(&engine_b, &[&a_id]).await;

    engine_b
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("take B offline before sending");

    let stale_text = "offline stale content must not reach B";
    let latest_text = "offline latest content must reach B";
    for text in [stale_text, latest_text] {
        let sent = engine_a
            .execute(Operation::SendText(SendTextInput {
                text: text.to_owned(),
                target_devices: vec![b_id.clone()],
            }))
            .await
            .expect("record an offline send attempt");
        let OperationResult::EntrySent(report) = sent else {
            panic!("unexpected offline send result: {sent:?}");
        };
        assert_eq!(report.total_accepted, 0);
        assert_eq!(report.total_offline, 1);
    }

    let engine_b = device_b.start().await;
    recover(&engine_b).await;
    wait_until(WAIT_TIMEOUT, || async {
        receiver_has_exact_text(&engine_b, latest_text).await
    })
    .await;
    assert!(
        !receiver_has_exact_text(&engine_b, stale_text).await,
        "B must not receive content replaced while it was offline"
    );

    engine_a
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shut down A after latest-only offline delivery recovery");
    engine_b
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shut down B after latest-only offline delivery recovery");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn stable_join_routes_a_fresh_device_then_switches_an_existing_device() {
    let rendezvous = mount_rendezvous().await;
    let first_sponsor = DeviceHarness::new(rendezvous.uri());
    let joining_device = DeviceHarness::new(rendezvous.uri());
    let second_sponsor = DeviceHarness::new(rendezvous.uri());

    let first_sponsor = first_sponsor.start().await;
    let joining_device = joining_device.start().await;
    let second_sponsor = second_sponsor.start().await;
    let (first_space_id, _) = create_space(&first_sponsor, "First Sponsor").await;
    let (second_space_id, _) = create_space(&second_sponsor, "Second Sponsor").await;

    let invalid_name = joining_device
        .execute(Operation::JoinSpace(JoinSpaceInput {
            invitation_code: "unused-for-invalid-input".to_owned(),
            device_name: Some("  ".to_owned()),
            passphrase: SecretString::new(PASSPHRASE),
            preserve_unreadable_history: false,
        }))
        .await
        .expect_err("blank join device name must be rejected");
    assert_eq!(invalid_name.code(), 1231);

    let fresh = join_through_with_result(
        &first_sponsor,
        &joining_device,
        "Joining Device",
        &first_space_id,
    )
    .await;
    assert_eq!(fresh.migrated_records, None);

    let switched = join_through_with_result(
        &second_sponsor,
        &joining_device,
        "Joining Device",
        &second_space_id,
    )
    .await;
    assert_eq!(switched.migrated_records, Some(0));
    assert!(!switched.self_device_id.is_empty());

    for engine in [&first_sponsor, &joining_device, &second_sponsor] {
        engine
            .shutdown(SHUTDOWN_TIMEOUT)
            .await
            .expect("shut down automatic join routing test engine");
    }
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
    let result = join_through_with_result(sponsor, joiner, device_name, expected_space_id).await;
    assert_eq!(
        result.migrated_records, None,
        "a fresh join must not report migrated records"
    );
    result.self_device_id
}

struct JoinResult {
    self_device_id: String,
    migrated_records: Option<u64>,
}

async fn join_through_with_result(
    sponsor: &Engine,
    joiner: &Engine,
    device_name: &str,
    expected_space_id: &str,
) -> JoinResult {
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
                preserve_unreadable_history: false,
            }))
            .await
        {
            Ok(OperationResult::SpaceJoined {
                space_id,
                self_device_id,
                migrated_records,
                ..
            }) => {
                assert_eq!(space_id, expected_space_id);
                return JoinResult {
                    self_device_id,
                    migrated_records,
                };
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

async fn workspace_convergence_summary(engine: &Engine) -> WorkspaceConvergenceSummary {
    let result = engine
        .execute(Operation::QueryWorkspaceConvergence)
        .await
        .expect("query workspace convergence state");
    let OperationResult::WorkspaceConvergence(summary) = result else {
        panic!("unexpected workspace convergence query result: {result:?}");
    };
    summary
}

async fn assert_receive_ready(engine: &Engine, expected: bool) {
    let result = engine
        .execute(Operation::QueryReceiveReadiness)
        .await
        .expect("query receive readiness");
    assert_eq!(
        result,
        OperationResult::ReceiveReadiness(uc_engine::ReceiveReadinessSummary {
            ready: expected,
            degraded: false,
        })
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

async fn wait_for_full_workspace_sync(devices: &[(&Engine, &str)]) {
    wait_until(WAIT_TIMEOUT, || async {
        for (engine, own_id) in devices {
            for (_, peer_id) in devices {
                if own_id != peer_id && !has_complete_membership(engine, peer_id).await {
                    return false;
                }
            }
        }
        true
    })
    .await;
}

async fn has_complete_membership(engine: &Engine, peer_id: &str) -> bool {
    let devices = list_devices(engine).await;
    let has_peer = devices
        .iter()
        .any(|device| device.device_id == peer_id && device.online);
    let Ok(OperationResult::WorkspaceConvergence(summary)) =
        engine.execute(Operation::QueryWorkspaceConvergence).await
    else {
        return false;
    };
    // The workspace chain must be readable and not unrecoverable.
    summary.phase != WorkspaceConvergencePhaseSummary::RecoveryRequired && has_peer
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
