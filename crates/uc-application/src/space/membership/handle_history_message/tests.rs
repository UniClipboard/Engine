use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use uc_core::ids::DeviceId;
use uc_core::membership::{
    AdmissionChangeFacts, HistoricalMembershipSignatureError,
    HistoricalMembershipSignatureVerifier, MembershipActivationBaselineV2, MembershipAdmissionV2,
    MembershipCredential, MembershipEventId, MembershipEventV2, MembershipHistoryAckV3,
    MembershipHistoryMessage, MembershipHistoryRelationship, MembershipOperationV2,
    VersionedMembershipHistory, ED25519_SIGNATURE_ALGORITHM_V1, MEMBERSHIP_EVENT_FORMAT_V2,
};

use super::*;
use crate::space::membership::{
    CommitMembershipLedgerPort, LoadMembershipLedgerPort, LoadedMembershipLedger,
    MembershipEffectKind, MembershipEffectPhase, MembershipLedger, MembershipLedgerError,
    MembershipLedgerMutation, PeerReconciliationRecord,
};

struct MemoryLedgerRepository {
    loaded: Mutex<LoadedMembershipLedger>,
    commits: AtomicUsize,
}

#[async_trait]
impl LoadMembershipLedgerPort for MemoryLedgerRepository {
    async fn load(&self) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        Ok(self.loaded.lock().unwrap().clone())
    }
}

#[async_trait]
impl CommitMembershipLedgerPort for MemoryLedgerRepository {
    async fn compare_and_commit(
        &self,
        mutation: MembershipLedgerMutation,
    ) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        let mut loaded = self.loaded.lock().unwrap();
        let digest = loaded
            .membership_history
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

fn admission(device: &str, credential: MembershipCredential) -> MembershipAdmissionV2 {
    let (facts, _) = member_facts(device, 0x55);
    let mut facts = facts;
    facts.member_instance = credential.member_instance_id(&facts.device_id);
    MembershipAdmissionV2 {
        facts,
        membership_credential: credential,
        resume_public_key_digest: [7; 32],
        security_commitment_id: [8; 32],
    }
}

fn add_event(
    history: &VersionedMembershipHistory,
    author: &MembershipAdmissionV2,
    added: MembershipAdmissionV2,
    marker: u8,
) -> MembershipEventV2 {
    let parent = history.current_head();
    let operation = MembershipOperationV2::AddDevice { admission: added };
    let mut event = MembershipEventV2::new(
        MEMBERSHIP_EVENT_FORMAT_V2,
        history.lineage_id().to_owned(),
        parent,
        parent
            .map(|parent| history.depth(parent).unwrap() + 1)
            .unwrap_or(0),
        [marker; 16],
        author.facts.member_instance,
        author.membership_credential.credential_id,
        author.membership_credential.signature_algorithm_version,
        operation.clone(),
        history
            .expected_resulting_members_digest(parent, &operation)
            .unwrap(),
        [marker.wrapping_add(1); 32],
        vec![marker],
        Some([marker.wrapping_add(2); 32]),
        vec![marker],
    );
    event.signature = vec![marker];
    event
}

fn two_page_extension() -> (
    LoadedMembershipLedger,
    DeviceId,
    Vec<MembershipHistoryMessage>,
) {
    let (local, local_credential) = member_facts("device-a", 0x41);
    let (peer, peer_credential) = member_facts("device-b", 0x42);
    let base = VersionedMembershipHistory::from_activation_baseline(
        MembershipActivationBaselineV2::Established {
            lineage_id: "space-a".to_owned(),
            head_event_id: MembershipEventId::from_hex(&"11".repeat(32)).unwrap(),
            head_depth: 0,
            current_members: vec![
                (local.clone(), local_credential),
                (peer.clone(), peer_credential.clone()),
            ],
        },
    )
    .unwrap();
    let author = MembershipAdmissionV2 {
        facts: peer.clone(),
        membership_credential: peer_credential,
        resume_public_key_digest: [7; 32],
        security_commitment_id: [8; 32],
    };
    let mut incoming = base.clone();
    let add_c = add_event(
        &incoming,
        &author,
        admission(
            "device-large-c",
            MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0x43; 2_100_000]),
        ),
        0x51,
    );
    incoming
        .verify_and_receive_event(add_c, &AcceptingVerifier)
        .unwrap();
    let add_d = add_event(
        &incoming,
        &author,
        admission(
            "device-large-d",
            MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0x44; 2_100_000]),
        ),
        0x52,
    );
    incoming
        .verify_and_receive_event(add_d, &AcceptingVerifier)
        .unwrap();
    let pages = incoming
        .export_suffix_pages_v3(peer.clone(), base.current_position().unwrap())
        .unwrap();
    assert_eq!(pages.len(), 2);
    let peer_device_id = peer.device_id.clone();
    let mut loaded = LoadedMembershipLedger::no_current_space();
    loaded.revision = 10;
    loaded.lineage_id = Some("space-a".to_owned());
    loaded.membership_history = Some(base.encode_persisted_v2().unwrap());
    loaded.local_device_id = Some(local.device_id);
    loaded.local_member_instance = Some(local.member_instance);
    loaded.local_join_active = true;
    loaded.peer_reconciliation.insert(
        peer_device_id.clone(),
        PeerReconciliationRecord {
            peer_device_id: peer_device_id.clone(),
            relationship: MembershipHistoryRelationship::Consistent,
            confirmed_position: None,
            sync_state: Default::default(),
            restricted_delivery: Vec::new(),
            updated_at_ms: 1,
        },
    );
    (
        loaded,
        peer_device_id,
        pages
            .into_iter()
            .map(MembershipHistoryMessage::SuffixPageV3)
            .collect(),
    )
}

