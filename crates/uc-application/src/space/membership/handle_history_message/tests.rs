use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use uc_core::ids::DeviceId;
use uc_core::membership::{
    AdmissionChangeFacts, HistoricalMembershipSignatureError,
    HistoricalMembershipSignatureVerifier, MembershipActivationBaselineV2, MembershipAdmissionV2,
    MembershipCredential, MembershipEventId, MembershipEventV2, MembershipHistoryMessage,
    MembershipHistoryRelationship, MembershipHistoryV2Ack, MembershipOperationV2,
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

fn consistent_transfer() -> (
    LoadedMembershipLedger,
    DeviceId,
    MembershipHistoryMessage,
    [u8; 32],
) {
    let (local, local_credential) = member_facts("device-a", 0x41);
    let (peer, peer_credential) = member_facts("device-b", 0x42);
    let history = VersionedMembershipHistory::from_activation_baseline(
        MembershipActivationBaselineV2::Established {
            lineage_id: "space-a".to_owned(),
            head_event_id: MembershipEventId::from_hex(&"11".repeat(32)).unwrap(),
            head_depth: 0,
            current_members: vec![
                (local.clone(), local_credential),
                (peer.clone(), peer_credential),
            ],
        },
    )
    .unwrap();
    let page = history
        .export_reconciliation_pages_v2(peer.clone())
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let transfer_id = page.transfer_id();
    let peer_device_id = peer.device_id.clone();
    let mut loaded = LoadedMembershipLedger::no_current_space();
    loaded.revision = 5;
    loaded.lineage_id = Some("space-a".to_owned());
    loaded.membership_history_v2 = Some(history.encode_persisted_v2().unwrap());
    loaded.local_device_id = Some(local.device_id);
    loaded.local_member_instance = Some(local.member_instance);
    loaded.local_join_active = true;
    loaded.peer_reconciliation.insert(
        peer_device_id.clone(),
        PeerReconciliationRecord {
            peer_device_id: peer_device_id.clone(),
            relationship: MembershipHistoryRelationship::Unknown,
            confirmed_position: None,
            restricted_delivery: Vec::new(),
            updated_at_ms: 1,
        },
    );
    (
        loaded,
        peer_device_id,
        MembershipHistoryMessage::HistoryPageV2(page),
        transfer_id,
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
        .export_reconciliation_pages_v2(peer.clone())
        .unwrap();
    assert_eq!(pages.len(), 2);
    let peer_device_id = peer.device_id.clone();
    let mut loaded = LoadedMembershipLedger::no_current_space();
    loaded.revision = 10;
    loaded.lineage_id = Some("space-a".to_owned());
    loaded.membership_history_v2 = Some(base.encode_persisted_v2().unwrap());
    loaded.local_device_id = Some(local.device_id);
    loaded.local_member_instance = Some(local.member_instance);
    loaded.local_join_active = true;
    loaded.peer_reconciliation.insert(
        peer_device_id.clone(),
        PeerReconciliationRecord {
            peer_device_id: peer_device_id.clone(),
            relationship: MembershipHistoryRelationship::Consistent,
            confirmed_position: None,
            restricted_delivery: Vec::new(),
            updated_at_ms: 1,
        },
    );
    (
        loaded,
        peer_device_id,
        pages
            .into_iter()
            .map(MembershipHistoryMessage::HistoryPageV2)
            .collect(),
    )
}

#[tokio::test]
async fn complete_page_is_persisted_before_ack_and_replays_idempotently() {
    let (loaded, peer_device_id, message, transfer_id) = consistent_transfer();
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

    let first = handler.execute(&source, message.clone()).await.unwrap();
    let replay = handler.execute(&source, message).await.unwrap();

    assert_eq!(
        first,
        MembershipHistoryMessage::AckV2(MembershipHistoryV2Ack::Consistent)
    );
    assert_eq!(replay, first);
    assert_eq!(repository.commits.load(Ordering::SeqCst), 1);
    let persisted = repository.load().await.unwrap();
    assert!(persisted.inbound_transfers.is_empty());
    assert_eq!(
        persisted
            .completed_inbound_transfers
            .get(&(peer_device_id.clone(), transfer_id)),
        Some(&MembershipHistoryV2Ack::Consistent)
    );
    assert_eq!(
        persisted
            .peer_reconciliation
            .get(&peer_device_id)
            .unwrap()
            .relationship,
        MembershipHistoryRelationship::Consistent
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
        MembershipHistoryMessage::AckV2(MembershipHistoryV2Ack::Continue {
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
    let base_history = after_first.membership_history_v2.clone();

    let final_ack = handler.execute(&source, pages[1].clone()).await.unwrap();

    assert_eq!(
        final_ack,
        MembershipHistoryMessage::AckV2(MembershipHistoryV2Ack::UpdatesApplied)
    );
    assert_eq!(repository.commits.load(Ordering::SeqCst), 2);
    let persisted = repository.load().await.unwrap();
    assert!(persisted.inbound_transfers.is_empty());
    assert_ne!(persisted.membership_history_v2, base_history);
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
        MembershipHistoryMessage::AckV2(MembershipHistoryV2Ack::Continue {
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
async fn replacing_an_active_transfer_is_persistently_invalid() {
    let (loaded, peer_device_id, pages) = two_page_extension();
    let (_, _, replacement, replacement_transfer_id) = consistent_transfer();
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
    handler.execute(&source, pages[0].clone()).await.unwrap();

    let invalid = handler.execute(&source, replacement.clone()).await.unwrap();
    let replay = handler.execute(&source, replacement).await.unwrap();

    assert_eq!(
        invalid,
        MembershipHistoryMessage::AckV2(MembershipHistoryV2Ack::Invalid)
    );
    assert_eq!(replay, invalid);
    assert_eq!(repository.commits.load(Ordering::SeqCst), 2);
    let persisted = repository.load().await.unwrap();
    assert!(persisted.inbound_transfers.is_empty());
    assert_eq!(
        persisted
            .completed_inbound_transfers
            .get(&(peer_device_id.clone(), replacement_transfer_id)),
        Some(&MembershipHistoryV2Ack::Invalid)
    );
    assert_eq!(
        persisted
            .peer_reconciliation
            .get(&peer_device_id)
            .unwrap()
            .relationship,
        MembershipHistoryRelationship::Invalid
    );
}

#[tokio::test]
async fn authenticated_removed_device_is_rejected_before_a_page_is_saved() {
    let (mut loaded, peer_device_id, message, _) = consistent_transfer();
    let (local, local_credential) = member_facts("device-a", 0x41);
    loaded.membership_history_v2 = Some(
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

    let response = handler
        .execute(&AuthenticatedMember::new(peer_device_id), message)
        .await
        .unwrap();

    assert_eq!(
        response,
        MembershipHistoryMessage::AckV2(MembershipHistoryV2Ack::Invalid)
    );
    assert_eq!(repository.commits.load(Ordering::SeqCst), 0);
    assert!(repository
        .load()
        .await
        .unwrap()
        .inbound_transfers
        .is_empty());
}
