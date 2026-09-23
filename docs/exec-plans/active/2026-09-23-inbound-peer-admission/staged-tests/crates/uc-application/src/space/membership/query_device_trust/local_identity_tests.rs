//! 本机当前身份与成员历史中本机身份不一致时，统一状态进入“需要处理”，并只在状态变化时留下诊断
//! （切片 S5a 暂存验收）。S5a 不给恢复动作，恢复方式由 S5b 在现场验证后决定。
//!
//! 在 `query_device_trust/mod.rs` 中以 `#[cfg(test)] mod local_identity_tests;` 声明。
use std::fmt::Debug;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::Value;
use tracing::{
    field::{Field, Visit},
    Event, Subscriber,
};
use tracing_subscriber::{layer::Context, prelude::*, Layer, Registry};
use uc_core::ids::DeviceId;
use uc_core::membership::{
    AdmissionChangeFacts, HistoricalMembershipSignatureError,
    HistoricalMembershipSignatureVerifier, MembershipActivationBaselineV2, MembershipCredential,
    MembershipEventId, MembershipHistoryRelationship, VersionedMembershipHistory,
    ED25519_SIGNATURE_ALGORITHM_V1,
};
use uc_core::ports::{LocalIdentityError, LocalIdentityPort, ReachabilityState};
use uc_core::security::IdentityFingerprint;
use uc_observability_contract::diagnostics::connectivity::decode_local_record;

use super::*;
use crate::space::membership::{
    CommitMembershipLedgerPort, LoadMembershipLedgerPort, LoadedMembershipLedger, MembershipLedger,
    MembershipLedgerError, MembershipLedgerMutation, PeerReconciliationRecord,
};

const RECORDED: &str = "ABCD-EFGH-IJKL-MNOP";
const ROTATED: &str = "QRST-UVWX-YZ23-4567";
const EVENT: &str = "space.local_identity.changed";

struct AcceptingVerifier;

impl HistoricalMembershipSignatureVerifier for AcceptingVerifier {
    fn verify(
        &self,
        _: u16,
        _: &[u8],
        _: &[u8],
        _: &[u8],
    ) -> Result<bool, HistoricalMembershipSignatureError> {
        Ok(true)
    }
}

struct Repository(LoadedMembershipLedger);

#[async_trait]
impl LoadMembershipLedgerPort for Repository {
    async fn load(&self) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        Ok(self.0.clone())
    }
}

#[async_trait]
impl CommitMembershipLedgerPort for Repository {
    async fn compare_and_commit(
        &self,
        _: MembershipLedgerMutation,
    ) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        panic!("query is read-only")
    }
}

struct Offline;

#[async_trait]
impl LoadDeviceTrustObservationsPort for Offline {
    async fn load(
        &self,
        device_ids: &[DeviceId],
    ) -> Result<Vec<DeviceTrustObservation>, QueryDeviceTrustError> {
        Ok(device_ids
            .iter()
            .map(|device_id| DeviceTrustObservation {
                device_id: device_id.clone(),
                display_name: None,
                reachability: ReachabilityState::Offline,
            })
            .collect())
    }
}

struct SecurityUpdates(SpaceDeviceUpdateStatus);

#[async_trait]
impl LoadSecurityDeviceUpdateStatusPort for SecurityUpdates {
    async fn load_security_device_update_status(
        &self,
    ) -> Result<SpaceDeviceUpdateStatus, QueryDeviceTrustError> {
        Ok(self.0)
    }
}