#[tokio::test]
async fn restricted_event_applies_only_the_authenticated_signed_event() {
    let (local, local_credential) = member_facts("device-a", 0x41);
    let (peer, peer_credential) = member_facts("device-b", 0x42);
    let base = VersionedMembershipHistory::from_activation_baseline(
        MembershipActivationBaselineV2::Established {
            lineage_id: "space-a".to_owned(),
            head_event_id: MembershipEventId::from_hex(&"11".repeat(32)).unwrap(),
            head_depth: 0,
            current_members: vec![
                (local.clone(), local_credential),
                (peer.clone(), peer_credential.clone()),
            ],
        },
    )
    .unwrap();
    let author = MembershipAdmissionV2 {
        facts: peer.clone(),
        membership_credential: peer_credential,
        resume_public_key_digest: [7; 32],
        security_commitment_id: [8; 32],
    };
    let event = add_event(
        &base,
        &author,
        admission(
            "device-c",
            MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0x43; 32]),
        ),
        0x51,
    );
    let event_id = *event.event_id().as_bytes();
    let mut loaded = LoadedMembershipLedger::no_current_space();
    loaded.revision = 3;
    loaded.lineage_id = Some("space-a".to_owned());
    loaded.membership_history = Some(base.encode_persisted_v2().unwrap());
    loaded.local_device_id = Some(local.device_id);
    loaded.local_member_instance = Some(local.member_instance);
    loaded.local_join_active = true;
    loaded.peer_reconciliation.insert(
        peer.device_id.clone(),
        PeerReconciliationRecord {
            peer_device_id: peer.device_id.clone(),
            relationship: MembershipHistoryRelationship::Consistent,
            confirmed_position: None,
            sync_state: Default::default(),
            restricted_delivery: Vec::new(),
            updated_at_ms: 1,
        },
    );
    let repository = Arc::new(MemoryLedgerRepository {
        loaded: Mutex::new(loaded),
        commits: AtomicUsize::new(0),
    });
    let ledger = Arc::new(MembershipLedger::new(
        repository.clone(),
        repository.clone(),
        Arc::new(AcceptingVerifier),
    ));
    let handler = HandleMembershipHistoryMessageUseCase::new(ledger);

    let response = handler
        .execute(
            &AuthenticatedMember::new(peer.device_id.clone()),
            MembershipHistoryMessage::RestrictedEventV3(event),
        )
        .await
        .unwrap();

    assert_eq!(
        response,
        MembershipHistoryMessage::AckV3(MembershipHistoryAckV3::RestrictedApplied)
    );
    let persisted = repository.load().await.unwrap();
    assert!(persisted.pending_effects.contains_key(&event_id));
    assert_eq!(
        persisted
            .peer_reconciliation
            .get(&peer.device_id)
            .and_then(|record| record.confirmed_position.as_ref()),
        None,
        "受限事件 ACK 不能伪造完整历史确认水位"
    );
}

