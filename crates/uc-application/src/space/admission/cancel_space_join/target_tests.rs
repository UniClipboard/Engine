use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use uc_core::membership::{
    AdmissionOutboxMessage, AdmissionOutboxPurpose, AdmissionProfileMetadata,
    AdmissionRejectionReason, JoinerAdmissionStage, SpaceJoinRecord, SpaceJoinRecordId,
};

use super::*;
use crate::space::membership::WakeSpaceMembershipMaintenancePort;
use crate::space::membership::{
    CommitMembershipLedgerPort, LoadMembershipLedgerPort, LoadedMembershipLedger, MembershipLedger,
    MembershipLedgerError, MembershipLedgerMutation,
};

struct MemoryRepository(Mutex<LoadedMembershipLedger>);

#[async_trait]
impl LoadMembershipLedgerPort for MemoryRepository {
    async fn load(&self) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        Ok(self.0.lock().unwrap().clone())
    }
}

#[async_trait]
impl CommitMembershipLedgerPort for MemoryRepository {
    async fn compare_and_commit(
        &self,
        mutation: MembershipLedgerMutation,
    ) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        let mut loaded = self.0.lock().unwrap();
        if loaded.revision != mutation.expected_revision {
            return Err(MembershipLedgerError::Conflict);
        }
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

struct WakeCounter(AtomicUsize);

impl WakeSpaceMembershipMaintenancePort for WakeCounter {
    fn wake(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn pending_join_is_rejected_atomically_before_commit_boundary() {
    let record_id = SpaceJoinRecordId::from_bytes([0xb1; 32]);
    let mut record =
        SpaceJoinRecord::new_joiner(record_id, [0xb2; 16], JoinerAdmissionStage::Initiated);
    record.local_join_ordinal = Some(1);
    record.outboxes.push(AdmissionOutboxMessage {
        purpose: AdmissionOutboxPurpose::JoinRequest,
        recipient: vec![0xb3],
        message_id: [0xb4; 32],
        predecessor_message_id: None,
        payload: vec![0xb5],
        superseded: false,
    });
    let mut loaded = LoadedMembershipLedger::no_current_space();
    loaded.admission_profile = Some(AdmissionProfileMetadata::fresh([0xb6; 16]));
    loaded
        .admission_records
        .insert(*record_id.as_bytes(), record);
    let repository = Arc::new(MemoryRepository(Mutex::new(loaded)));
    let ledger = Arc::new(MembershipLedger::new(
        repository.clone(),
        repository,
        Arc::new(UnusedVerifier),
    ));
    let wake = Arc::new(WakeCounter(AtomicUsize::new(0)));
    let cancel = CancelSpaceJoinUseCase::new(ledger, wake.clone());

    let status = cancel.execute([0xb2; 16]).await.unwrap();

    assert!(matches!(
        status,
        crate::space::admission::CurrentJoinStatus::Rejected {
            reason: AdmissionRejectionReason::Cancelled,
            ..
        }
    ));
    assert_eq!(wake.0.load(Ordering::SeqCst), 1);
}
