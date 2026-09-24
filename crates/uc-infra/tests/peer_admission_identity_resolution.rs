use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use iroh::protocol::Router;
use iroh::{Endpoint, RelayMode};
use uc_core::membership::{
    MemberRepositoryPort, MembershipError, PeerAdmissionError, PeerAdmissionPort, SpaceMember,
};
use uc_core::ports::security::IdentityFingerprintFactoryPort;
use uc_core::ports::{PeerAddressError, PeerAddressRecord, PeerAddressRepositoryPort};
use uc_core::{DeviceId, MemberSyncPreferences};
use uc_infra::network::iroh::{IrohPeerReachabilityAdapter, PEER_REACHABILITY_ALPN};
use uc_infra::security::Sha256IdentityFingerprintFactory;
use uc_infra::SystemClock;
use uc_testkit::{Scenario, ScenarioBudget, ScenarioConfig};

const ADMISSION_REQUEST: u8 = 1;
const ADMISSION_ACCEPTED: u8 = 1;
const ADMISSION_REJECTED: u8 = 2;

struct OrderedMemberRepository {
    members: Vec<SpaceMember>,
    fail_list: bool,
}

#[async_trait]
impl MemberRepositoryPort for OrderedMemberRepository {
    async fn get(&self, device_id: &DeviceId) -> Result<Option<SpaceMember>, MembershipError> {
        Ok(self
            .members
            .iter()
            .find(|member| &member.device_id == device_id)
            .cloned())
    }

    async fn list(&self) -> Result<Vec<SpaceMember>, MembershipError> {
        if self.fail_list {
            Err(MembershipError::Repository("diagnostic failure".into()))
        } else {
            Ok(self.members.clone())
        }
    }

    async fn save(&self, _member: &SpaceMember) -> Result<(), MembershipError> {
        Ok(())
    }

    async fn remove(&self, _device_id: &DeviceId) -> Result<bool, MembershipError> {
        Ok(false)
    }
}

struct RecordingAdmission {
    admitted_device: DeviceId,
    checked: Mutex<Vec<DeviceId>>,
    fail_check: bool,
}

impl RecordingAdmission {
    fn new(admitted_device: DeviceId) -> Self {
        Self {
            admitted_device,
            checked: Mutex::new(Vec::new()),
            fail_check: false,
        }
    }

    fn failing(admitted_device: DeviceId) -> Self {
        Self {
            admitted_device,
            checked: Mutex::new(Vec::new()),
            fail_check: true,
        }
    }

    fn checked(&self) -> Vec<DeviceId> {
        self.checked.lock().unwrap().clone()
    }
}

#[async_trait]
impl PeerAdmissionPort for RecordingAdmission {
    async fn is_admitted(&self, device_id: &DeviceId) -> Result<bool, PeerAdmissionError> {
        self.checked.lock().unwrap().push(device_id.clone());
        if self.fail_check {
            return Err(PeerAdmissionError::Unavailable);
        }
        Ok(device_id == &self.admitted_device)
    }
}

#[derive(Default)]
struct EmptyPeerAddresses;

#[async_trait]
impl PeerAddressRepositoryPort for EmptyPeerAddresses {
    async fn get(
        &self,
        _device_id: &DeviceId,
    ) -> Result<Option<PeerAddressRecord>, PeerAddressError> {
        Ok(None)
    }

    async fn upsert(&self, _record: &PeerAddressRecord) -> Result<(), PeerAddressError> {
        Ok(())
    }

    async fn list(&self) -> Result<Vec<PeerAddressRecord>, PeerAddressError> {
        Ok(Vec::new())
    }

    async fn remove(&self, _device_id: &DeviceId) -> Result<(), PeerAddressError> {
        Ok(())
    }
}

async fn endpoint() -> Arc<Endpoint> {
    Arc::new(
        Endpoint::builder(iroh::endpoint::presets::N0)
            .alpns(vec![PEER_REACHABILITY_ALPN.to_vec()])
            .relay_mode(RelayMode::Disabled)
            .clear_address_lookup()
            .bind()
            .await
            .unwrap(),
    )
}