#[tokio::test]
async fn two_page_transfer_persists_each_page_and_applies_only_when_complete() {
    let (loaded, peer_device_id, pages) = two_page_extension();
    let repository = Arc::new(MemoryLedgerRepository {
        loaded: Mutex::new(loaded),
        commits: AtomicUsize::new(0),
    });
    let ledger = Arc::new(MembershipLedger::new(
        repository.clone(),
        repository.clone(),
        Arc::new(AcceptingVerifier),
    ));
    let handler = HandleMembershipHistoryMessageUseCase::new(ledger);
    let source = AuthenticatedMember::new(peer_device_id.clone());

    let first = handler.execute(&source, pages[0].clone()).await.unwrap();
    let repeated = handler.execute(&source, pages[0].clone()).await.unwrap();
    let after_first = repository.load().await.unwrap();

    assert!(matches!(
        first,
        MembershipHistoryMessage::AckV3(MembershipHistoryAckV3::Continue {
            next_page_index: 1,
            ..
        })
    ));
    assert_eq!(repeated, first);
    assert_eq!(repository.commits.load(Ordering::SeqCst), 1);
    assert_eq!(
        after_first
            .inbound_transfers
            .get(&peer_device_id)
            .unwrap()
            .pages
            .len(),
        1
    );
    let base_history = after_first.membership_history.clone();

    let final_ack = handler.execute(&source, pages[1].clone()).await.unwrap();

    assert!(matches!(
        final_ack,
        MembershipHistoryMessage::AckV3(MembershipHistoryAckV3::Confirmed { .. })
    ));
    assert_eq!(repository.commits.load(Ordering::SeqCst), 2);
    let persisted = repository.load().await.unwrap();
    assert!(persisted.inbound_transfers.is_empty());
    assert_ne!(persisted.membership_history, base_history);
    let prepared_add_devices = persisted
        .pending_effects
        .values()
        .filter(|effect| {
            effect.kind == MembershipEffectKind::AddDevice
                && effect.phase == MembershipEffectPhase::Prepared
        })
        .flat_map(|effect| effect.affected_device_ids.clone())
        .collect::<Vec<_>>();
    assert!(prepared_add_devices.contains(&DeviceId::new("device-large-c")));
    assert!(prepared_add_devices.contains(&DeviceId::new("device-large-d")));
}

#[tokio::test]
async fn out_of_order_page_requests_the_missing_page_without_persisting() {
    let (loaded, peer_device_id, pages) = two_page_extension();
    let repository = Arc::new(MemoryLedgerRepository {
        loaded: Mutex::new(loaded),
        commits: AtomicUsize::new(0),
    });
    let ledger = Arc::new(MembershipLedger::new(
        repository.clone(),
        repository.clone(),
        Arc::new(AcceptingVerifier),
    ));
    let handler = HandleMembershipHistoryMessageUseCase::new(ledger);
    let source = AuthenticatedMember::new(peer_device_id);

    let response = handler.execute(&source, pages[1].clone()).await.unwrap();

    assert!(matches!(
        response,
        MembershipHistoryMessage::AckV3(MembershipHistoryAckV3::Continue {
            next_page_index: 0,
            ..
        })
    ));
    assert_eq!(repository.commits.load(Ordering::SeqCst), 0);
    assert!(repository
        .load()
        .await
        .unwrap()
        .inbound_transfers
        .is_empty());
}

#[tokio::test]
async fn unknown_sender_can_stage_a_bounded_page_but_cannot_commit_unrelated_history() {
    let (mut loaded, peer_device_id, pages) = two_page_extension();
    let (local, local_credential) = member_facts("device-a", 0x41);
    loaded.membership_history = Some(
        VersionedMembershipHistory::new_single_member_root(
            "space-a".to_owned(),
            local.clone(),
            local_credential,
        )
        .unwrap()
        .encode_persisted_v2()
        .unwrap(),
    );
    loaded.local_device_id = Some(local.device_id);
    loaded.local_member_instance = Some(local.member_instance);
    loaded.peer_reconciliation.remove(&peer_device_id);
    let repository = Arc::new(MemoryLedgerRepository {
        loaded: Mutex::new(loaded),
        commits: AtomicUsize::new(0),
    });
    let ledger = Arc::new(MembershipLedger::new(
        repository.clone(),
        repository.clone(),
        Arc::new(AcceptingVerifier),
    ));
    let handler = HandleMembershipHistoryMessageUseCase::new(ledger);

    let source = AuthenticatedMember::new(peer_device_id.clone());
    let first = handler.execute(&source, pages[0].clone()).await.unwrap();

    assert!(matches!(
        first,
        MembershipHistoryMessage::AckV3(MembershipHistoryAckV3::Continue { .. })
    ));
    let final_ack = handler.execute(&source, pages[1].clone()).await.unwrap();
    assert_eq!(
        final_ack,
        MembershipHistoryMessage::AckV3(MembershipHistoryAckV3::Invalid)
    );
    let persisted = repository.load().await.unwrap();
    assert!(persisted.inbound_transfers.is_empty());
    let history = VersionedMembershipHistory::decode_persisted_v2(
        persisted.membership_history.as_deref().unwrap(),
        &AcceptingVerifier,
    )
    .unwrap();
    assert_eq!(history.effective_members().len(), 1);
}
