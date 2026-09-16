//! 真实协议拒收必须在普通采集的实际文件中保留原因。
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use iroh::{endpoint::presets, protocol::Router, Endpoint, RelayMode};
use uc_application::deps::ClipboardReceiverPort;
use uc_core::ids::DeviceId;
use uc_core::membership::{
    MemberRepositoryPort, MemberSyncPreferences, MembershipError, PeerAdmissionError,
    PeerAdmissionPort, SpaceMember,
};
use uc_core::ports::security::IdentityFingerprintFactoryPort;
use uc_core::ports::ClipboardHeader;
use uc_infra::network::iroh::{clipboard_wire, IrohClipboardReceiverAdapter, CLIPBOARD_ALPN};
use uc_infra::security::Sha256IdentityFingerprintFactory;
use uc_observability_runtime::*;

struct Member(SpaceMember);
#[async_trait]
impl MemberRepositoryPort for Member {
    async fn get(&self, _: &DeviceId) -> Result<Option<SpaceMember>, MembershipError> {
        Ok(Some(self.0.clone()))
    }
    async fn list(&self) -> Result<Vec<SpaceMember>, MembershipError> {
        Ok(vec![self.0.clone()])
    }
    async fn save(&self, _: &SpaceMember) -> Result<(), MembershipError> {
        panic!("只读测试")
    }
    async fn remove(&self, _: &DeviceId) -> Result<bool, MembershipError> {
        panic!("只读测试")
    }
}
struct Admitted;
#[async_trait]
impl PeerAdmissionPort for Admitted {
    async fn is_admitted(&self, _: &DeviceId) -> Result<bool, PeerAdmissionError> {
        Ok(true)
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

#[tokio::test]
async fn receipt_failures_remain_distinct_in_standard_and_detailed_exports() {
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
    let sender = endpoint().await;
    let receiver = endpoint().await;
    let members = Arc::new(Member(SpaceMember {
        device_id: DeviceId::new("private-device-sentinel"),
        device_name: "private-name-sentinel".into(),
        identity_fingerprint: Sha256IdentityFingerprintFactory
            .from_public_key(sender.id().as_bytes())
            .expect("fingerprint"),
        joined_at: chrono::Utc::now(),
        sync_preferences: MemberSyncPreferences::default(),
    }));
    let adapter = IrohClipboardReceiverAdapter::new(
        receiver.clone(),
        members,
        Arc::new(Admitted),
        Arc::new(Sha256IdentityFingerprintFactory),
    );
    let router = Router::builder((*receiver).clone())
        .accept(CLIPBOARD_ALPN, adapter.handler())
        .spawn();
    for detailed in [false, true] {
        if detailed {
            handle
                .start_local_diagnostic_capture(DetailedCaptureRequest::default())
                .expect("detailed capture");
        }
        for settlement in [0, 1, 2] {
            let mut subscription = (settlement != 0).then(|| adapter.subscribe());
            let connection = sender
                .connect(receiver.addr(), CLIPBOARD_ALPN)
                .await
                .expect("connect");
            let (mut send, mut recv) = connection.open_bi().await.expect("stream");
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
            let held = if let Some(subscription) = subscription.as_mut() {
                let delivery = subscription.recv().await.expect("delivery");
                if settlement == 2 {
                    Some(delivery)
                } else {
                    None
                }
            } else {
                None
            };
            if held.is_some() {
                tokio::time::pause();
                tokio::time::advance(Duration::from_secs(61)).await;
                tokio::time::resume();
            }
            let mut ack = [0];
            tokio::time::timeout(Duration::from_secs(3), recv.read_exact(&mut ack))
                .await
                .expect("bounded reply")
                .expect("reply");
            assert_eq!(ack[0], clipboard_wire::AckCode::Rejected.as_byte());
            connection.close(0_u32.into(), b"");
        }
    }
    router.shutdown().await.expect("router");
    sender.close().await;
    let report = handle
        .prepare_local_diagnostic_export(Duration::from_secs(5))
        .expect("export report");
    assert_eq!(report.flush, SignalResult::Completed);
    let rows: Vec<serde_json::Value> = managed_log_files(logs.path())
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
    let failures: Vec<_> = rows
        .iter()
        .filter(|row| row["fields"]["uc.operation"] == "clipboard_receive")
        .collect();
    assert_eq!(failures.len(), 6);
    assert_eq!(failures[0]["fields"]["uc.outcome"], "error");
    assert_eq!(failures[0]["fields"]["error.phase"], "handoff");
    assert_eq!(failures[0]["fields"]["error.reason"], "no_consumer");
    assert_eq!(failures[1]["fields"]["error.phase"], "settlement");
    assert_eq!(failures[1]["fields"]["error.reason"], "receipt_dropped");
    assert_eq!(failures[2]["fields"]["error.reason"], "application_timeout");
    for index in 0..3 {
        assert_eq!(failures[index]["capture_mode"], "standard");
        assert_eq!(failures[index + 3]["capture_mode"], "detailed");
        assert_eq!(
            failures[index]["fields"]["error.reason"],
            failures[index + 3]["fields"]["error.reason"]
        );
    }
    assert!(!serde_json::to_string(&rows)
        .expect("rows")
        .contains("private-"));
}