#[derive(Clone, Copy)]
enum Identity {
    Current(&'static str),
    Missing,
    Broken,
}

/// 可在两次查询之间替换的本机身份。
struct LocalIdentity(Mutex<Identity>);

impl LocalIdentity {
    fn new(identity: Identity) -> Arc<Self> {
        Arc::new(Self(Mutex::new(identity)))
    }
    fn set(&self, identity: Identity) {
        *self.0.lock().unwrap() = identity;
    }
}

#[async_trait]
impl LocalIdentityPort for LocalIdentity {
    async fn create(&self) -> Result<IdentityFingerprint, LocalIdentityError> {
        panic!("the query must never create an identity")
    }
    async fn ensure(&self) -> Result<IdentityFingerprint, LocalIdentityError> {
        panic!("the query must never generate an identity")
    }
    async fn get_current_fingerprint(
        &self,
    ) -> Result<Option<IdentityFingerprint>, LocalIdentityError> {
        match *self.0.lock().unwrap() {
            Identity::Current(value) => Ok(Some(
                IdentityFingerprint::from_display_string(value).unwrap(),
            )),
            Identity::Missing => Ok(None),
            Identity::Broken => Err(LocalIdentityError::Storage("fixture".to_owned())),
        }
    }
}

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<Value>>>);

#[derive(Default)]
struct Fields {
    name: String,
    payload: String,
}

impl Visit for Fields {
    fn record_debug(&mut self, _: &Field, _: &dyn Debug) {}
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "event.name" => self.name = value.into(),
            "payload" => self.payload = value.into(),
            _ => {}
        }
    }
}

impl<S: Subscriber> Layer<S> for Capture {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        if event.metadata().target() != "uc.connectivity" {
            return;
        }
        let mut fields = Fields::default();
        event.record(&mut fields);
        if fields.name != EVENT {
            return;
        }
        let record = decode_local_record(
            &fields.name,
            &fields.payload,
            event.metadata().level().as_str(),
        )
        .expect("the closed record must decode");
        self.0.lock().unwrap().push(Value::Object(record));
    }
}

impl Capture {
    fn states(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|record| record["state"].as_str().unwrap_or("").to_owned())
            .collect()
    }
}

fn facts(device: &str, byte: u8) -> (AdmissionChangeFacts, MembershipCredential) {
    let device_id = DeviceId::new(device);
    let credential = MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![byte; 32]);
    (
        AdmissionChangeFacts {
            member_instance: credential.member_instance_id(&device_id),
            device_id,
            device_name: device.to_owned(),
            identity_fingerprint: IdentityFingerprint::from_display_string(RECORDED).unwrap(),
            transport_public_key: vec![1],
            transport_address_blob: vec![2],
            identity_signature: vec![3],
        },
        credential,
    )
}

fn ledger(local_join_active: bool) -> LoadedMembershipLedger {
    let (local, local_credential) = facts("device-a", 0x41);
    let (peer, peer_credential) = facts("device-b", 0x42);
    let history = VersionedMembershipHistory::from_activation_baseline(
        MembershipActivationBaselineV2::Established {
            lineage_id: "space-a".to_owned(),
            head_event_id: MembershipEventId::from_hex(&"11".repeat(32)).unwrap(),
            head_depth: 0,
            current_members: vec![(local.clone(), local_credential), (peer, peer_credential)],
        },
    )
    .unwrap();
    let mut loaded = LoadedMembershipLedger::no_current_space();
    loaded.revision = 3;
    loaded.lineage_id = Some("space-a".to_owned());
    loaded.membership_history = Some(history.encode_persisted_v2().unwrap());
    loaded.local_device_id = Some(local.device_id);
    loaded.local_member_instance = Some(local.member_instance);
    loaded.local_join_active = local_join_active;
    loaded.peer_reconciliation.insert(
        DeviceId::new("device-b"),
        PeerReconciliationRecord {
            peer_device_id: DeviceId::new("device-b"),
            relationship: MembershipHistoryRelationship::Consistent,
            confirmed_position: None,
            sync_state: Default::default(),
            restricted_delivery: Vec::new(),
            updated_at_ms: 1,
        },
    );
    loaded
}

fn query(
    loaded: LoadedMembershipLedger,
    security: SpaceDeviceUpdateStatus,
    identity: Arc<LocalIdentity>,
) -> QueryDeviceTrustUseCase {
    let repository = Arc::new(Repository(loaded));
    QueryDeviceTrustUseCase::new(
        Arc::new(MembershipLedger::new(
            Arc::clone(&repository) as Arc<dyn LoadMembershipLedgerPort>,
            repository,
            Arc::new(AcceptingVerifier),
        )),
        Arc::new(Offline),
        Arc::new(NoCurrentJoinStatus),
        Arc::new(SecurityUpdates(security)),
        identity,
    )
}

