use super::QueryDeviceGroupChoicesUseCase;
use crate::space::admission::CurrentJoinStatus;
use crate::space::membership::{
    CommitMembershipLedgerPort, DeviceTrustObservation, LoadCurrentJoinStatusPort,
    LoadDeviceTrustObservationsPort, LoadMembershipLedgerPort, LoadedMembershipLedger,
    MembershipLedger, MembershipLedgerError, MembershipLedgerMutation, QueryDeviceTrustError,
    QueryDeviceTrustUseCase, ResolveMembershipConflictUseCase,
};
use async_trait::async_trait;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use uc_core::membership::{
    HistoricalMembershipSignatureError, HistoricalMembershipSignatureVerifier,
};

#[derive(Default)]
struct ChangingLedger(AtomicU64);

#[async_trait]
impl LoadMembershipLedgerPort for ChangingLedger {
    async fn load(&self) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        let mut record = LoadedMembershipLedger::no_current_space();
        record.revision = self.0.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(record)
    }
}

#[async_trait]
impl CommitMembershipLedgerPort for ChangingLedger {
    async fn compare_and_commit(
        &self,
        _: MembershipLedgerMutation,
    ) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        panic!("query must not write membership");
    }
}

struct EmptyInputs;

impl HistoricalMembershipSignatureVerifier for EmptyInputs {
    fn verify(
        &self,
        _: u16,
        _: &[u8],
        _: &[u8],
        _: &[u8],
    ) -> Result<bool, HistoricalMembershipSignatureError> {
        panic!("empty history has no signatures");
    }
}

#[async_trait]
impl LoadDeviceTrustObservationsPort for EmptyInputs {
    async fn load(
        &self,
        _: &[uc_core::DeviceId],
    ) -> Result<Vec<DeviceTrustObservation>, QueryDeviceTrustError> {
        panic!("empty space has no devices");
    }
}

#[async_trait]
impl LoadCurrentJoinStatusPort for EmptyInputs {
    async fn load_current_join(&self) -> Result<Option<CurrentJoinStatus>, QueryDeviceTrustError> {
        Ok(None)
    }
}

#[tokio::test]
async fn concurrent_membership_updates_cannot_split_a_device_group_query() {
    let repository = Arc::new(ChangingLedger::default());
    let ledger = Arc::new(MembershipLedger::new(
        repository.clone(),
        repository.clone(),
        Arc::new(EmptyInputs),
    ));
    let trust = Arc::new(QueryDeviceTrustUseCase::new(
        ledger.clone(),
        Arc::new(EmptyInputs),
        Arc::new(EmptyInputs),
    ));
    let conflicts = Arc::new(ResolveMembershipConflictUseCase::new(
        ledger.clone(),
        trust.clone(),
    ));
    let query = QueryDeviceGroupChoicesUseCase::new(ledger, trust, conflicts);
    let view = query
        .execute()
        .await
        .expect("one query must return one coherent membership snapshot");
    assert_eq!(view.revision, view.device_trust.revision);
    assert_eq!(view.revision, view.conflicts.revision);
    assert_eq!(
        repository.0.load(Ordering::SeqCst),
        1,
        "read once, do not retry mixed snapshots"
    );
}
