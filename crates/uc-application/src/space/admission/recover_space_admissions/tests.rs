use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use uc_core::ids::DeviceId;
use uc_core::membership::{
    AdmissionChangeFacts, AdmissionOutboxMessage, AdmissionOutboxPurpose, AdmissionProfileMetadata,
    HistoricalMembershipSignatureError, HistoricalMembershipSignatureVerifier,
    JoinerAdmissionStage, MembershipCredential, SpaceJoinRecord, SpaceJoinRecordId,
    VersionedMembershipHistory, ED25519_SIGNATURE_ALGORITHM_V1,
};

use super::*;
use crate::deps::{
    AdmissionOutboxDeliveryError, AdmissionOutboxDeliveryPort, AdmissionOutboxDeliveryResult,
    AdmissionOutboxDeliveryRoute,
};
use crate::space::membership_ledger::{
    CommitMembershipLedgerPort, LoadMembershipLedgerPort, LoadedMembershipLedger, MembershipLedger,
    MembershipLedgerError, MembershipLedgerMutation,
};

struct MemoryRepository {
    loaded: Mutex<LoadedMembershipLedger>,
    commits: AtomicUsize,
}

#[async_trait]
impl LoadMembershipLedgerPort for MemoryRepository {
    async fn load(&self) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        Ok(self.loaded.lock().unwrap().clone())
    }
}

#[async_trait]
impl CommitMembershipLedgerPort for MemoryRepository {
    async fn compare_and_commit(
        &self,
        mutation: MembershipLedgerMutation,
    ) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        let mut loaded = self.loaded.lock().unwrap();
        let digest = loaded
            .membership_history_v2
            .as_deref()
            .map(|bytes| <[u8; 32]>::from(Sha256::digest(bytes)));
        if loaded.revision != mutation.expected_revision
            || digest != mutation.expected_history_digest
        {
            return Err(MembershipLedgerError::Conflict);
        }
        self.commits.fetch_add(1, Ordering::SeqCst);
        *loaded = mutation.replacement;
        Ok(loaded.clone())
    }
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

struct PersistingDelivery;

#[async_trait]
impl AdmissionOutboxDeliveryPort for PersistingDelivery {
    async fn deliver(
        &self,
        _attempt_id: SpaceJoinRecordId,
        message: &AdmissionOutboxMessage,
        _route: Option<&AdmissionOutboxDeliveryRoute>,
    ) -> Result<AdmissionOutboxDeliveryResult, AdmissionOutboxDeliveryError> {
        Ok(AdmissionOutboxDeliveryResult::Persisted(
            crate::space::admission::outbox::acknowledgment(message),
        ))
    }
}

struct RejectingDelivery;

#[async_trait]
impl AdmissionOutboxDeliveryPort for RejectingDelivery {
    async fn deliver(
        &self,
        attempt_id: SpaceJoinRecordId,
        message: &AdmissionOutboxMessage,
        _route: Option<&AdmissionOutboxDeliveryRoute>,
    ) -> Result<AdmissionOutboxDeliveryResult, AdmissionOutboxDeliveryError> {
        Ok(AdmissionOutboxDeliveryResult::Rejected(
            crate::space::admission::outbox::message(
                attempt_id,
                AdmissionOutboxPurpose::Rejected,
                &message.recipient,
                Some(message.message_id),
                b"rejected",
            ),
        ))
    }
}

