//! 入站拒绝原因必须在拒绝方的实际导出文件中可区分（执行计划 2026-09-23-inbound-peer-admission 切片 S2）。
//!
//! 使用真实 iroh 连接和现有公开构造入口驱动 presence 与剪贴板接收入站；每次拒绝恰好留下一条
//! `peer.inbound.rejected`，对端看到的拒绝字节保持不变，导出不含设备标识或名称。
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uc_application::deps::{KnownPeerIdentity, MembershipLedgerError, PeerIdentityDirectoryPort};

use async_trait::async_trait;
use bytes::Bytes;
use iroh::{endpoint::presets, protocol::Router, Endpoint, RelayMode};
use serde_json::Value;
use uc_core::ids::DeviceId;
use uc_core::membership::{
    MemberRepositoryPort, MemberSyncPreferences, MembershipError, PeerAdmissionError,
    PeerAdmissionPort, SpaceMember,
};
use uc_core::ports::security::IdentityFingerprintFactoryPort;
use uc_core::ports::{
    ClipboardHeader, PeerAddressError, PeerAddressRecord, PeerAddressRepositoryPort,
    PeerReachabilityPort,
};
use uc_infra::network::iroh::{
    clipboard_wire, IrohClipboardReceiverAdapter, IrohPeerReachabilityAdapter, CLIPBOARD_ALPN,
    PEER_REACHABILITY_ALPN,
};
use uc_infra::security::Sha256IdentityFingerprintFactory;
use uc_infra::SystemClock;
use uc_observability_runtime::*;

const ADMISSION_REQUEST: u8 = 1;
const ADMISSION_ACCEPTED: u8 = 1;
const ADMISSION_REJECTED: u8 = 2;
const EVENT: &str = "peer.inbound.rejected";

struct Members {
    members: Vec<SpaceMember>,
    fail_list: bool,
}

#[async_trait]
impl MemberRepositoryPort for Members {
    async fn get(&self, device_id: &DeviceId) -> Result<Option<SpaceMember>, MembershipError> {
        Ok(self
            .members
            .iter()
            .find(|member| &member.device_id == device_id)
            .cloned())
    }
    async fn list(&self) -> Result<Vec<SpaceMember>, MembershipError> {
        if self.fail_list {
            Err(MembershipError::Repository("private-list-failure".into()))
        } else {
            Ok(self.members.clone())
        }
    }
    async fn save(&self, _: &SpaceMember) -> Result<(), MembershipError> {
        panic!("read-only fixture")
    }
    async fn remove(&self, _: &DeviceId) -> Result<bool, MembershipError> {
        panic!("read-only fixture")
    }
}

#[async_trait]
impl PeerIdentityDirectoryPort for Members {
    async fn known_peer_identities(&self) -> Result<Vec<KnownPeerIdentity>, MembershipLedgerError> {
        Ok(self
            .list()
            .await
            .map_err(MembershipLedgerError::unavailable_from)?
            .into_iter()
            .map(|member| KnownPeerIdentity {
                device_id: member.device_id,
                identity_fingerprint: member.identity_fingerprint,
            })
            .collect())
    }
}

#[derive(Clone, Copy)]
enum Decision {
    Admit,
    Deny,
    Fail,
}

struct Admission {
    decision: Decision,
    checked: Mutex<Vec<DeviceId>>,
}

impl Admission {
    fn new(decision: Decision) -> Arc<Self> {
        Arc::new(Self {
            decision,
            checked: Mutex::new(Vec::new()),
        })
    }
    fn checked(&self) -> Vec<DeviceId> {
        self.checked.lock().expect("checked").clone()
    }
}

#[async_trait]
impl PeerAdmissionPort for Admission {
    async fn is_admitted(&self, device_id: &DeviceId) -> Result<bool, PeerAdmissionError> {
        self.checked
            .lock()
            .expect("checked")
            .push(device_id.clone());
        match self.decision {
            Decision::Admit => Ok(true),
            Decision::Deny => Ok(false),
            Decision::Fail => Err(PeerAdmissionError::Unavailable),
        }
    }
}

struct NoAddresses;

#[async_trait]
impl PeerAddressRepositoryPort for NoAddresses {
    async fn get(&self, _: &DeviceId) -> Result<Option<PeerAddressRecord>, PeerAddressError> {
        Ok(None)
    }
    async fn upsert(&self, _: &PeerAddressRecord) -> Result<(), PeerAddressError> {
        Ok(())
    }
    async fn list(&self) -> Result<Vec<PeerAddressRecord>, PeerAddressError> {
        Ok(Vec::new())
    }
    async fn remove(&self, _: &DeviceId) -> Result<(), PeerAddressError> {
        Ok(())
    }
}