async fn wait_for_address(endpoint: &Endpoint) {
    for _ in 0..100 {
        if !endpoint.addr().addrs.is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("isolated endpoint did not publish a direct address");
}

fn member_for(endpoint: &Endpoint, device_id: &str) -> SpaceMember {
    let identity_fingerprint = Sha256IdentityFingerprintFactory
        .from_public_key(endpoint.id().as_bytes())
        .unwrap();
    SpaceMember {
        device_id: DeviceId::new(device_id),
        device_name: device_id.to_owned(),
        identity_fingerprint,
        joined_at: Utc::now(),
        sync_preferences: MemberSyncPreferences::default(),
    }
}

async fn admission_result(
    dialer: &Endpoint,
    members: Arc<dyn MemberRepositoryPort>,
    admission: Arc<RecordingAdmission>,
) -> u8 {
    let receiver = endpoint().await;
    wait_for_address(&receiver).await;
    let adapter = IrohPeerReachabilityAdapter::new(
        Arc::clone(&receiver),
        Arc::new(EmptyPeerAddresses),
        members,
        admission,
        Arc::new(Sha256IdentityFingerprintFactory),
        Arc::new(SystemClock),
    );
    let router = Router::builder((*receiver).clone())
        .accept(PEER_REACHABILITY_ALPN, adapter.handler())
        .spawn();
    let connection = dialer
        .connect(receiver.addr(), PEER_REACHABILITY_ALPN)
        .await
        .unwrap();
    let (mut send, mut receive) = connection.open_bi().await.unwrap();
    send.write_all(&[ADMISSION_REQUEST]).await.unwrap();
    send.finish().unwrap();
    let mut response = [0u8; 1];
    receive.read_exact(&mut response).await.unwrap();
    connection.close(0u32.into(), b"diagnostic_complete");
    router.shutdown().await.unwrap();
    receiver.close().await;
    response[0]
}

#[tokio::test]
async fn identity_resolution_distinguishes_mapping_and_read_failures() {
    let scenario = Scenario::start(ScenarioConfig::new(
        "peer-admission-identity-resolution",
        0x0010_2026_0923,
        ScenarioBudget::new(Duration::from_secs(30)),
        "cargo test -p uc-infra --test peer_admission_identity_resolution --locked -- --nocapture",
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-artifacts/peer-admission-identity-resolution"),
    ))
    .unwrap();
    let dialer = endpoint().await;
    wait_for_address(&dialer).await;

    let active = member_for(&dialer, "active-peer");
    let unique_admission = Arc::new(RecordingAdmission::new(active.device_id.clone()));
    {
        let _stage = scenario.stage("unique-current-identity");
        let result = admission_result(
            &dialer,
            Arc::new(OrderedMemberRepository {
                members: vec![active.clone()],
                fail_list: false,
            }),
            Arc::clone(&unique_admission),
        )
        .await;
        assert_eq!(result, ADMISSION_ACCEPTED);
        assert_eq!(
            unique_admission.checked(),
            vec![active.device_id.clone(), active.device_id.clone()]
        );
        scenario.record_event("unique-current-identity-admitted");
    }

    let mut stale = active.clone();
    stale.device_id = DeviceId::new("stale-peer");
    let duplicate_admission = Arc::new(RecordingAdmission::new(active.device_id.clone()));
    {
        let _stage = scenario.stage("duplicate-identity-stale-first");
        let result = admission_result(
            &dialer,
            Arc::new(OrderedMemberRepository {
                members: vec![stale.clone(), active.clone()],
                fail_list: false,
            }),
            Arc::clone(&duplicate_admission),
        )
        .await;
        assert_eq!(result, ADMISSION_REJECTED);
        assert!(duplicate_admission.checked().is_empty());
        scenario.record_event("duplicate-identity-rejected-before-admission");
    }

    let failed_read_admission = Arc::new(RecordingAdmission::new(active.device_id.clone()));
    {
        let _stage = scenario.stage("member-projection-read-failure");
        let result = admission_result(
            &dialer,
            Arc::new(OrderedMemberRepository {
                members: Vec::new(),
                fail_list: true,
            }),
            Arc::clone(&failed_read_admission),
        )
        .await;
        assert_eq!(result, ADMISSION_REJECTED);
        assert!(failed_read_admission.checked().is_empty());
        scenario.record_event("member-projection-read-failed-before-admission");
    }

    let failed_ledger_admission = Arc::new(RecordingAdmission::failing(active.device_id.clone()));
    {
        let _stage = scenario.stage("admission-ledger-read-failure");
        let result = admission_result(
            &dialer,
            Arc::new(OrderedMemberRepository {
                members: vec![active.clone()],
                fail_list: false,
            }),
            Arc::clone(&failed_ledger_admission),
        )
        .await;
        assert_eq!(result, ADMISSION_REJECTED);
        assert_eq!(failed_ledger_admission.checked(), vec![active.device_id]);
        scenario.record_event("admission-ledger-read-failed-after-identity-resolution");
    }

    dialer.close().await;
    scenario.finish(Ok(())).unwrap();
}
