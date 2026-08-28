use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use uc_core::ids::DeviceId;
use uc_core::membership::{
    AdmissionChangeFacts, AdmissionProfileMetadata, HistoricalMembershipSignatureError,
    HistoricalMembershipSignatureVerifier, JoinerAdmissionStage, MembershipCredential,
    MembershipHistoryRelationship, SpaceJoinRecord, SpaceJoinRecordId, VersionedMembershipHistory,
    ED25519_SIGNATURE_ALGORITHM_V1,
};
use uc_core::pairing::invitation::{InvitationCode, PairingInvitation};
use uc_core::ports::ClockPort;

use super::*;
use crate::space::membership::WakeSpaceMembershipMaintenancePort;
use crate::space::membership::{
    CommitMembershipLedgerPort, LoadMembershipLedgerPort, LoadedMembershipLedger,
    MembershipEffectKind, MembershipEffectPhase, MembershipLedger, MembershipLedgerError,
    MembershipLedgerMutation, PeerReconciliationRecord, PendingMembershipEffect,
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

struct PreparedAdmission;

#[async_trait]
impl PrepareSpaceAdmissionMessagePort for PreparedAdmission {
    async fn prepare(
        &self,
        message: &AuthenticatedSpaceAdmissionMessage,
        context: &SpaceAdmissionPreparationContext,
    ) -> Result<PreparedSpaceAdmissionMessage, HandleSpaceAdmissionMessageError> {
        Ok(PreparedSpaceAdmissionMessage::Commit(
            PreparedSpaceAdmissionCommit {
                invitation_generation: context.invitation_generation,
                record: SpaceJoinRecord::new_joiner(
                    message.record_id,
                    [0x31; 16],
                    JoinerAdmissionStage::Prepared,
                ),
                membership_history_v2: context.membership_history_v2.clone(),
                relationship: PeerReconciliationRecord {
                    peer_device_id: message.source_device_id.clone(),
                    relationship: MembershipHistoryRelationship::Unknown,
                    confirmed_position: None,
                    restricted_delivery: Vec::new(),
                    updated_at_ms: 1,
                },
                effect: PendingMembershipEffect {
                    event_id: [0x32; 32],
                    kind: MembershipEffectKind::AddDevice,
                    phase: MembershipEffectPhase::Prepared,
                    affected_device_ids: vec![message.source_device_id.clone()],
                    payload: vec![0x33],
                },
                reply: vec![0x34],
            },
        ))
    }
}

struct PanickingPreparation;

#[async_trait]
impl PrepareSpaceAdmissionMessagePort for PanickingPreparation {
    async fn prepare(
        &self,
        _message: &AuthenticatedSpaceAdmissionMessage,
        _context: &SpaceAdmissionPreparationContext,
    ) -> Result<PreparedSpaceAdmissionMessage, HandleSpaceAdmissionMessageError> {
        panic!("invalid invitation must be rejected before preparation");
    }
}

struct RecordingWake(AtomicUsize);

impl WakeSpaceMembershipMaintenancePort for RecordingWake {
    fn wake(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

struct FixedClock;

impl ClockPort for FixedClock {
    fn now_ms(&self) -> i64 {
        1_800_000_000_000
    }
}

fn active_ledger() -> LoadedMembershipLedger {
    let device_id = DeviceId::new("local");
    let credential = MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0x41; 32]);
    let member_instance = credential.member_instance_id(&device_id);
    let history = VersionedMembershipHistory::new_single_member_root(
        "space-a".to_owned(),
        AdmissionChangeFacts {
            member_instance,
            device_id: device_id.clone(),
            device_name: "Local".to_owned(),
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
    let mut loaded = LoadedMembershipLedger::no_current_space();
    loaded.revision = 4;
    loaded.lineage_id = Some("space-a".to_owned());
    loaded.membership_history = Some(history.encode_persisted_v2().unwrap());
    loaded.local_device_id = Some(device_id);
    loaded.local_member_instance = Some(member_instance);
    loaded.local_join_active = true;
    let mut profile = AdmissionProfileMetadata::fresh([0x42; 16]);
    profile.device_trust_revision = 4;
    loaded.admission_profile = Some(profile);
    loaded
}

#[tokio::test]
async fn inbound_admission_commits_all_facts_before_returning_the_reply() {
    let repository = Arc::new(MemoryRepository {
        loaded: Mutex::new(active_ledger()),
        commits: AtomicUsize::new(0),
    });
    let ledger = Arc::new(MembershipLedger::new(
        repository.clone(),
        repository.clone(),
        Arc::new(AcceptingVerifier),
    ));
    let wake = Arc::new(RecordingWake(AtomicUsize::new(0)));
    let invitations =
        Arc::new(crate::space::admission::invitation::InMemoryPairingInvitationHolder::new());
    let code = InvitationCode::new("123456");
    let now = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(1_800_000_000_000).unwrap();
    let (invitation, _) = PairingInvitation::issue(
        uc_core::membership::InvitationId::from_bytes([0x44; 32]).expect("valid invitation id"),
        code.clone(),
        uc_core::pairing::invitation::FullInvitation::new("ucspace1_ABCD-1234")
            .expect("valid full invitation"),
        now,
        now + chrono::Duration::minutes(5),
        DeviceId::new("local"),
        4,
    );
    invitations.insert(invitation).await;
    let use_case = HandleSpaceAdmissionMessageUseCase::new(
        Arc::new(PreparedAdmission),
        ledger,
        wake.clone(),
        Arc::clone(&invitations),
        Arc::new(FixedClock),
    );
    let record_id = SpaceJoinRecordId::from_bytes([0x51; 32]);
    let peer = DeviceId::new("peer");

    let reply = use_case
        .execute(AuthenticatedSpaceAdmissionMessage {
            source_device_id: peer.clone(),
            record_id,
            message_id: [0x52; 32],
            payload: vec![0x53],
            invitation_code: Some(code.clone()),
        })
        .await
        .unwrap();

    assert_eq!(reply, vec![0x34]);
    assert_eq!(repository.commits.load(Ordering::SeqCst), 1);
    assert_eq!(wake.0.load(Ordering::SeqCst), 1);
    let persisted = repository.load().await.unwrap();
    assert!(persisted
        .admission_records
        .contains_key(record_id.as_bytes()));
    assert!(persisted.peer_reconciliation.contains_key(&peer));
    assert!(persisted.pending_effects.contains_key(&[0x32; 32]));
    assert_eq!(persisted.revision, 5);
    assert!(invitations.get_for_test(&code).await.is_none());
}

#[test]
fn admission_message_debug_output_redacts_sensitive_fields() {
    let message = AuthenticatedSpaceAdmissionMessage {
        source_device_id: DeviceId::new("sensitive-device"),
        record_id: SpaceJoinRecordId::from_bytes([0xa1; 32]),
        message_id: [0xa2; 32],
        payload: b"sensitive-payload".to_vec(),
        invitation_code: Some(InvitationCode::new("654321")),
    };

    let output = format!("{message:?}");

    assert!(!output.contains("sensitive-device"));
    assert!(!output.contains("sensitive-payload"));
    assert!(!output.contains("654321"));
    assert!(output.contains("[REDACTED]"));
}

#[tokio::test]
async fn invalid_invitation_is_rejected_before_protocol_preparation() {
    let repository = Arc::new(MemoryRepository {
        loaded: Mutex::new(active_ledger()),
        commits: AtomicUsize::new(0),
    });
    let ledger = Arc::new(MembershipLedger::new(
        repository.clone(),
        repository.clone(),
        Arc::new(AcceptingVerifier),
    ));
    let use_case = HandleSpaceAdmissionMessageUseCase::new(
        Arc::new(PanickingPreparation),
        ledger,
        Arc::new(RecordingWake(AtomicUsize::new(0))),
        Arc::new(crate::space::admission::invitation::InMemoryPairingInvitationHolder::new()),
        Arc::new(FixedClock),
    );

    let error = use_case
        .execute(AuthenticatedSpaceAdmissionMessage {
            source_device_id: DeviceId::new("peer"),
            record_id: SpaceJoinRecordId::from_bytes([0xb1; 32]),
            message_id: [0xb2; 32],
            payload: vec![0xb3],
            invitation_code: Some(InvitationCode::new("000000")),
        })
        .await
        .unwrap_err();

    assert_eq!(error, HandleSpaceAdmissionMessageError::Invalid);
    assert_eq!(repository.commits.load(Ordering::SeqCst), 0);
}