async fn endpoint() -> Arc<Endpoint> {
    let endpoint = Arc::new(
        Endpoint::builder(presets::N0)
            .relay_mode(RelayMode::Disabled)
            .clear_address_lookup()
            .bind()
            .await
            .expect("endpoint"),
    );
    for _ in 0..100 {
        if !endpoint.addr().addrs.is_empty() {
            return endpoint;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("direct address unavailable")
}

fn member(endpoint: &Endpoint, device: &str) -> SpaceMember {
    SpaceMember {
        device_id: DeviceId::new(device),
        device_name: "private-name-sentinel".to_owned(),
        identity_fingerprint: Sha256IdentityFingerprintFactory
            .from_public_key(endpoint.id().as_bytes())
            .expect("fingerprint"),
        joined_at: chrono::Utc::now(),
        sync_preferences: MemberSyncPreferences::default(),
    }
}

fn stranger(device: &str) -> SpaceMember {
    SpaceMember {
        device_id: DeviceId::new(device),
        device_name: "private-name-sentinel".to_owned(),
        identity_fingerprint: Sha256IdentityFingerprintFactory
            .from_public_key(&[0x5a; 32])
            .expect("fingerprint"),
        joined_at: chrono::Utc::now(),
        sync_preferences: MemberSyncPreferences::default(),
    }
}

/// 发起一次 presence 准入确认，返回接收方回写的字节。
async fn presence(
    dialer: &Endpoint,
    members: Members,
    admission: Arc<Admission>,
    stop_accepting_first: bool,
) -> u8 {
    let receiver = endpoint().await;
    let adapter = IrohPeerReachabilityAdapter::new(
        Arc::clone(&receiver),
        Arc::new(NoAddresses),
        Arc::new(members),
        admission,
        Arc::new(Sha256IdentityFingerprintFactory),
        Arc::new(SystemClock),
    );
    if stop_accepting_first {
        adapter.disconnect_all().await;
    }
    let router = Router::builder((*receiver).clone())
        .accept(PEER_REACHABILITY_ALPN, adapter.handler())
        .spawn();
    let connection = dialer
        .connect(receiver.addr(), PEER_REACHABILITY_ALPN)
        .await
        .expect("connect");
    let (mut send, mut receive) = connection.open_bi().await.expect("stream");
    send.write_all(&[ADMISSION_REQUEST]).await.expect("request");
    send.finish().expect("finish");
    let mut response = [0u8; 1];
    tokio::time::timeout(Duration::from_secs(5), receive.read_exact(&mut response))
        .await
        .expect("bounded response")
        .expect("response");
    connection.close(0u32.into(), b"acceptance");
    router.shutdown().await.expect("router");
    receiver.close().await;
    response[0]
}

/// 发送一帧剪贴板内容，返回接收方的回执字节。
async fn clipboard(dialer: &Endpoint, members: Members, admission: Arc<Admission>) -> u8 {
    let receiver = endpoint().await;
    let adapter = IrohClipboardReceiverAdapter::new(
        Arc::clone(&receiver),
        Arc::new(members),
        admission,
        Arc::new(Sha256IdentityFingerprintFactory),
    );
    let router = Router::builder((*receiver).clone())
        .accept(CLIPBOARD_ALPN, adapter.handler())
        .spawn();
    let connection = dialer
        .connect(receiver.addr(), CLIPBOARD_ALPN)
        .await
        .expect("connect");
    let (mut send, mut receive) = connection.open_bi().await.expect("stream");
    clipboard_wire::write_frame(
        &mut send,
        &ClipboardHeader {
            version: ClipboardHeader::CURRENT_VERSION,
            snapshot_hash: "private-hash-sentinel".into(),
            captured_at_ms: 1,
            origin_device_id: "private-device-sentinel".into(),
            origin_device_name: "private-name-sentinel".into(),
            payload_version: 3,
        },
        &Bytes::from_static(b"private-content-sentinel"),
    )
    .await
    .expect("frame");
    send.finish().expect("finish");
    let mut ack = [0u8; 1];
    tokio::time::timeout(Duration::from_secs(5), receive.read_exact(&mut ack))
        .await
        .expect("bounded ack")
        .expect("ack");
    connection.close(0u32.into(), b"acceptance");
    router.shutdown().await.expect("router");
    receiver.close().await;
    ack[0]
}

#[tokio::test]
async fn inbound_rejections_are_classified_in_the_exported_file() {
    let logs = tempfile::tempdir().expect("logs");
    let handle = ProcessObservabilityRuntime::install(
        ObservabilityConfig::new(
            ObservabilityResource::new(
                "1.1.0",
                DeploymentEnvironment::Test,
                OperatingSystem::Macos,
                "test",
            )
            .expect("resource"),
        )
        .with_local_logs(LocalLogConfig::new(logs.path())),
    )
    .expect("runtime")
    .handle();
    let dialer = endpoint().await;
    let active = member(&dialer, "private-device-active");
    let mut duplicate = active.clone();
    duplicate.device_id = DeviceId::new("private-device-stale");
    let only = |members: Vec<SpaceMember>| Members {
        members,
        fail_list: false,
    };
    let failing = || Members {
        members: Vec::new(),
        fail_list: true,
    };

    // presence：对端只看到固定拒绝字节，拒绝方本地分出原因。
    let unused = Admission::new(Decision::Admit);
    assert_eq!(
        presence(
            &dialer,
            only(vec![stranger("private-device-other")]),
            Arc::clone(&unused),
            false
        )
        .await,
        ADMISSION_REJECTED
    );
    assert_eq!(
        presence(&dialer, failing(), Arc::clone(&unused), false).await,
        ADMISSION_REJECTED
    );
    assert_eq!(
        presence(
            &dialer,
            only(vec![duplicate.clone(), active.clone()]),
            Arc::clone(&unused),
            false
        )
        .await,
        ADMISSION_REJECTED
    );
    assert!(
        unused.checked().is_empty(),
        "identity failures must not consult the ledger"
    );

    let denied = Admission::new(Decision::Deny);
    assert_eq!(
        presence(
            &dialer,
            only(vec![active.clone()]),
            Arc::clone(&denied),
            false
        )
        .await,
        ADMISSION_REJECTED
    );
    assert_eq!(denied.checked(), vec![active.device_id.clone()]);

    let unavailable = Admission::new(Decision::Fail);
    assert_eq!(
        presence(
            &dialer,
            only(vec![active.clone()]),
            Arc::clone(&unavailable),
            false
        )
        .await,
        ADMISSION_REJECTED
    );

    let admitted = Admission::new(Decision::Admit);
    assert_eq!(
        presence(
            &dialer,
            only(vec![active.clone()]),
            Arc::clone(&admitted),
            true
        )
        .await,
        ADMISSION_REJECTED,
        "a stopped runtime rejects even an admitted peer"
    );
    assert_eq!(
        presence(
            &dialer,
            only(vec![active.clone()]),
            Arc::clone(&admitted),
            false
        )
        .await,
        ADMISSION_ACCEPTED
    );

    // 剪贴板接收：同一身份规则，回执保持 Rejected。
    let rejected = clipboard_wire::AckCode::Rejected.as_byte();
    assert_eq!(
        clipboard(
            &dialer,
            only(vec![stranger("private-device-other")]),
            Admission::new(Decision::Admit)
        )
        .await,
        rejected
    );
    assert_eq!(
        clipboard(&dialer, failing(), Admission::new(Decision::Admit)).await,
        rejected
    );
    assert_eq!(
        clipboard(
            &dialer,
            only(vec![duplicate.clone(), active.clone()]),
            Admission::new(Decision::Admit)
        )
        .await,
        rejected
    );
    assert_eq!(
        clipboard(
            &dialer,
            only(vec![active.clone()]),
            Admission::new(Decision::Deny)
        )
        .await,
        rejected
    );
    dialer.close().await;

    let report = handle
        .prepare_local_diagnostic_export(Duration::from_secs(5))
        .expect("export report");
    assert_eq!(report.flush, SignalResult::Completed);
    let rows: Vec<Value> = managed_log_files(logs.path())
        .expect("files")
        .iter()
        .flat_map(|file| {
            std::fs::read_to_string(file)
                .expect("content")
                .lines()
                .map(|line| serde_json::from_str(line).expect("JSON"))
                .collect::<Vec<_>>()
        })
        .collect();
    handle.shutdown(Duration::from_secs(5));

    let rejections: Vec<&Value> = rows
        .iter()
        .filter(|row| row["fields"]["event.name"] == EVENT)
        .collect();
    for row in &rejections {
        assert_eq!(row["level"], "WARN");
        assert_eq!(row["fields"]["direction"], "inbound");
        assert_eq!(row["fields"]["uc.outcome"], "rejected");
    }
    let mut observed: Vec<(String, String, String)> = rejections
        .iter()
        .map(|row| {
            (
                row["fields"]["protocol"].as_str().unwrap_or("").to_owned(),
                row["fields"]["error.phase"]
                    .as_str()
                    .unwrap_or("")
                    .to_owned(),
                row["fields"]["error.reason"]
                    .as_str()
                    .unwrap_or("")
                    .to_owned(),
            )
        })
        .collect();
    observed.sort();
    let mut expected: Vec<(String, String, String)> = [
        ("presence", "identity", "identity_unresolved"),
        ("presence", "identity", "member_read_failed"),
        ("presence", "identity", "identity_ambiguous"),
        ("presence", "admission", "ledger_denied"),
        ("presence", "admission", "ledger_unavailable"),
        ("presence", "admission", "not_accepting"),
        ("clipboard", "identity", "identity_unresolved"),
        ("clipboard", "identity", "member_read_failed"),
        ("clipboard", "identity", "identity_ambiguous"),
        ("clipboard", "admission", "ledger_denied"),
    ]
    .into_iter()
    .map(|(protocol, phase, reason)| (protocol.to_owned(), phase.to_owned(), reason.to_owned()))
    .collect();
    expected.sort();
    assert_eq!(
        observed, expected,
        "exactly one record per rejected connection, none for the admitted one"
    );
    assert!(
        !serde_json::to_string(&rows)
            .expect("rows")
            .contains("private-"),
        "exports must not carry device identifiers, names or error text"
    );
}
