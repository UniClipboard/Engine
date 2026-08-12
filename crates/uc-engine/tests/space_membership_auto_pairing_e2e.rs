#![cfg(feature = "dev-tools")]

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use tempfile::TempDir;
use uc_engine::{
    CreateSpaceInput, DecideMembershipRemovalInput, DevOperation, DevOperationResult,
    DeviceSummary, Engine, EngineConfig, EngineErrorCategory, HistoryEntryInput, HostCapabilities,
    HostCapabilityError, HostCapabilityErrorCategory, HostClipboard, HostClipboardSnapshot,
    HostDirectories, HostFileAccess, HostFileHandle, HostFileMetadata, HostSecureStorage,
    JoinSpaceInput, ListHistoryEntriesInput, MembershipRemovalDecision, Operation, OperationResult,
    RecoverSessionInput, RemoveMemberInput, SecretString, SendTargetOutcome, SendTextInput,
    WorkspaceConvergencePhaseSummary, WorkspaceConvergenceSummary,
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

#[derive(Clone)]
struct FileSecureStorage {
    root: PathBuf,
}

impl FileSecureStorage {
    fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn path_for(&self, key: &str) -> PathBuf {
        let encoded = key
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        self.root.join(format!("{encoded}.bin"))
    }
}

impl HostSecureStorage for FileSecureStorage {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, HostCapabilityError> {
        match std::fs::read(self.path_for(key)) {
            Ok(value) => Ok(Some(value)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(HostCapabilityError::new(
                HostCapabilityErrorCategory::Io,
                "failed to read test secure storage",
            )),
        }
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<(), HostCapabilityError> {
        std::fs::create_dir_all(&self.root).map_err(|_| {
            HostCapabilityError::new(
                HostCapabilityErrorCategory::Io,
                "failed to create test secure storage",
            )
        })?;
        std::fs::write(self.path_for(key), value).map_err(|_| {
            HostCapabilityError::new(
                HostCapabilityErrorCategory::Io,
                "failed to write test secure storage",
            )
        })
    }

    fn delete(&self, key: &str) -> Result<(), HostCapabilityError> {
        match std::fs::remove_file(self.path_for(key)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(HostCapabilityError::new(
                HostCapabilityErrorCategory::Io,
                "failed to delete test secure storage",
            )),
        }
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

async fn start_engine_from_v019_data(
    data_root: &Path,
    rendezvous_base_url: String,
    bind_port: Option<u16>,
) -> Engine {
    let host = HostCapabilities::new(
        HostDirectories::new(
            data_root.to_path_buf(),
            data_root.join("cache"),
            data_root.join("temporary"),
            data_root.join("logs"),
        ),
        Box::new(FileSecureStorage::new(data_root.join("keyring"))),
        Box::new(EmptyClipboard),
        Box::new(EmptyFiles),
    );
    let mut config = EngineConfig::new("1.1.0").with_rendezvous_base_url(rendezvous_base_url);
    if let Some(bind_port) = bind_port {
        config = config.with_test_iroh_bind_port(bind_port);
    }
    let (engine, _events) = Engine::start(config, host)
        .await
        .expect("start current engine from v0.19 data");
    engine
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

// 场景流程：
// 1. 使用正式 v0.19.0 程序创建空间并正常停止旧进程，保留原资料目录和文件密钥。
// 2. 本次 1.1 Engine 直接接管同一目录和同一密钥，不重新初始化或复制业务资料。
// 3. 1.1 恢复会话后读取旧设备和成员状态，再正常关闭。
// 验证：从 0.19 原地升级到 1.1 时，Space、成员关系和设备身份不会丢失。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires a data directory produced by the official v0.19.0 binary"]
async fn v019_data_is_resumed_in_place_by_v11() {
    uc_engine::init_test_tracing();
    let data_root = std::env::var_os("UC_V019_DATA_ROOT")
        .map(PathBuf::from)
        .expect("UC_V019_DATA_ROOT must point to stopped v0.19 data");
    let expected_device_id = std::env::var("UC_V019_DEVICE_ID")
        .expect("UC_V019_DEVICE_ID must identify the v0.19 device");
    let rendezvous = mount_rendezvous().await;
    let engine = start_engine_from_v019_data(&data_root, rendezvous.uri(), None).await;

    recover(&engine).await;

    let devices = list_devices(&engine).await;
    assert!(
        devices
            .iter()
            .any(|device| device.device_id == expected_device_id),
        "the upgraded engine must retain the v0.19 device identity"
    );
    let convergence = workspace_convergence_summary(&engine).await;
    assert!(!convergence.removed);
    assert!(convergence.failure_category.is_none());

    engine
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shut down upgraded v0.19 probe");
}

// 场景流程：
// 1. A、B 先用正式 v0.19.0 建立同一个 Space，随后只停止 A 的旧进程。
// 2. 本次 1.1 Engine 原地接管 A 的资料，B 继续运行 v0.19.0。
// 3. A 恢复后仍从原关系中显示 B，并通过旧内容入口的空连接确认 B 低于 1.1。
// 4. A 重启后提示仍保留；A、B 分别发送唯一新文本，接收端都不得保存该内容。
// 5. 正常停止 B 的 0.19 进程，再让 1.1 Engine 原地接管 B 的同一资料和网络端口。
// 6. A、B 自动建立同一份两成员历史，A 清除升级提示，并双向发送唯一文本。
// 验证：单边升级期间持久提示并暂停双向同步；双方升级后自动清除提示并由接收端历史确认恢复。
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "requires paired v0.19 data and a running v0.19 peer"]
async fn v11_marks_a_running_v019_peer_as_upgrade_required() {
    uc_engine::init_test_tracing();
    let data_root = std::env::var_os("UC_V019_DATA_ROOT")
        .map(PathBuf::from)
        .expect("UC_V019_DATA_ROOT must point to stopped v0.19 data");
    let expected_peer_id =
        std::env::var("UC_V019_PEER_ID").expect("UC_V019_PEER_ID must identify the v0.19 peer");
    let v019_cli = std::env::var_os("UC_V019_CLI")
        .map(PathBuf::from)
        .expect("UC_V019_CLI must point to the running peer's v0.19 uniclip binary");
    let v019_profile =
        std::env::var("UC_V019_PROFILE").expect("UC_V019_PROFILE must identify the v0.19 peer");
    let peer_data_root = std::env::var_os("UC_V019_PEER_DATA_ROOT")
        .map(PathBuf::from)
        .expect("UC_V019_PEER_DATA_ROOT must point to the running peer's v0.19 data");
    let peer_pid = std::env::var("UC_V019_PEER_PID")
        .expect("UC_V019_PEER_PID must identify the running v0.19 peer")
        .parse::<u32>()
        .expect("UC_V019_PEER_PID must be an unsigned process id");
    let a_bind_port = required_test_port("UC_V11_A_BIND_PORT");
    let b_bind_port = required_test_port("UC_V11_B_BIND_PORT");
    let rendezvous = mount_rendezvous().await;
    let engine = start_engine_from_v019_data(&data_root, rendezvous.uri(), Some(a_bind_port)).await;

    recover(&engine).await;

    wait_until(WAIT_TIMEOUT, || async {
        list_devices(&engine)
            .await
            .iter()
            .any(|device| device.device_id == expected_peer_id)
    })
    .await;
    let own_device_id = list_devices(&engine)
        .await
        .into_iter()
        .find(|device| device.is_local)
        .map(|device| device.device_id)
        .expect("upgraded A remains in the device roster");
    wait_until(WAIT_TIMEOUT, || async {
        workspace_convergence_summary(&engine)
            .await
            .upgrade_required_peer_device_ids
            .contains(&expected_peer_id)
    })
    .await;

    engine
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shut down mixed-version engine A before restart");

    let engine = start_engine_from_v019_data(&data_root, rendezvous.uri(), Some(a_bind_port)).await;
    recover(&engine).await;
    assert!(
        workspace_convergence_summary(&engine)
            .await
            .upgrade_required_peer_device_ids
            .contains(&expected_peer_id),
        "the upgrade requirement must survive A restart"
    );

    let a_to_b = format!("ADR020-V11-A-TO-V019-B-{}", uuid::Uuid::new_v4());
    send_and_assert_blocked(&engine, &expected_peer_id, &a_to_b).await;
    assert_v019_cli_never_observes_text(&v019_cli, &v019_profile, &a_to_b).await;

    let b_to_a = format!("ADR020-V019-B-TO-V11-A-{}", uuid::Uuid::new_v4());
    send_v019_text(&v019_cli, &v019_profile, &own_device_id, &b_to_a);
    assert_engine_never_observes_text(&engine, &b_to_a).await;

    stop_v019_peer(peer_pid, &peer_data_root).await;
    let engine_b =
        start_engine_from_v019_data(&peer_data_root, rendezvous.uri(), Some(b_bind_port)).await;
    recover(&engine_b).await;

    wait_for_same_workspace_state_with_diagnostics(
        "A and B after upgrading B to 1.1",
        &[&engine, &engine_b],
        2,
        2,
    )
    .await;
    assert!(
        !workspace_convergence_summary(&engine)
            .await
            .upgrade_required_peer_device_ids
            .contains(&expected_peer_id),
        "A must clear B's upgrade requirement after B upgrades"
    );

    let resumed_a_to_b = format!("ADR020-V11-A-TO-V11-B-{}", uuid::Uuid::new_v4());
    send_and_verify(&engine, &engine_b, &expected_peer_id, &resumed_a_to_b).await;
    let resumed_b_to_a = format!("ADR020-V11-B-TO-V11-A-{}", uuid::Uuid::new_v4());
    send_and_verify(&engine_b, &engine, &own_device_id, &resumed_b_to_a).await;

    engine_b
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shut down upgraded engine B");
    engine
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shut down upgraded engine A");
}

// 场景流程：
// 1. A 创建空间，B 加入并保存 A、B 的完整成员记录。
// 2. A 离线后，仍在线的 B 让 C 加入；C 必须保存 A、B、C 的完整成员记录。
// 3. B 再离线，A 恢复后必须能与 C 重新互认并双向传递内容。
// 4. A、C 同时重启并恢复后，仍必须再次双向传递内容。
// 验证：中间两个发起加入的设备都离线后，首个设备与新设备仍能恢复连续成员关系。
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn members_converge_when_sponsor_stays_offline_after_joining_c() {
    uc_engine::init_test_tracing();
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
    // The joiner must have received the continuous chain before the sponsor
    // goes offline (ADR-016: joining produces only local readiness until
    // the chain is handed over).
    wait_until(WAIT_TIMEOUT, || async {
        workspace_convergence_summary(&engine_b)
            .await
            .history_event_count
            >= 2
    })
    .await;
    engine_a
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shut down A before C joins");

    let engine_c = device_c.start().await;
    let c_id = join_through(&engine_b, &engine_c, "Device C", &space_id).await;
    wait_for_members(&engine_b, &[&a_id, &c_id]).await;
    wait_until(WAIT_TIMEOUT, || async {
        workspace_convergence_summary(&engine_c)
            .await
            .history_event_count
            >= 3
    })
    .await;
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
    let locked_query = engine_a.execute(Operation::QueryWorkspaceConvergence).await;
    assert!(
        locked_query.is_err(),
        "locked membership state must not be decrypted"
    );
    recover(&engine_a).await;
    assert_receive_ready(&engine_a, true).await;
    wait_for_converged_members_with_diagnostics(
        "A must reconnect with C after B stops",
        &engine_a,
        &engine_c,
        &a_id,
        &c_id,
    )
    .await;
    tokio::time::sleep(Duration::from_secs(8)).await;

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
    wait_for_converged_members_with_diagnostics(
        "A and C must reconnect after both restart",
        &engine_a,
        &engine_c,
        &a_id,
        &c_id,
    )
    .await;

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

// 场景流程：
// 1. A 创建空间，B 加入。
// 2. B 离线期间，A 让 C 加入。
// 3. B 重启并恢复后，必须发现 C 并与 C 双向传递内容。
// 验证：离线成员恢复后，能够接入自己离线期间新增的成员。
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn restarted_member_pairs_with_a_member_added_while_it_was_offline() {
    uc_engine::init_test_tracing();
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

// 场景流程：
// 1. A 创建空间，B 加入后保存 A、B 的完整成员记录。
// 2. A 离线，B 让 C 加入；B、C 都保存 A、B、C 的完整成员记录。
// 3. B 离线，C 让 D 加入；C、D 都保存四台设备的完整成员记录。
// 4. A 恢复后，必须与 D 重新互认并双向传递内容。
// 验证：连续两次接力加入后，最早离线的设备仍能恢复到当前成员关系。
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn four_members_converge_through_an_online_relay_after_two_sponsors_leave() {
    uc_engine::init_test_tracing();
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
    wait_until(WAIT_TIMEOUT, || async {
        workspace_convergence_summary(&engine_b)
            .await
            .history_event_count
            >= 2
    })
    .await;

    engine_a
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shut down A");
    let engine_c = device_c.start().await;
    let _c_id = join_through(&engine_b, &engine_c, "Device C", &space_id).await;
    wait_until(WAIT_TIMEOUT, || async {
        workspace_convergence_summary(&engine_c)
            .await
            .history_event_count
            >= 3
            && workspace_convergence_summary(&engine_b)
                .await
                .history_event_count
                >= 3
    })
    .await;
    engine_b
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shut down B");

    let engine_d = device_d.start().await;
    let d_id = join_through(&engine_c, &engine_d, "Device D", &space_id).await;
    wait_for_same_workspace_state_with_diagnostics(
        "C and D must save the complete four-member history before A recovers",
        &[&engine_c, &engine_d],
        4,
        4,
    )
    .await;
    wait_for_member_with_diagnostics("C must observe D after D joins through C", &engine_c, &d_id)
        .await;

    let engine_a = device_a.start().await;
    recover(&engine_a).await;
    wait_for_converged_members_with_diagnostics(
        "A must reconnect with D after A recovers",
        &engine_a,
        &engine_d,
        &a_id,
        &d_id,
    )
    .await;
    send_and_verify(&engine_a, &engine_d, &d_id, "A to D through C").await;
    send_and_verify(&engine_d, &engine_a, &a_id, "D to A through C").await;

    for engine in [engine_a, engine_c, engine_d] {
        engine
            .shutdown(SHUTDOWN_TIMEOUT)
            .await
            .expect("final shutdown");
    }
}

// 场景流程：
// 1. A 创建空间，依次让 B、C、D、E 加入，并等待每次加入传到下一位操作设备。
// 2. A 依次移除 B、D；每个仍保留的设备各自接受对应移除。
// 3. C 让 B 重新加入，E 在收到该变化后让 D 重新加入。
// 4. 五台设备恢复为同一成员记录，并进行多组双向传递。
// 验证：连续移除和重新加入后，所有成员能够恢复到同一状态。
#[tokio::test(flavor = "multi_thread", worker_threads = 10)]
async fn five_devices_restore_full_sync_after_two_completed_removals_and_rejoins() {
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let harnesses = (0..5)
        .map(|_| DeviceHarness::new(rendezvous.uri()))
        .collect::<Vec<_>>();
    let engine_a = harnesses[0].start_local_only().await;
    let engine_b = harnesses[1].start_local_only().await;
    let engine_c = harnesses[2].start_local_only().await;
    let engine_d = harnesses[3].start_local_only().await;
    let engine_e = harnesses[4].start_local_only().await;
    let (space_id, a_id) = create_space(&engine_a, "Device A").await;
    let b_id = join_through(&engine_a, &engine_b, "Device B", &space_id).await;
    let c_id = join_through(&engine_a, &engine_c, "Device C", &space_id).await;
    let d_id = join_through(&engine_a, &engine_d, "Device D", &space_id).await;
    let e_id = join_through(&engine_a, &engine_e, "Device E", &space_id).await;
    wait_for_members(&engine_a, &[&b_id, &c_id, &d_id, &e_id]).await;
    // Each join must propagate before the next operation: the joiner has to
    // receive the continuous chain it is entitled to (receiver-side
    // evidence, not a local completion claim).
    for (engine, expected_changes) in [
        (&engine_b, 2),
        (&engine_c, 3),
        (&engine_d, 4),
        (&engine_e, 5),
    ] {
        wait_until(WAIT_TIMEOUT, || async {
            let c = workspace_convergence_summary(engine)
                .await
                .history_event_count;
            c >= expected_changes
        })
        .await;
    }

    for (removal_index, (device_id, expected_member_count)) in
        [(&b_id, 4), (&d_id, 3)].into_iter().enumerate()
    {
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
        // The removal must reach every retained member and be accepted on
        // each device before the next membership operation.
        let expected_changes = 5 + removal_index + 1;
        let retained_members: &[&Engine] = if removal_index == 0 {
            &[&engine_c, &engine_d, &engine_e]
        } else {
            &[&engine_c, &engine_e]
        };
        for engine in retained_members {
            wait_until(WAIT_TIMEOUT, || async {
                let summary = workspace_convergence_summary(engine).await;
                summary.history_event_count >= expected_changes as u64
                    && summary.pending_removal_decision_event_id.is_some()
            })
            .await;
            let removal_event_id = workspace_convergence_summary(engine)
                .await
                .pending_removal_decision_event_id
                .expect("retained member exposes the pending removal decision");
            engine
                .execute(Operation::DecideMembershipRemoval(
                    DecideMembershipRemovalInput {
                        removal_event_id,
                        decision: MembershipRemovalDecision::Accept,
                    },
                ))
                .await
                .expect("retained member accepts the removal");
            wait_until(WAIT_TIMEOUT, || async {
                let summary = workspace_convergence_summary(engine).await;
                summary.pending_removal_decision_event_id.is_none()
                    && summary.effective_member_count == expected_member_count
            })
            .await;
        }
    }

    let b_rejoin = join_through_with_result(&engine_c, &engine_b, "Device B", &space_id).await;
    assert_eq!(b_rejoin.self_device_id, b_id);
    assert_eq!(b_rejoin.migrated_records, Some(0));
    // B's rejoin change is carried by C's chain; wait until it has reached
    // E so D's rejoin through E appends to a current head. A real user
    // operates the next rejoin after the previous one has propagated.
    wait_until(WAIT_TIMEOUT, || async {
        workspace_convergence_summary(&engine_e)
            .await
            .history_event_count
            >= 8
    })
    .await;
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

// 场景流程：
// 1. A 创建空间，B、C 加入。
// 2. A 提交移除 B；C 收到后先保留待决定项，再明确接受。
// 3. A、C 必须收敛到只保留 A、C 的同一成员记录。
// 验证：远端移除只有在本机明确接受后才会生效，并能恢复一致状态。
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn member_removal_converges_across_three_independent_engine_directories() {
    uc_engine::init_test_tracing();
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

    wait_until(WAIT_TIMEOUT, || async {
        let c = workspace_convergence_summary(&engine_c).await;
        c.effective_member_count == 3 && c.pending_removal_decision_event_id.is_some()
    })
    .await;
    let pending_removal_event_id = workspace_convergence_summary(&engine_c)
        .await
        .pending_removal_decision_event_id
        .expect("C exposes the pending removal decision");

    let accepted = engine_c
        .execute(Operation::DecideMembershipRemoval(
            DecideMembershipRemovalInput {
                removal_event_id: pending_removal_event_id,
                decision: MembershipRemovalDecision::Accept,
            },
        ))
        .await
        .expect("C accepts the pending member removal");
    assert!(matches!(accepted, OperationResult::WorkspaceConvergence(_)));

    wait_until(WAIT_TIMEOUT, || async {
        let a = workspace_convergence_summary(&engine_a).await;
        let c = workspace_convergence_summary(&engine_c).await;
        a.effective_member_count == 2
            && c.effective_member_count == 2
            && c.pending_removal_decision_event_id.is_none()
            && a.convergence_digest == c.convergence_digest
            && a.convergence_digest.is_some()
    })
    .await;

    for engine in [engine_a, engine_b, engine_c] {
        engine
            .shutdown(SHUTDOWN_TIMEOUT)
            .await
            .expect("shut down member removal engine");
    }
}

// 场景流程：
// 1. A、B、C 建立同一空间，并先确认 B、C 可双向传递内容。
// 2. A 提交移除 B；B、C 都收到同一待决定项。
// 3. B、C 都拒绝，因而分别与 A 分叉，但 B、C 仍处在同一保留分支。
// 4. 验证 A 与 C 双向阻断，B 与 C 继续双向传递内容。
// 验证：拒绝只隔离相关分叉关系，不影响同一保留分支中的其他成员。
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn rejecting_a_member_removal_keeps_an_unaffected_member_connection_usable() {
    uc_engine::init_test_tracing();
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
    wait_for_members(&engine_b, &[&a_id, &c_id]).await;
    wait_for_members(&engine_c, &[&a_id, &b_id]).await;
    wait_for_converged_members(&engine_b, &engine_c, &b_id, &c_id).await;
    send_and_verify(
        &engine_c,
        &engine_b,
        &b_id,
        "C to B before rejecting A removal",
    )
    .await;
    send_and_verify(
        &engine_b,
        &engine_c,
        &c_id,
        "B to C before rejecting A removal",
    )
    .await;
    engine_a
        .execute(Operation::RemoveMember(RemoveMemberInput {
            device_id: b_id.clone(),
        }))
        .await
        .expect("A removes B");
    wait_for_pending_decisions(&engine_b, &engine_c).await;
    let pending_removal_event_id = workspace_convergence_summary(&engine_c)
        .await
        .pending_removal_decision_event_id
        .expect("C exposes the pending removal decision");
    let b_pending_removal_event_id = workspace_convergence_summary(&engine_b)
        .await
        .pending_removal_decision_event_id
        .expect("B exposes the pending removal decision");
    assert_eq!(pending_removal_event_id, b_pending_removal_event_id);

    let (c_decision, b_decision) = tokio::join!(
        engine_c.execute(Operation::DecideMembershipRemoval(
            DecideMembershipRemovalInput {
                removal_event_id: pending_removal_event_id,
                decision: MembershipRemovalDecision::Reject,
            },
        )),
        engine_b.execute(Operation::DecideMembershipRemoval(
            DecideMembershipRemovalInput {
                removal_event_id: b_pending_removal_event_id,
                decision: MembershipRemovalDecision::Reject,
            },
        )),
    );
    c_decision.expect("C rejects the removal");
    b_decision.expect("B rejects the removal");
    wait_until(WAIT_TIMEOUT, || async {
        let a = workspace_convergence_summary(&engine_a).await;
        let b = workspace_convergence_summary(&engine_b).await;
        let c = workspace_convergence_summary(&engine_c).await;
        a.diverged_peer_device_ids.len() == 2
            && a.diverged_peer_device_ids.contains(&b_id)
            && a.diverged_peer_device_ids.contains(&c_id)
            && b.diverged_peer_device_ids == vec![a_id.clone()]
            && c.diverged_peer_device_ids == vec![a_id.clone()]
            && c.effective_member_count == 3
    })
    .await;

    send_and_assert_blocked(
        &engine_a,
        &c_id,
        "A to C must be blocked after C rejects A removal",
    )
    .await;
    send_and_assert_blocked(
        &engine_c,
        &a_id,
        "C to A must be blocked after rejecting A removal",
    )
    .await;
    send_and_verify(
        &engine_c,
        &engine_b,
        &b_id,
        "C to B after rejecting A removal",
    )
    .await;
    send_and_verify(
        &engine_b,
        &engine_c,
        &c_id,
        "B to C after C rejects A removal",
    )
    .await;

    for engine in [engine_a, engine_b, engine_c] {
        engine
            .shutdown(SHUTDOWN_TIMEOUT)
            .await
            .expect("shut down rejection branch engine");
    }
}

// 场景流程：
// 1. A、B、C 建立同一空间；A 提交移除 B，B、C 都收到待决定项。
// 2. C 在未作决定时重启，恢复后必须仍看到原来的待决定项。
// 3. B、C 都拒绝该移除，分别与 A 分叉。
// 4. B 与 C 必须继续双向传递内容。
// 验证：待决定项可跨重启保存，拒绝后不会破坏不相关成员之间的传递。
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn pending_member_removal_survives_restart_before_a_rejection_is_decided() {
    uc_engine::init_test_tracing();
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
    wait_for_members(&engine_c, &[&a_id, &b_id]).await;
    engine_a
        .execute(Operation::RemoveMember(RemoveMemberInput {
            device_id: b_id.clone(),
        }))
        .await
        .expect("A removes B");
    wait_for_pending_decisions_with_diagnostics(
        "pending removal did not reach B and C before C restart",
        &engine_a,
        &engine_b,
        &engine_c,
    )
    .await;
    let pending_removal_event_id = workspace_convergence_summary(&engine_c)
        .await
        .pending_removal_decision_event_id
        .expect("C exposes the pending removal decision before restart");
    let b_pending_removal_event_id = workspace_convergence_summary(&engine_b)
        .await
        .pending_removal_decision_event_id
        .expect("B exposes the pending removal decision before C restarts");
    assert_eq!(pending_removal_event_id, b_pending_removal_event_id);

    engine_c
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shut down C with a pending removal decision");
    let engine_c = device_c.start_local_only().await;
    recover(&engine_c).await;
    let recovered = workspace_convergence_summary(&engine_c).await;
    assert_eq!(
        recovered.pending_removal_decision_event_id,
        Some(pending_removal_event_id.clone()),
        "restart must retain the original pending removal decision"
    );
    assert_eq!(recovered.effective_member_count, 3);

    let (c_decision, b_decision) = tokio::join!(
        engine_c.execute(Operation::DecideMembershipRemoval(
            DecideMembershipRemovalInput {
                removal_event_id: pending_removal_event_id,
                decision: MembershipRemovalDecision::Reject,
            },
        )),
        engine_b.execute(Operation::DecideMembershipRemoval(
            DecideMembershipRemovalInput {
                removal_event_id: b_pending_removal_event_id,
                decision: MembershipRemovalDecision::Reject,
            },
        )),
    );
    c_decision.expect("C rejects the recovered pending removal");
    b_decision.expect("B rejects the removal while C is restarting");
    wait_for_recovered_rejection_with_diagnostics(
        &engine_a, &engine_b, &engine_c, &a_id, &b_id, &c_id,
    )
    .await;
    send_and_verify(
        &engine_c,
        &engine_b,
        &b_id,
        "C to B after rejecting a recovered removal",
    )
    .await;
    send_and_verify(
        &engine_b,
        &engine_c,
        &c_id,
        "B to C after C rejects a recovered removal",
    )
    .await;

    for engine in [engine_a, engine_b, engine_c] {
        engine
            .shutdown(SHUTDOWN_TIMEOUT)
            .await
            .expect("shut down recovered rejection branch engine");
    }
}

// 场景流程：
// 1. A、B、C、D 建立同一空间，先确认 B、D 可双向传递内容。
// 2. A 提交移除 B；B、C、D 都收到同一待决定项。
// 3. C 接受，B、D 拒绝，因此 A、C 形成已移除 B 的分支，B、D 保留原分支。
// 4. 验证 A 与 D 双向阻断，A 与 C 可传递，D 与 B 仍可传递。
// 验证：同一移除可产生独立分支，分支内部继续工作，分支之间停止普通传递。
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_accept_and_reject_of_one_removal_keep_their_branches_independent() {
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let device_a = DeviceHarness::new(rendezvous.uri());
    let device_b = DeviceHarness::new(rendezvous.uri());
    let device_c = DeviceHarness::new(rendezvous.uri());
    let device_d = DeviceHarness::new(rendezvous.uri());
    let engine_a = device_a.start_local_only().await;
    let engine_b = device_b.start_local_only().await;
    let engine_c = device_c.start_local_only().await;
    let engine_d = device_d.start_local_only().await;
    let (space_id, a_id) = create_space(&engine_a, "Device A").await;
    let b_id = join_through(&engine_a, &engine_b, "Device B", &space_id).await;
    let c_id = join_through(&engine_a, &engine_c, "Device C", &space_id).await;
    let d_id = join_through(&engine_a, &engine_d, "Device D", &space_id).await;

    wait_for_members(&engine_a, &[&b_id, &c_id, &d_id]).await;
    wait_for_members(&engine_b, &[&d_id]).await;
    wait_for_members(&engine_c, &[&a_id, &b_id]).await;
    wait_for_members(&engine_d, &[&a_id, &b_id, &c_id]).await;
    send_and_verify(
        &engine_d,
        &engine_b,
        &b_id,
        "D to B before resolving A removal",
    )
    .await;
    send_and_verify(
        &engine_b,
        &engine_d,
        &d_id,
        "B to D before resolving A removal",
    )
    .await;
    engine_a
        .execute(Operation::RemoveMember(RemoveMemberInput {
            device_id: b_id.clone(),
        }))
        .await
        .expect("A removes B");
    wait_for_pending_decision_on_both_branches(&engine_a, &engine_b, &engine_c, &engine_d).await;
    wait_until(WAIT_TIMEOUT, || async {
        workspace_convergence_summary(&engine_b)
            .await
            .pending_removal_decision_event_id
            .is_some()
    })
    .await;
    let b_event_id = workspace_convergence_summary(&engine_b)
        .await
        .pending_removal_decision_event_id
        .expect("B exposes a pending removal");
    let c_event_id = workspace_convergence_summary(&engine_c)
        .await
        .pending_removal_decision_event_id
        .expect("C exposes a pending removal");
    let d_event_id = workspace_convergence_summary(&engine_d)
        .await
        .pending_removal_decision_event_id
        .expect("D exposes a pending removal");
    assert_eq!(b_event_id, c_event_id);
    assert_eq!(c_event_id, d_event_id);

    let (b_decision, c_decision, d_decision) = tokio::join!(
        engine_b.execute(Operation::DecideMembershipRemoval(
            DecideMembershipRemovalInput {
                removal_event_id: b_event_id,
                decision: MembershipRemovalDecision::Reject,
            },
        )),
        engine_c.execute(Operation::DecideMembershipRemoval(
            DecideMembershipRemovalInput {
                removal_event_id: c_event_id,
                decision: MembershipRemovalDecision::Accept,
            },
        )),
        engine_d.execute(Operation::DecideMembershipRemoval(
            DecideMembershipRemovalInput {
                removal_event_id: d_event_id,
                decision: MembershipRemovalDecision::Reject,
            },
        )),
    );
    assert!(matches!(
        b_decision.expect("B rejects the removal"),
        OperationResult::WorkspaceConvergence(_)
    ));
    assert!(matches!(
        c_decision.expect("C accepts the removal"),
        OperationResult::WorkspaceConvergence(_)
    ));
    assert!(matches!(
        d_decision.expect("D rejects the removal"),
        OperationResult::WorkspaceConvergence(_)
    ));

    wait_for_concurrent_decision_state(
        &engine_a, &engine_b, &engine_c, &engine_d, &a_id, &b_id, &c_id, &d_id,
    )
    .await;

    send_and_assert_blocked(
        &engine_a,
        &d_id,
        "A to D must be blocked after D rejects while C accepts",
    )
    .await;
    send_and_assert_blocked(
        &engine_d,
        &a_id,
        "D to A must be blocked after D rejects while C accepts",
    )
    .await;
    send_and_verify(
        &engine_a,
        &engine_c,
        &c_id,
        "A to C after C accepts the same removal",
    )
    .await;
    send_and_verify(
        &engine_d,
        &engine_b,
        &b_id,
        "D to B after D rejects the same removal",
    )
    .await;

    for engine in [engine_a, engine_b, engine_c, engine_d] {
        engine
            .shutdown(SHUTDOWN_TIMEOUT)
            .await
            .expect("shut down concurrent-decision branch engine");
    }
}

// 场景流程：
// 1. A、C 建立同一空间，C 将 A 的当前成员实例移除。
// 2. A 使用相同设备标识重新加入，但这次加入生成新的成员实例。
// 3. C 与重新加入后的 A 必须恢复为两个有效成员，并可传递内容。
// 验证：针对旧成员实例的历史移除不能误伤同一设备的新成员实例。
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn historical_removal_does_not_remove_a_fresh_joiner() {
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let device_c = DeviceHarness::new(rendezvous.uri());
    let device_a = DeviceHarness::new(rendezvous.uri());
    let device_d = DeviceHarness::new(rendezvous.uri());
    let engine_c = device_c.start_local_only().await;
    let engine_a = device_a.start_local_only().await;
    let engine_d = device_d.start_local_only().await;
    let (space_id, c_id) = create_space(&engine_c, "Device C").await;
    let a_id = join_through(&engine_c, &engine_a, "Device A", &space_id).await;
    wait_for_members(&engine_c, &[&a_id]).await;
    wait_for_members(&engine_a, &[&c_id]).await;

    // C removes A: the historical removal intent and removal change are
    // persisted before any later admission.
    engine_c
        .execute(Operation::RemoveMember(RemoveMemberInput {
            device_id: a_id.clone(),
        }))
        .await
        .expect("submit the historical member removal");
    wait_until(WAIT_TIMEOUT, || async {
        let c = workspace_convergence_summary(&engine_c).await;
        c.effective_member_count == 1
    })
    .await;

    // A fresh device D joins the space that already carries a historical
    // removal record. The record must only affect its exact old target and
    // must never mark the new joiner removed.
    let d_id = join_through(&engine_c, &engine_d, "Device D", &space_id).await;
    wait_until(WAIT_TIMEOUT, || async {
        let c = workspace_convergence_summary(&engine_c).await;
        let d = workspace_convergence_summary(&engine_d).await;
        c.effective_member_count == 2
            && d.effective_member_count == 2
            && !d.removed
            && d.phase != WorkspaceConvergencePhaseSummary::RecoveryRequired
            && c.convergence_digest.is_some()
            && c.convergence_digest == d.convergence_digest
    })
    .await;

    // The joiner must stay usable: the historical removal record arrives as
    // a recorded fact, not as a local removal, and content sync keeps
    // working in both directions.
    send_and_verify(
        &engine_c,
        &engine_d,
        &d_id,
        "C to D with a historical removal in the chain",
    )
    .await;
    send_and_verify(
        &engine_d,
        &engine_c,
        &c_id,
        "D to C with a historical removal in the chain",
    )
    .await;

    for engine in [engine_c, engine_a, engine_d] {
        engine
            .shutdown(SHUTDOWN_TIMEOUT)
            .await
            .expect("shut down historical removal joiner engine");
    }
}

// 场景流程：
// 1. A、C、D 建立同一空间，随后 A 离线。
// 2. C 移除离线的 A，D 收到后明确接受，C、D 进入较新的成员记录。
// 3. C 离线后，旧状态的 A 尝试作为发起方让 D 再次加入同一空间。
// 4. D 必须拒绝该加入，并保持 C、D 的较新成员记录；C 返回后也不能覆盖 D。
// 验证：旧状态设备不能用同一空间的加入流程覆盖已确认的新分支状态。
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn stale_removed_sponsor_cannot_replace_d_newer_same_space_state() {
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let device_a = DeviceHarness::new(rendezvous.uri());
    let device_c = DeviceHarness::new(rendezvous.uri());
    let device_d = DeviceHarness::new(rendezvous.uri());

    let engine_a = device_a.start_local_only().await;
    let engine_c = device_c.start_local_only().await;
    let engine_d = device_d.start_local_only().await;
    let (space_id, _c_id) = create_space(&engine_c, "Device C").await;
    let a_id = join_through(&engine_c, &engine_a, "Device A", &space_id).await;
    let _d_id = join_through(&engine_c, &engine_d, "Device D", &space_id).await;
    wait_for_same_workspace_state_with_diagnostics(
        "initial A/C/D history",
        &[&engine_a, &engine_c, &engine_d],
        3,
        3,
    )
    .await;
    let a_before_removal = workspace_convergence_summary(&engine_a).await;
    assert!(
        !a_before_removal.removed,
        "A must still consider its original member instance active before going offline"
    );

    engine_a
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shut down A before A is removed");
    engine_c
        .execute(Operation::RemoveMember(RemoveMemberInput {
            device_id: a_id.clone(),
        }))
        .await
        .expect("remove offline A from C");
    wait_until(WAIT_TIMEOUT, || async {
        workspace_convergence_summary(&engine_d)
            .await
            .pending_removal_decision_event_id
            .is_some()
    })
    .await;
    let d_removal_event_id = workspace_convergence_summary(&engine_d)
        .await
        .pending_removal_decision_event_id
        .expect("D exposes C's removal of offline A for a local decision");
    engine_d
        .execute(Operation::DecideMembershipRemoval(
            DecideMembershipRemovalInput {
                removal_event_id: d_removal_event_id,
                decision: MembershipRemovalDecision::Accept,
            },
        ))
        .await
        .expect("D accepts C's removal of offline A");
    wait_for_same_workspace_state_with_diagnostics(
        "C and D apply removal of offline A",
        &[&engine_c, &engine_d],
        2,
        4,
    )
    .await;
    let d_before_stale_join = workspace_convergence_summary(&engine_d).await;
    assert_receive_ready(&engine_d, true).await;

    engine_c
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shut down C before stale A sponsors D");
    let engine_a = device_a.start_local_only().await;
    recover(&engine_a).await;
    let a_stale_state = workspace_convergence_summary(&engine_a).await;
    assert!(
        !a_stale_state.removed,
        "offline A must not have learned that its member instance was removed"
    );
    assert_eq!(
        a_stale_state.history_event_count, a_before_removal.history_event_count,
        "A must restart on the same old change chain it held before removal"
    );
    assert_eq!(
        a_stale_state.convergence_digest, a_before_removal.convergence_digest,
        "A must restart on the same old digest it held before removal"
    );
    assert!(
        a_stale_state.history_event_count < d_before_stale_join.history_event_count,
        "A must be behind D before the stale admission attempt; A={a_stale_state:?}; D={d_before_stale_join:?}"
    );
    assert_ne!(
        a_stale_state.convergence_digest, d_before_stale_join.convergence_digest,
        "A and D must hold different same-Space states before the stale admission attempt"
    );

    let addresses = engine_a
        .execute_dev(DevOperation::ListPairingInvitationAddresses)
        .await
        .expect("list stale A invitation addresses");
    let DevOperationResult::PairingInvitationAddresses(mut addresses) = addresses else {
        panic!("unexpected invitation address result");
    };
    addresses.sort_by_key(|address| (!address.ip.is_loopback(), !address.ip.is_ipv4()));
    let address = addresses
        .into_iter()
        .next()
        .expect("stale A must expose an invitation address");
    let invitation = engine_a
        .execute_dev(DevOperation::IssueInvitationForAddress {
            address: address.ip,
        })
        .await
        .expect("issue stale A invitation");
    let DevOperationResult::InvitationIssued(invitation) = invitation else {
        panic!("unexpected invitation result");
    };

    let stale_join = engine_d.execute(Operation::JoinSpace(JoinSpaceInput {
        invitation_code: invitation.code,
        device_name: Some("Device D".to_owned()),
        passphrase: SecretString::new(PASSPHRASE),
        preserve_unreadable_history: false,
    }));
    tokio::pin!(stale_join);
    let mut observed_a_during_join = false;
    let stale_join = tokio::time::timeout(WAIT_TIMEOUT, async {
        loop {
            let peer_query = engine_d.execute(Operation::QueryPeerConnections);
            tokio::pin!(peer_query);
            tokio::select! {
                result = &mut stale_join => break result,
                result = &mut peer_query => {
                    let peer_ids = peer_ids_from_result(
                        result.expect("query D peers while stale join is running"),
                    );
                    observed_a_during_join |= peer_ids.contains(&a_id);
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            }
        }
    })
    .await
    .expect("stale same-Space join must decide before the deadline");
    let d_after_stale_join = workspace_convergence_summary(&engine_d).await;
    let d_members_after_stale_join = list_peer_ids(&engine_d).await;
    observed_a_during_join |= d_members_after_stale_join.contains(&a_id);

    let stale_error =
        stale_join.expect_err("D must reject stale A for the same Space before B and C return");
    assert_eq!(stale_error.category(), EngineErrorCategory::Conflict);
    assert!(!stale_error.is_retryable());
    assert_eq!(
        d_after_stale_join.convergence_digest, d_before_stale_join.convergence_digest,
        "a rejected stale same-Space join must preserve D's convergence state"
    );
    assert_eq!(
        d_after_stale_join.history_event_count, d_before_stale_join.history_event_count,
        "a rejected stale same-Space join must preserve D's change count"
    );
    assert_eq!(
        d_after_stale_join.effective_member_count, d_before_stale_join.effective_member_count,
        "a rejected stale same-Space join must preserve D's effective members"
    );
    assert!(
        !observed_a_during_join,
        "A must never appear in D's peer list during the rejected stale join; \
         before={d_before_stale_join:?}; after={d_after_stale_join:?}; \
         peers_after={d_members_after_stale_join:?}"
    );
    assert_receive_ready(&engine_d, true).await;

    let engine_c = device_c.start_local_only().await;
    recover(&engine_c).await;
    wait_for_same_workspace_state_with_diagnostics(
        "C returns after D rejects stale A",
        &[&engine_c, &engine_d],
        2,
        4,
    )
    .await;
    let d_after_recovery = workspace_convergence_summary(&engine_d).await;
    assert_eq!(
        d_after_recovery.convergence_digest, d_before_stale_join.convergence_digest,
        "returning C must not let the rejected stale sponsor replace D's state"
    );

    let (c_shutdown, d_shutdown, a_shutdown) = tokio::join!(
        engine_c.shutdown(SHUTDOWN_TIMEOUT),
        engine_d.shutdown(SHUTDOWN_TIMEOUT),
        engine_a.shutdown(SHUTDOWN_TIMEOUT),
    );
    for (device, result) in [("C", c_shutdown), ("D", d_shutdown), ("A", a_shutdown)] {
        result.unwrap_or_else(|error| {
            panic!("shut down stale sponsor test engine {device}: {error:?}")
        });
    }
}

// 场景流程：
// 1. C 创建空间，A 加入。
// 2. C 移除 A，使 A 的旧成员实例失效。
// 3. A 再次通过 C 加入，并取得新的成员实例。
// 4. C、A 必须恢复为两个有效成员，A 不能显示为已移除，并可接收内容。
// 验证：被移除设备重新加入时，旧移除记录不会应用到新成员实例。
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn removed_device_rejoins_under_a_new_instance_without_stale_removal() {
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let device_c = DeviceHarness::new(rendezvous.uri());
    let device_a = DeviceHarness::new(rendezvous.uri());
    let engine_c = device_c.start_local_only().await;
    let engine_a = device_a.start_local_only().await;
    let (space_id, c_id) = create_space(&engine_c, "Device C").await;
    let a_id = join_through(&engine_c, &engine_a, "Device A", &space_id).await;
    wait_for_members(&engine_c, &[&a_id]).await;
    wait_for_members(&engine_a, &[&c_id]).await;

    engine_c
        .execute(Operation::RemoveMember(RemoveMemberInput {
            device_id: a_id.clone(),
        }))
        .await
        .expect("submit the member removal");
    wait_until(WAIT_TIMEOUT, || async {
        workspace_convergence_summary(&engine_c)
            .await
            .effective_member_count
            == 1
    })
    .await;

    // The same device rejoins: the admission creates a new member instance
    // and the old removal record must not mark the new instance removed.
    let rejoin = join_through_with_result(&engine_c, &engine_a, "Device A", &space_id).await;
    assert_eq!(rejoin.self_device_id, a_id);
    assert_eq!(rejoin.migrated_records, Some(0));
    let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
    loop {
        let c = workspace_convergence_summary(&engine_c).await;
        let a = workspace_convergence_summary(&engine_a).await;
        if c.effective_member_count == 2
            && a.effective_member_count == 2
            && !a.removed
            && a.phase != WorkspaceConvergencePhaseSummary::RecoveryRequired
            && c.convergence_digest.is_some()
            && c.convergence_digest == a.convergence_digest
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "rejoin did not converge; C={c:?}; A={a:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    send_and_verify(
        &engine_c,
        &engine_a,
        &a_id,
        "C to A after A rejoined under a new instance",
    )
    .await;

    for engine in [engine_c, engine_a] {
        engine
            .shutdown(SHUTDOWN_TIMEOUT)
            .await
            .expect("shut down rejoin engine");
    }
}

// 场景流程：
// 1. A、B、C 建立同一空间。
// 2. A 移除 B；C 收到后明确接受，A、C 收敛到只保留两人的成员记录。
// 3. A 再移除 C，后续移除必须基于已经恢复的当前成员记录。
// 验证：一项移除完成并恢复一致后，可以从该结果继续下一项移除。
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn completed_removal_can_continue_from_the_recovered_member_state() {
    uc_engine::init_test_tracing();
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
        workspace_convergence_summary(&engine_c)
            .await
            .pending_removal_decision_event_id
            .is_some()
    })
    .await;
    let removal_event_id = workspace_convergence_summary(&engine_c)
        .await
        .pending_removal_decision_event_id
        .expect("C exposes the first removal for a local decision");
    assert!(matches!(
        engine_c
            .execute(Operation::DecideMembershipRemoval(
                DecideMembershipRemovalInput {
                    removal_event_id,
                    decision: MembershipRemovalDecision::Accept,
                },
            ))
            .await
            .expect("C accepts the first removal"),
        OperationResult::WorkspaceConvergence(_)
    ));
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

// 场景流程：
// 1. A、B 建立同一空间，B 离线。
// 2. A 向 B 发送内容，发送结果只记录 B 当前离线。
// 3. A 先重启恢复，再由 B 重启恢复。
// 4. B 必须实际收到离线期间发送的内容。
// 验证：离线投递记录可跨发送方重启保存，并在接收方恢复后完成送达。
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn offline_clipboard_delivery_reaches_the_receiver_after_it_restarts() {
    uc_engine::init_test_tracing();
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

// 场景流程：
// 1. A、B 建立同一空间，B 离线。
// 2. A 连续向 B 发送较早内容和较新内容。
// 3. B 恢复后，只能收到较新内容，不能收到已被替代的较早内容。
// 验证：离线期间同一接收方只保留最新待送内容，避免旧内容在恢复后倒灌。
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn offline_clipboard_delivery_only_sends_the_latest_content_when_the_receiver_returns() {
    uc_engine::init_test_tracing();
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

// 场景流程：
// 1. 第一台发起方创建空间，新的设备加入。
// 2. 第二台发起方为同一空间创建新的加入入口。
// 3. 已加入设备使用该入口时，必须切换到同一空间，而不是重复创建成员或空间。
// 验证：新设备加入和已加入设备切换使用稳定、可区分的结果路径。
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn stable_join_routes_a_fresh_device_then_switches_an_existing_device() {
    uc_engine::init_test_tracing();
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

async fn wait_for_member_with_diagnostics(stage: &str, engine: &Engine, expected_id: &str) {
    let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
    loop {
        let devices = list_devices(engine).await;
        if devices.iter().any(|device| device.device_id == expected_id) {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            let summary = workspace_convergence_summary(engine).await;
            panic!(
                "{stage} before timeout: summary={summary:?}, observed_device_count={}",
                devices.len()
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_for_converged_members(engine_a: &Engine, engine_c: &Engine, a_id: &str, c_id: &str) {
    wait_until(WAIT_TIMEOUT, || async {
        has_complete_membership(engine_a, c_id).await
            && has_complete_membership(engine_c, a_id).await
    })
    .await;
}

async fn wait_for_converged_members_with_diagnostics(
    stage: &str,
    engine_a: &Engine,
    engine_d: &Engine,
    a_id: &str,
    d_id: &str,
) {
    let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
    loop {
        if has_complete_membership(engine_a, d_id).await
            && has_complete_membership(engine_d, a_id).await
        {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            let a = workspace_convergence_summary(engine_a).await;
            let d = workspace_convergence_summary(engine_d).await;
            panic!("{stage} before timeout: A={a:?}, D={d:?}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
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

async fn wait_for_same_workspace_state(
    engines: &[&Engine],
    expected_member_count: u64,
    minimum_history_event_count: u64,
) {
    wait_until(WAIT_TIMEOUT, || async {
        let mut expected_digest = None;
        for engine in engines {
            let summary = workspace_convergence_summary(engine).await;
            if summary.effective_member_count != expected_member_count
                || summary.history_event_count < minimum_history_event_count
                || summary.removed
                || summary.failure_category.is_some()
            {
                return false;
            }
            let Some(digest) = summary.convergence_digest else {
                return false;
            };
            if let Some(expected) = &expected_digest {
                if expected != &digest {
                    return false;
                }
            } else {
                expected_digest = Some(digest);
            }
        }
        true
    })
    .await;
}

async fn wait_for_same_workspace_state_with_diagnostics(
    stage: &str,
    engines: &[&Engine],
    expected_member_count: u64,
    minimum_history_event_count: u64,
) {
    let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
    loop {
        let mut summaries = Vec::with_capacity(engines.len());
        for engine in engines {
            summaries.push(workspace_convergence_summary(engine).await);
        }
        let expected_digest = summaries
            .first()
            .and_then(|summary| summary.convergence_digest.clone());
        let converged = expected_digest.is_some()
            && summaries.iter().all(|summary| {
                summary.effective_member_count == expected_member_count
                    && summary.history_event_count >= minimum_history_event_count
                    && !summary.removed
                    && summary.failure_category.is_none()
                    && summary.convergence_digest == expected_digest
            });
        if converged {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{stage} did not converge; summaries={summaries:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_for_pending_decision_on_both_branches(
    engine_a: &Engine,
    engine_b: &Engine,
    engine_c: &Engine,
    engine_d: &Engine,
) {
    let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
    loop {
        let c = workspace_convergence_summary(engine_c).await;
        let d = workspace_convergence_summary(engine_d).await;
        if c.pending_removal_decision_event_id.is_some()
            && d.pending_removal_decision_event_id.is_some()
        {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            let a = workspace_convergence_summary(engine_a).await;
            let b = workspace_convergence_summary(engine_b).await;
            panic!(
                "C and D did not both receive the pending removal before timeout: \
                 A={a:?}, B={b:?}, C={c:?}, D={d:?}"
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_for_pending_decisions(first: &Engine, second: &Engine) {
    wait_until(WAIT_TIMEOUT, || async {
        workspace_convergence_summary(first)
            .await
            .pending_removal_decision_event_id
            .is_some()
            && workspace_convergence_summary(second)
                .await
                .pending_removal_decision_event_id
                .is_some()
    })
    .await;
}

async fn wait_for_pending_decisions_with_diagnostics(
    stage: &str,
    engine_a: &Engine,
    engine_b: &Engine,
    engine_c: &Engine,
) {
    let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
    loop {
        let b = workspace_convergence_summary(engine_b).await;
        let c = workspace_convergence_summary(engine_c).await;
        if b.pending_removal_decision_event_id.is_some()
            && c.pending_removal_decision_event_id.is_some()
        {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            let a = workspace_convergence_summary(engine_a).await;
            panic!("{stage}: A={a:?}, B={b:?}, C={c:?}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_for_recovered_rejection_with_diagnostics(
    engine_a: &Engine,
    engine_b: &Engine,
    engine_c: &Engine,
    a_id: &str,
    b_id: &str,
    c_id: &str,
) {
    let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
    loop {
        let a = workspace_convergence_summary(engine_a).await;
        let b = workspace_convergence_summary(engine_b).await;
        let c = workspace_convergence_summary(engine_c).await;
        if a.diverged_peer_device_ids.len() == 2
            && a.diverged_peer_device_ids.iter().any(|peer| peer == b_id)
            && a.diverged_peer_device_ids.iter().any(|peer| peer == c_id)
            && b.diverged_peer_device_ids == [a_id]
            && c.diverged_peer_device_ids == [a_id]
        {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!(
                "recovered rejection did not establish only the A/C divergence: \
                 A={a:?}, B={b:?}, C={c:?}"
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_for_concurrent_decision_state(
    engine_a: &Engine,
    engine_b: &Engine,
    engine_c: &Engine,
    engine_d: &Engine,
    a_id: &str,
    b_id: &str,
    c_id: &str,
    d_id: &str,
) {
    let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
    loop {
        let a = workspace_convergence_summary(engine_a).await;
        let b = workspace_convergence_summary(engine_b).await;
        let c = workspace_convergence_summary(engine_c).await;
        let d = workspace_convergence_summary(engine_d).await;
        if a.effective_member_count == 3
            && c.effective_member_count == 3
            && a.convergence_digest == c.convergence_digest
            && a.convergence_digest.is_some()
            && a.diverged_peer_device_ids.len() == 2
            && a.diverged_peer_device_ids.iter().any(|peer| peer == b_id)
            && a.diverged_peer_device_ids.iter().any(|peer| peer == d_id)
            && c.diverged_peer_device_ids.len() == 2
            && c.diverged_peer_device_ids.iter().any(|peer| peer == b_id)
            && c.diverged_peer_device_ids.iter().any(|peer| peer == d_id)
            && b.diverged_peer_device_ids.len() == 2
            && b.diverged_peer_device_ids.iter().any(|peer| peer == a_id)
            && b.diverged_peer_device_ids.iter().any(|peer| peer == c_id)
            && d.diverged_peer_device_ids.len() == 2
            && d.diverged_peer_device_ids.iter().any(|peer| peer == a_id)
            && d.diverged_peer_device_ids.iter().any(|peer| peer == c_id)
            && b.convergence_digest == d.convergence_digest
            && b.convergence_digest.is_some()
            && b.effective_member_count == 4
            && d.effective_member_count == 4
        {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!(
                "concurrent accept/reject did not reach the expected branch state before timeout: \
                 A={a:?}, B={b:?}, C={c:?}, D={d:?}"
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn has_complete_membership(engine: &Engine, peer_id: &str) -> bool {
    let devices = list_devices(engine).await;
    let has_peer = devices
        .iter()
        .any(|device| device.device_id == peer_id && device.online);
    let has_peer_connection = list_peer_ids(engine)
        .await
        .iter()
        .any(|connected_peer_id| connected_peer_id == peer_id);
    let Ok(OperationResult::WorkspaceConvergence(summary)) =
        engine.execute(Operation::QueryWorkspaceConvergence).await
    else {
        return false;
    };
    // The workspace chain must be readable and the peer must be present in
    // both the public roster and the connection view used by dispatch.
    summary.phase != WorkspaceConvergencePhaseSummary::RecoveryRequired
        && has_peer
        && has_peer_connection
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

async fn list_peer_ids(engine: &Engine) -> Vec<String> {
    list_peers(engine)
        .await
        .into_iter()
        .map(|peer| peer.peer_id)
        .collect()
}

async fn list_peers(engine: &Engine) -> Vec<uc_engine::PeerConnectionSummary> {
    let result = engine
        .execute(Operation::QueryPeerConnections)
        .await
        .expect("query peer connections");
    peers_from_result(result)
}

fn peer_ids_from_result(result: OperationResult) -> Vec<String> {
    peers_from_result(result)
        .into_iter()
        .map(|peer| peer.peer_id)
        .collect()
}

fn peers_from_result(result: OperationResult) -> Vec<uc_engine::PeerConnectionSummary> {
    let OperationResult::PeerConnections(peers) = result else {
        panic!("unexpected peer connections result: {result:?}");
    };
    peers
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
    if report.total_accepted != 1 {
        let sender_summary = workspace_convergence_summary(sender).await;
        let receiver_summary = workspace_convergence_summary(receiver).await;
        let sender_devices = list_devices(sender).await;
        let receiver_devices = list_devices(receiver).await;
        panic!(
            "content send was not accepted: report={report:?}, sender={sender_summary:?}, \
             receiver={receiver_summary:?}, sender_devices={sender_devices:?}, \
             receiver_devices={receiver_devices:?}"
        );
    }
    assert_eq!(report.total_errored, 0);
    assert_eq!(report.total_offline, 0);
    assert_eq!(report.per_target.len(), 1);
    assert_eq!(report.per_target[0].device_id, target_id);
    assert_eq!(report.per_target[0].outcome, SendTargetOutcome::Accepted);

    wait_for_received_text(sender, receiver, text).await;
}

async fn wait_for_received_text(sender: &Engine, receiver: &Engine, text: &str) {
    let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
    loop {
        if receiver_has_exact_text(receiver, text).await {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            let sender_summary = workspace_convergence_summary(sender).await;
            let receiver_summary = workspace_convergence_summary(receiver).await;
            panic!(
                "content delivery did not reach the receiver before timeout: \
                 sender={sender_summary:?}, receiver={receiver_summary:?}"
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn send_and_assert_blocked(sender: &Engine, target_id: &str, text: &str) {
    let sent = sender
        .execute(Operation::SendText(SendTextInput {
            text: text.to_owned(),
            target_devices: vec![target_id.to_owned()],
        }))
        .await
        .expect("send text to a diverged member");
    let OperationResult::EntrySent(report) = sent else {
        panic!("unexpected send result: {sent:?}");
    };
    assert_eq!(report.total_accepted, 0);
    assert_eq!(report.total_offline, 0);
    assert_eq!(report.total_errored, 0);
    assert_eq!(report.per_target.len(), 0);
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

fn send_v019_text(cli: &Path, profile: &str, peer: &str, text: &str) {
    let output = Command::new(cli)
        .args(["send", "--json", "--peer", peer, text])
        .env("UC_PROFILE", profile)
        .output()
        .expect("run the official v0.19 CLI send command");
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("v0.19 CLI send returns JSON");
    assert_eq!(
        report
            .get("totalAccepted")
            .and_then(serde_json::Value::as_u64),
        Some(0)
    );
    assert_eq!(
        report
            .get("totalOffline")
            .and_then(serde_json::Value::as_u64),
        Some(1)
    );
    assert_eq!(
        report
            .get("totalErrored")
            .and_then(serde_json::Value::as_u64),
        Some(0)
    );
}

fn required_test_port(name: &str) -> u16 {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("{name} must identify the fixed test network port"))
        .parse::<u16>()
        .unwrap_or_else(|_| panic!("{name} must be a valid network port"))
}

async fn stop_v019_peer(pid: u32, data_root: &Path) {
    let expected_working_dir = data_root
        .parent()
        .and_then(Path::parent)
        .expect("v0.19 peer data root must be below its binary directory")
        .canonicalize()
        .expect("resolve expected v0.19 peer binary directory");
    let output = Command::new("lsof")
        .args(["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"])
        .output()
        .expect("inspect the v0.19 peer working directory");
    assert!(
        output.status.success(),
        "the requested v0.19 peer process must still be running"
    );
    let actual_working_dir = String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix('n'))
        .map(PathBuf::from)
        .expect("lsof must report the v0.19 peer working directory")
        .canonicalize()
        .expect("resolve actual v0.19 peer working directory");
    assert_eq!(
        actual_working_dir, expected_working_dir,
        "refuse to stop a process outside the requested v0.19 peer directory"
    );

    let status = Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status()
        .expect("request a normal v0.19 peer shutdown");
    assert!(status.success(), "stop the requested v0.19 peer process");

    wait_until(SHUTDOWN_TIMEOUT, || async {
        !Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    })
    .await;
}

fn v019_cli_has_exact_text(cli: &Path, profile: &str, expected_text: &str) -> bool {
    let output = Command::new(cli)
        .args(["get", "--list", "--json", "--limit", "100"])
        .env("UC_PROFILE", profile)
        .output()
        .expect("run the official v0.19 CLI history command");
    assert!(
        output.status.success(),
        "v0.19 CLI history failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let entries: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("v0.19 CLI history returns JSON");
    entries.as_array().is_some_and(|entries| {
        entries.iter().any(|entry| {
            entry.get("preview").and_then(serde_json::Value::as_str) == Some(expected_text)
        })
    })
}

async fn assert_v019_cli_never_observes_text(cli: &Path, profile: &str, expected_text: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        assert!(
            !v019_cli_has_exact_text(cli, profile, expected_text),
            "the v0.19 receiver must not save content from a 1.1 peer"
        );
        if tokio::time::Instant::now() >= deadline {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn assert_engine_never_observes_text(engine: &Engine, expected_text: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        assert!(
            !receiver_has_exact_text(engine, expected_text).await,
            "the 1.1 receiver must not save content from a v0.19 peer"
        );
        if tokio::time::Instant::now() >= deadline {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
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
