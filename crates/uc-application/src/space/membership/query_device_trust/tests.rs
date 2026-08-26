use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use uc_core::ids::DeviceId;
use uc_core::membership::{
    AdmissionChangeFacts, AdmissionProfileMetadata, HistoricalMembershipSignatureError,
    HistoricalMembershipSignatureVerifier, JoinerAdmissionStage, MembershipActivationBaselineV2,
    MembershipCredential, MembershipEventId, MembershipHistoryRelationship, MembershipOperationV2,
    SpaceJoinRecord, SpaceJoinRecordId, VersionedMembershipHistory, ED25519_SIGNATURE_ALGORITHM_V1,
};
use uc_core::ports::ReachabilityState;

use super::*;
use crate::space::membership::{
    CommitMembershipLedgerPort, LoadMembershipLedgerPort, LoadedMembershipLedger, MembershipLedger,
    MembershipLedgerError, MembershipLedgerMutation,
};

struct MemoryLedgerRepository {
    loaded: LoadedMembershipLedger,
}

#[async_trait]
impl LoadMembershipLedgerPort for MemoryLedgerRepository {
    async fn load(&self) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        Ok(self.loaded.clone())
    }
}

#[async_trait]
impl CommitMembershipLedgerPort for MemoryLedgerRepository {
    async fn compare_and_commit(
        &self,
        _mutation: MembershipLedgerMutation,
    ) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        Err(MembershipLedgerError::Unavailable)
    }
}

fn member_facts(device: &str, credential_byte: u8) -> (AdmissionChangeFacts, MembershipCredential) {
    let device_id = DeviceId::new(device);
    let credential =
        MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![credential_byte; 32]);
    let member_instance = credential.member_instance_id(&device_id);
    (
        AdmissionChangeFacts {
            member_instance,
            device_id,
            device_name: device.to_owned(),
            identity_fingerprint: uc_core::security::IdentityFingerprint::from_display_string(
                "ABCD-EFGH-IJKL-MNOP",
            )
            .unwrap(),
            transport_public_key: vec![1],
            transport_address_blob: vec![2],
            identity_signature: vec![3],
        },
        credential,
    )
}

fn active_ledger() -> LoadedMembershipLedger {
    let (local_facts, local_credential) = member_facts("device-a", 0x41);
    let (peer_facts, peer_credential) = member_facts("device-b", 0x42);
    let local_member = local_facts.member_instance;
    let peer_device_id = peer_facts.device_id.clone();
    let history = VersionedMembershipHistory::from_activation_baseline(
        MembershipActivationBaselineV2::Established {
            lineage_id: "space-a".to_owned(),
            head_event_id: MembershipEventId::from_hex(&"11".repeat(32)).unwrap(),
            head_depth: 0,
            current_members: vec![
                (local_facts.clone(), local_credential),
                (peer_facts, peer_credential),
            ],
        },
    )
    .unwrap();
    let mut loaded = LoadedMembershipLedger::no_current_space();
    loaded.revision = 8;
    loaded.lineage_id = Some("space-a".to_owned());
    loaded.membership_history_v2 = Some(history.encode_persisted_v2().unwrap());
    loaded.local_device_id = Some(local_facts.device_id);
    loaded.local_member_instance = Some(local_member);
    loaded.local_join_active = true;
    loaded.peer_reconciliation.insert(
        peer_device_id.clone(),
        crate::space::membership::PeerReconciliationRecord {
            peer_device_id,
            relationship: MembershipHistoryRelationship::Consistent,
            confirmed_position: None,
            restricted_delivery: Vec::new(),
            updated_at_ms: 1,
        },
    );
    loaded
}

fn ledger_with_pending_local_removal() -> LoadedMembershipLedger {
    let mut loaded = active_ledger();
    let mut history = VersionedMembershipHistory::decode_persisted_v2(
        loaded.membership_history_v2.as_deref().unwrap(),
        &AcceptingVerifier,
    )
    .unwrap();
    let local_member = loaded.local_member_instance.unwrap();
    let peer_device_id = DeviceId::new("device-b");
    let peer_member = history
        .effective_member_for_device(&peer_device_id)
        .unwrap();
    let peer_credential = history.credential_for(peer_member).unwrap().clone();
    let mut removal = history
        .create_unsigned_local_removal_event(
            peer_member,
            &peer_credential,
            local_member,
            [0x31; 16],
            [0x32; 32],
        )
        .unwrap();
    removal.signature = vec![0x33];
    let mut incoming = history.clone();
    incoming
        .verify_and_receive_event(removal, &AcceptingVerifier)
        .unwrap();
    history
        .merge_remote_history(&incoming, local_member, &AcceptingVerifier)
        .unwrap();
    loaded.membership_history_v2 = Some(history.encode_persisted_v2().unwrap());
    loaded
        .peer_reconciliation
        .get_mut(&peer_device_id)
        .unwrap()
        .relationship = MembershipHistoryRelationship::PendingRemovalDecision;
    loaded
}

struct AcceptingVerifier;

impl HistoricalMembershipSignatureVerifier for AcceptingVerifier {
    fn verify(
        &self,
        _signature_algorithm_version: u16,
        _public_key: &[u8],
        _payload: &[u8],
        _signature: &[u8],
    ) -> Result<bool, HistoricalMembershipSignatureError> {
        Ok(true)
    }
}

struct UnexpectedObservations;

#[async_trait]
impl LoadDeviceTrustObservationsPort for UnexpectedObservations {
    async fn load(
        &self,
        _device_ids: &[uc_core::ids::DeviceId],
    ) -> Result<Vec<DeviceTrustObservation>, QueryDeviceTrustError> {
        panic!("empty status must not read device observations")
    }
}

struct StaticObservations {
    calls: Arc<Mutex<Vec<Vec<DeviceId>>>>,
}

