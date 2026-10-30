//! 证据验证与提交之间切换控制数据库，不能用相同修订号接受旧操作。
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

use uc_core::membership::{
    HistoricalMembershipSignatureError, HistoricalMembershipSignatureVerifier, LedgerInput,
    MembershipConflictEvidenceV3, PeerEvidence,
};

use super::Fixture;
use crate::space::membership::testing::{AcceptingVerifier, MemoryMembershipRecords};
use crate::space::membership::{
    ledger_error, MembershipLedgerError, MembershipOwner, MembershipRecord,
    ReconcileMembershipEvidenceUseCase,
};

type Replacement = (Arc<MemoryMembershipRecords>, MembershipRecord);

#[derive(Default)]
struct ReplaceDatabaseDuringVerification {
    replacement: Mutex<Option<Replacement>>,
    verified: Mutex<Option<Arc<Notify>>>,
}

impl HistoricalMembershipSignatureVerifier for ReplaceDatabaseDuringVerification {
    fn verify(
        &self,
        version: u16,
        public_key: &[u8],
        payload: &[u8],
        signature: &[u8],
    ) -> Result<bool, HistoricalMembershipSignatureError> {
        let replacement = self.replacement.lock().unwrap().take();
        if let Some((records, record)) = replacement {
            records.replace_database(record);
        }
        if let Some(verified) = self.verified.lock().unwrap().as_ref() {
            verified.notify_one();
        }
        AcceptingVerifier.verify(version, public_key, payload, signature)
    }
}

async fn assert_replacement_rejected(scenario: &str) {
    let fixture = Fixture::new();
    let peer = fixture.peer.device_id;
    if scenario != "diverged" {
        let evidence = if scenario == "consistent_noop" {
            PeerEvidence::Consistent
        } else {
            PeerEvidence::Diverged
        };
        fixture
            .ledger
            .commit(move |draft| {
                draft
                    .apply(LedgerInput::PeerEvidenceReconciled {
                        source: peer,
                        history: None,
                        evidence,
                    })
                    .map_err(ledger_error)?;
                Ok(())
            })
            .await
            .unwrap();
    }
    let remote = if scenario == "diverged" {
        fixture.remote.clone()
    } else {
        fixture.owner_fixture.records.ledger().history().clone()
    };
    let evidence = MembershipConflictEvidenceV3 {
        transfer_id: remote.current_position().unwrap().history_digest,
        pages: remote
            .export_conflict_evidence_pages_v2(fixture.peer.clone())
            .unwrap(),
    };
    let verifier = Arc::new(ReplaceDatabaseDuringVerification::default());
    let collaborators = &fixture.owner_fixture;
    let owner = Arc::new(MembershipOwner::new(
        collaborators.records.clone(),
        verifier.clone(),
        collaborators.clock.clone(),
        collaborators.events.clone(),
        collaborators.wake.clone(),
    ));
    // 首次加载完成后再启用切换；触发点精确落在远端证据验证中。
    owner.load().await.unwrap();
    let original_generation = owner.generation();
    let replacement = collaborators.records.record();
    let original_revision = replacement.revision();
    let original_commits = collaborators.records.commit_count();
    *verifier.replacement.lock().unwrap() = Some((collaborators.records.clone(), replacement));
    let result = ReconcileMembershipEvidenceUseCase::new(owner.clone())
        .execute(&peer, &evidence)
        .await;
    assert_eq!(
        owner.generation(),
        original_generation + 1,
        "切换屏障必须实际触发"
    );
    assert!(
        matches!(result, Err(MembershipLedgerError::Unavailable { .. })),
        "{scenario}: 旧数据库的证据不能在新数据库继续写入或回复"
    );
    assert_eq!(collaborators.records.commit_count(), original_commits);
    assert_eq!(collaborators.records.record().revision(), original_revision);
}

#[tokio::test]
async fn diverged_rejects_replacement_during_verification() {
    assert_replacement_rejected("diverged").await;
}

#[tokio::test]
async fn consistent_update_rejects_replacement_during_verification() {
    assert_replacement_rejected("consistent_update").await;
}

#[tokio::test]
async fn consistent_noop_rejects_replacement_during_verification() {
    assert_replacement_rejected("consistent_noop").await;
}

#[tokio::test]
async fn diverged_rejects_replacement_while_waiting_to_commit() {
    let fixture = Fixture::new();
    let peer = fixture.peer.device_id;
    let remote = &fixture.remote;
    let evidence = MembershipConflictEvidenceV3 {
        transfer_id: remote.current_position().unwrap().history_digest,
        pages: remote
            .export_conflict_evidence_pages_v2(fixture.peer.clone())
            .unwrap(),
    };
    let verifier = Arc::new(ReplaceDatabaseDuringVerification::default());
    let collaborators = &fixture.owner_fixture;
    let owner = Arc::new(MembershipOwner::new(
        collaborators.records.clone(),
        verifier.clone(),
        collaborators.clock.clone(),
        collaborators.events.clone(),
        collaborators.wake.clone(),
    ));
    owner.load().await.unwrap();
    let generation = owner.generation();
    let original = collaborators.records.record();
    let original_commits = collaborators.records.commit_count();
    let ready = Arc::new(Notify::new());
    let verified = Arc::new(Notify::new());
    // 先占用既有Owner提交边界，直到旧证据验证结束，再替换数据库并释放锁。
    let blocker = {
        let owner = owner.clone();
        let records = collaborators.records.clone();
        let ready = ready.clone();
        let verified = verified.clone();
        let original = original.clone();
        tokio::spawn(async move {
            owner
                .run_and_commit_in_generation(
                    generation,
                    |_| Ok(()),
                    async move {
                        ready.notify_one();
                        verified.notified().await;
                        records.replace_database(original);
                        Ok::<(), ()>(())
                    },
                    |_| Ok(()),
                )
                .await
        })
    };
    ready.notified().await;
    *verifier.verified.lock().unwrap() = Some(verified);
    let result = ReconcileMembershipEvidenceUseCase::new(owner.clone())
        .execute(&peer, &evidence)
        .await;
    blocker.await.unwrap().unwrap().unwrap();
    assert_eq!(owner.generation(), generation + 1);
    assert!(
        matches!(result, Err(MembershipLedgerError::Unavailable { .. })),
        "排队期间切换数据库后，不得在新库重新加载并提交旧证据"
    );
    assert_eq!(collaborators.records.commit_count(), original_commits);
    assert_eq!(
        collaborators.records.record().revision(),
        original.revision()
    );
}