fn mismatch() -> SpaceDeviceUpdateStatus {
    SpaceDeviceUpdateStatus::needs_attention_without_recovery(
        SpaceDeviceUpdateProblem::LocalIdentityMismatch,
    )
}

#[tokio::test]
async fn a_replaced_local_identity_needs_attention_instead_of_retrying() {
    let status = query(
        ledger(true),
        SpaceDeviceUpdateStatus::retryable_failure(1_000),
        LocalIdentity::new(Identity::Current(ROTATED)),
    )
    .execute()
    .await
    .unwrap();
    assert_eq!(status.space_device_update, mismatch());
    assert_eq!(
        status.space_device_update.phase,
        SpaceDeviceUpdatePhase::NeedsAttention
    );
    assert_eq!(
        status.space_device_update.recovery, None,
        "S5a must not prescribe a recovery action"
    );
    assert_eq!(status.space_device_update.next_retry_at_ms, None);
}

#[tokio::test]
async fn a_matching_local_identity_keeps_the_existing_status() {
    let status = query(
        ledger(true),
        SpaceDeviceUpdateStatus::completed(),
        LocalIdentity::new(Identity::Current(RECORDED)),
    )
    .execute()
    .await
    .unwrap();
    assert_ne!(status.space_device_update, mismatch());
    assert_eq!(
        status.space_device_update.phase,
        SpaceDeviceUpdatePhase::Updating,
        "the unconfirmed peer still keeps the space updating"
    );
}

#[tokio::test]
async fn no_local_identity_yet_is_not_reported_as_a_mismatch() {
    let status = query(
        ledger(true),
        SpaceDeviceUpdateStatus::completed(),
        LocalIdentity::new(Identity::Missing),
    )
    .execute()
    .await
    .unwrap();
    assert_ne!(status.space_device_update, mismatch());
}

#[tokio::test]
async fn an_inactive_local_member_is_not_checked() {
    let status = query(
        ledger(false),
        SpaceDeviceUpdateStatus::completed(),
        LocalIdentity::new(Identity::Current(ROTATED)),
    )
    .execute()
    .await
    .unwrap();
    assert_ne!(status.space_device_update, mismatch());
}

#[tokio::test]
async fn an_unreadable_local_identity_keeps_its_source() {
    let error = query(
        ledger(true),
        SpaceDeviceUpdateStatus::completed(),
        LocalIdentity::new(Identity::Broken),
    )
    .execute()
    .await
    .unwrap_err();
    match error {
        QueryDeviceTrustError::Dependency { source } => {
            assert!(source.downcast_ref::<LocalIdentityError>().is_some());
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn the_diagnostic_is_recorded_only_when_the_state_changes() {
    let capture = Capture::default();
    let _guard = tracing::subscriber::set_default(Registry::default().with(capture.clone()));
    let identity = LocalIdentity::new(Identity::Current(RECORDED));
    let query = query(
        ledger(true),
        SpaceDeviceUpdateStatus::completed(),
        Arc::clone(&identity),
    );

    query.execute().await.unwrap();
    assert!(
        capture.states().is_empty(),
        "a consistent identity from the start is not a change"
    );

    identity.set(Identity::Current(ROTATED));
    query.execute().await.unwrap();
    query.execute().await.unwrap();
    query.execute().await.unwrap();
    assert_eq!(
        capture.states(),
        ["mismatch"],
        "repeated queries must not repeat the record"
    );

    identity.set(Identity::Current(RECORDED));
    query.execute().await.unwrap();
    query.execute().await.unwrap();
    assert_eq!(capture.states(), ["mismatch", "consistent"]);

    let exported = serde_json::to_string(&*capture.0.lock().unwrap()).unwrap();
    assert!(!exported.contains("ABCD") && !exported.contains("QRST"));
}