#[async_trait]
impl LoadDeviceTrustObservationsPort for StaticObservations {
    async fn load(
        &self,
        device_ids: &[DeviceId],
    ) -> Result<Vec<DeviceTrustObservation>, QueryDeviceTrustError> {
        self.calls.lock().unwrap().push(device_ids.to_vec());
        Ok(vec![
            DeviceTrustObservation {
                device_id: DeviceId::new("device-b"),
                display_name: Some("Peer B".to_owned()),
                reachability: ReachabilityState::Online,
            },
            DeviceTrustObservation {
                device_id: DeviceId::new("device-a"),
                display_name: Some("Local A".to_owned()),
                reachability: ReachabilityState::Offline,
            },
        ])
    }
}

#[tokio::test]
async fn profile_without_a_space_returns_an_explicit_empty_status() {
    let repository = Arc::new(MemoryLedgerRepository {
        loaded: LoadedMembershipLedger::no_current_space(),
    });
    let ledger = Arc::new(MembershipLedger::new(
        repository.clone(),
        repository,
        Arc::new(AcceptingVerifier),
    ));
    let query = QueryDeviceTrustUseCase::new(ledger, Arc::new(UnexpectedObservations));

    let status = query.execute().await.unwrap();

    assert_eq!(status.revision, 0);
    assert_eq!(
        status.local_membership,
        DeviceTrustMembership::NoCurrentSpace
    );
    assert!(status.local_device_id.is_none());
    assert!(status.devices.is_empty());
    assert!(status.current_change.is_none());
}

#[tokio::test]
async fn active_status_combines_verified_members_with_one_observation_read() {
    let repository = Arc::new(MemoryLedgerRepository {
        loaded: active_ledger(),
    });
    let ledger = Arc::new(MembershipLedger::new(
        repository.clone(),
        repository,
        Arc::new(AcceptingVerifier),
    ));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let query = QueryDeviceTrustUseCase::new(
        ledger,
        Arc::new(StaticObservations {
            calls: Arc::clone(&calls),
        }),
    );

    let status = query.execute().await.unwrap();

    assert_eq!(status.revision, 8);
    assert_eq!(status.local_device_id, Some(DeviceId::new("device-a")));
    assert_eq!(status.local_membership, DeviceTrustMembership::Active);
    assert_eq!(
        calls.lock().unwrap().as_slice(),
        &[vec![DeviceId::new("device-a"), DeviceId::new("device-b")]]
    );
    assert_eq!(status.devices.len(), 2);
    assert_eq!(status.devices[0].device_id, DeviceId::new("device-a"));
    assert_eq!(status.devices[0].display_name, "Local A");
    assert_eq!(
        status.devices[0].relationship,
        DeviceTrustRelationship::Local
    );
    assert_eq!(status.devices[1].device_id, DeviceId::new("device-b"));
    assert_eq!(status.devices[1].display_name, "Peer B");
    assert_eq!(
        status.devices[1].relationship,
        DeviceTrustRelationship::Consistent
    );
    assert_eq!(status.devices[1].sync_state, DeviceTrustSyncState::Usable);
}

#[tokio::test]
async fn status_exposes_the_current_pending_removal_facts() {
    let repository = Arc::new(MemoryLedgerRepository {
        loaded: ledger_with_pending_local_removal(),
    });
    let ledger = Arc::new(MembershipLedger::new(
        repository.clone(),
        repository,
        Arc::new(AcceptingVerifier),
    ));
    let query = QueryDeviceTrustUseCase::new(
        ledger,
        Arc::new(StaticObservations {
            calls: Arc::new(Mutex::new(Vec::new())),
        }),
    );

    let status = query.execute().await.unwrap();
    let change = status.current_change.unwrap();

    assert_eq!(change.proposed_by_device_id, DeviceId::new("device-b"));
    assert_eq!(change.target_device_ids, vec![DeviceId::new("device-a")]);
    assert!(change.includes_local_device);
    let history = VersionedMembershipHistory::decode_persisted_v2(
        &ledger_with_pending_local_removal()
            .membership_history_v2
            .unwrap(),
        &AcceptingVerifier,
    )
    .unwrap();
    let event = history.event(change.change_id).unwrap();
    assert!(matches!(
        event.operation,
        MembershipOperationV2::RemoveDevice { .. }
    ));
}

#[tokio::test]
async fn status_projects_the_current_pending_local_join_from_the_ledger() {
    let mut loaded = active_ledger();
    let record_id = SpaceJoinRecordId::from_bytes([0xa1; 32]);
    let mut join =
        SpaceJoinRecord::new_joiner(record_id, [0xa2; 16], JoinerAdmissionStage::Initiated);
    join.local_join_ordinal = Some(1);
    join.lineage_id = Some("target-space".to_owned());
    loaded.admission_records.insert(*record_id.as_bytes(), join);
    let mut profile = AdmissionProfileMetadata::fresh([0xa3; 16]);
    profile.next_local_join_ordinal = 1;
    loaded.admission_profile = Some(profile);
    let repository = Arc::new(MemoryLedgerRepository { loaded });
    let ledger = Arc::new(MembershipLedger::new(
        repository.clone(),
        repository,
        Arc::new(AcceptingVerifier),
    ));
    let query = QueryDeviceTrustUseCase::new(
        ledger,
        Arc::new(StaticObservations {
            calls: Arc::new(Mutex::new(Vec::new())),
        }),
    );

    let status = query.execute().await.unwrap();

    assert!(matches!(
        status.current_join,
        Some(crate::space::admission::CurrentJoinStatus::Pending {
            join_id,
            target_space_id: Some(ref target),
            cancel_requested: false,
            ..
        }) if join_id == [0xa2; 16] && target == "target-space"
    ));
}