fn loaded_with_outbox() -> (LoadedMembershipLedger, SpaceJoinRecordId) {
    let device_id = DeviceId::new("device-a");
    let credential = MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0x41; 32]);
    let member_instance = credential.member_instance_id(&device_id);
    let history = VersionedMembershipHistory::new_single_member_root(
        "space-a".to_owned(),
        AdmissionChangeFacts {
            member_instance,
            device_id: device_id.clone(),
            device_name: "Device A".to_owned(),
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
    .unwrap();
    let record_id = SpaceJoinRecordId::from_bytes([0x51; 32]);
    let mut record =
        SpaceJoinRecord::new_joiner(record_id, [0x52; 16], JoinerAdmissionStage::Initiated);
    record.outboxes.push(AdmissionOutboxMessage {
        purpose: AdmissionOutboxPurpose::JoinRequest,
        recipient: vec![0x53],
        message_id: [0x54; 32],
        predecessor_message_id: None,
        payload: vec![0x55],
        superseded: false,
    });
    let mut loaded = LoadedMembershipLedger::no_current_space();
    loaded.revision = 7;
    loaded.lineage_id = Some("space-a".to_owned());
    loaded.membership_history_v2 = Some(history.encode_persisted_v2().unwrap());
    loaded.local_device_id = Some(device_id);
    loaded.local_member_instance = Some(member_instance);
    loaded.local_join_active = true;
    let mut metadata = AdmissionProfileMetadata::fresh([0x56; 16]);
    metadata.device_trust_revision = 7;
    loaded.admission_profile = Some(metadata);
    loaded
        .admission_records
        .insert(*record_id.as_bytes(), record);
    (loaded, record_id)
}

#[tokio::test]
async fn persisted_outbox_ack_is_settled_in_the_same_ledger() {
    let (loaded, record_id) = loaded_with_outbox();
    let repository = Arc::new(MemoryRepository {
        loaded: Mutex::new(loaded),
        commits: AtomicUsize::new(0),
    });
    let ledger = Arc::new(MembershipLedger::new(
        repository.clone(),
        repository.clone(),
        Arc::new(AcceptingVerifier),
    ));
    let recover = RecoverSpaceAdmissionsUseCase::new(ledger, Arc::new(PersistingDelivery));

    let report = recover.execute().await;

    assert_eq!(report.completed_count, 1);
    assert_eq!(report.deferred_count, 0);
    assert_eq!(repository.commits.load(Ordering::SeqCst), 1);
    let persisted = repository.load().await.unwrap();
    let record = persisted
        .admission_records
        .get(record_id.as_bytes())
        .unwrap();
    assert_eq!(record.record_version, 1);
    assert!(record.outboxes[0].superseded);
    assert_eq!(persisted.revision, 8);
}

#[tokio::test]
async fn every_persisted_outbox_ack_uses_the_latest_record_version() {
    let (mut loaded, record_id) = loaded_with_outbox();
    loaded
        .admission_records
        .get_mut(record_id.as_bytes())
        .unwrap()
        .outboxes
        .push(AdmissionOutboxMessage {
            purpose: AdmissionOutboxPurpose::Prepared,
            recipient: vec![0x61],
            message_id: [0x62; 32],
            predecessor_message_id: Some([0x54; 32]),
            payload: vec![0x63],
            superseded: false,
        });
    let repository = Arc::new(MemoryRepository {
        loaded: Mutex::new(loaded),
        commits: AtomicUsize::new(0),
    });
    let ledger = Arc::new(MembershipLedger::new(
        repository.clone(),
        repository.clone(),
        Arc::new(AcceptingVerifier),
    ));
    let recover = RecoverSpaceAdmissionsUseCase::new(ledger, Arc::new(PersistingDelivery));

    let report = recover.execute().await;

    assert_eq!(report.completed_count, 2);
    assert_eq!(report.deferred_count, 0);
    assert_eq!(repository.commits.load(Ordering::SeqCst), 2);
    let persisted = repository.load().await.unwrap();
    let record = persisted
        .admission_records
        .get(record_id.as_bytes())
        .unwrap();
    assert_eq!(record.record_version, 2);
    assert!(record.outboxes.iter().all(|message| message.superseded));
    assert_eq!(persisted.revision, 9);
}

#[tokio::test]
async fn stable_rejection_settles_the_referenced_outbox_message() {
    let (loaded, record_id) = loaded_with_outbox();
    let repository = Arc::new(MemoryRepository {
        loaded: Mutex::new(loaded),
        commits: AtomicUsize::new(0),
    });
    let ledger = Arc::new(MembershipLedger::new(
        repository.clone(),
        repository.clone(),
        Arc::new(AcceptingVerifier),
    ));
    let recover = RecoverSpaceAdmissionsUseCase::new(ledger, Arc::new(RejectingDelivery));

    let report = recover.execute().await;

    assert_eq!(report.stable_failure_count, 1);
    assert_eq!(report.deferred_count, 0);
    assert_eq!(repository.commits.load(Ordering::SeqCst), 1);
    assert!(
        repository
            .load()
            .await
            .unwrap()
            .admission_records
            .get(record_id.as_bytes())
            .unwrap()
            .outboxes[0]
            .superseded
    );
}
