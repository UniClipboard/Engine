use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use uc_core::membership::{
    AdmissionOutboxMessage, AdmissionOutboxPurpose, AdmissionProfileMetadata, JoinerAdmissionStage,
    SpaceJoinRecord, SpaceJoinRecordId,
};
use uc_core::ports::SettingsPort;

use super::*;
use crate::space::membership_ledger::{
    CommitMembershipLedgerPort, LoadMembershipLedgerPort, LoadedMembershipLedger, MembershipLedger,
    MembershipLedgerError, MembershipLedgerMutation,
};
use crate::space::remove_space_member::WakeSpaceMembershipMaintenancePort;

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
        if loaded.revision != mutation.expected_revision {
            return Err(MembershipLedgerError::Conflict);
        }
        self.commits.fetch_add(1, Ordering::SeqCst);
        *loaded = mutation.replacement;
        Ok(loaded.clone())
    }
}

struct UnusedVerifier;

impl uc_core::membership::HistoricalMembershipSignatureVerifier for UnusedVerifier {
    fn verify(
        &self,
        _signature_algorithm_version: u16,
        _public_key: &[u8],
        _payload: &[u8],
        _signature: &[u8],
    ) -> Result<bool, uc_core::membership::HistoricalMembershipSignatureError> {
        Ok(true)
    }
}

struct InMemorySettings(Mutex<uc_core::settings::model::Settings>);

#[async_trait]
impl SettingsPort for InMemorySettings {
    async fn load(&self) -> anyhow::Result<uc_core::settings::model::Settings> {
        Ok(self.0.lock().unwrap().clone())
    }

    async fn save(&self, settings: &uc_core::settings::model::Settings) -> anyhow::Result<()> {
        *self.0.lock().unwrap() = settings.clone();
        Ok(())
    }
}

struct PreparedProtocol;

#[async_trait]
impl PrepareJoinSpacePort for PreparedProtocol {
    async fn prepare(&self, _input: &JoinSpaceInput) -> Result<PreparedJoinSpace, JoinSpaceError> {
        let record_id = SpaceJoinRecordId::from_bytes([0xc1; 32]);
        let mut record =
            SpaceJoinRecord::new_joiner(record_id, [0xc2; 16], JoinerAdmissionStage::Initiated);
        record.local_join_ordinal = Some(1);
        record.outboxes.push(AdmissionOutboxMessage {
            purpose: AdmissionOutboxPurpose::JoinRequest,
            recipient: vec![0xc3],
            message_id: [0xc4; 32],
            predecessor_message_id: None,
            payload: vec![0xc5],
            superseded: false,
        });
        Ok(PreparedJoinSpace {
            record,
            expected_membership_history_v2: None,
            requires_session_transition: true,
        })
    }
}

struct WakeCounter(AtomicUsize);

impl WakeSpaceMembershipMaintenancePort for WakeCounter {
    fn wake(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn join_persists_one_attempt_before_network_recovery() {
    let mut loaded = LoadedMembershipLedger::no_current_space();
    loaded.admission_profile = Some(AdmissionProfileMetadata::fresh([0xc6; 16]));
    let repository = Arc::new(MemoryRepository {
        loaded: Mutex::new(loaded),
        commits: AtomicUsize::new(0),
    });
    let ledger = Arc::new(MembershipLedger::new(
        repository.clone(),
        repository.clone(),
        Arc::new(UnusedVerifier),
    ));
    let wake = Arc::new(WakeCounter(AtomicUsize::new(0)));
    let join = JoinSpaceUseCase::new(
        Arc::new(InMemorySettings(Mutex::new(Default::default()))),
        Arc::new(PreparedProtocol),
        ledger,
        wake.clone(),
    );

    let result = join
        .execute(JoinSpaceInput {
            invitation_code: uc_core::pairing::InvitationCode::new("join-code"),
            device_name: Some("New Device".to_owned()),
            passphrase: uc_core::crypto::domain::Passphrase::new("passphrase"),
            preserve_unreadable_history: false,
        })
        .await
        .unwrap();

    assert!(matches!(
        result.status,
        crate::space::admission::CurrentJoinStatus::Pending {
            join_id,
            ..
        } if join_id == [0xc2; 16]
    ));
    assert!(result.requires_session_transition);
    assert_eq!(repository.commits.load(Ordering::SeqCst), 1);
    assert_eq!(wake.0.load(Ordering::SeqCst), 1);
}
