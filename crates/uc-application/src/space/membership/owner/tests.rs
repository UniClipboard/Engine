use uc_core::ids::DeviceId;
use uc_core::membership::{LedgerInput, LedgerOutcome, MembershipHistoryAckV3, PeerSyncResult};

use crate::space::lifecycle::SpaceMembershipResetPort;
use crate::space::membership::testing::{EstablishedSpace, OwnerFixture};
use crate::space::membership::{
    CurrentSpaceMemberScopePort, MembershipLedgerError, MembershipRecord,
};

fn active_space() -> OwnerFixture {
    OwnerFixture::new(EstablishedSpace::new(&["device-a", "device-b"]).record("device-a", 4))
}

fn remember_completed_transfer(
    draft: &mut super::MembershipDraft,
) -> Result<(), MembershipLedgerError> {
    draft
        .history_exchange_mut()?
        .completed_inbound_transfers
        .insert(
            (DeviceId::new("device-b"), [7; 32]),
            MembershipHistoryAckV3::Invalid,
        );
    Ok(())
}

#[tokio::test]
async fn a_change_without_differences_writes_nothing() {
    let fixture = active_space();

    let committed = fixture.owner.commit(|_| Ok(())).await.unwrap();

    assert_eq!(committed.view.revision(), 4);
    assert_eq!(fixture.records.commit_count(), 0);
    assert!(fixture.events.committed_revisions().is_empty());
    assert_eq!(fixture.wake.wake_count(), 0);
}

#[tokio::test]
async fn companion_data_advances_the_revision_without_announcing_a_trust_change() {
    let fixture = active_space();

    let committed = fixture
        .owner
        .commit(remember_completed_transfer)
        .await
        .unwrap();

    assert_eq!(committed.view.revision(), 5);
    assert_eq!(fixture.records.record().revision(), 5);
    assert_eq!(fixture.records.commit_count(), 1);
    assert!(fixture.events.committed_revisions().is_empty());
    // 读模型与记录在同一次提交中写入。
    assert!(fixture.records.last_projection().is_some());
}

#[tokio::test]
async fn a_failed_commit_keeps_the_stored_state_and_reloads_it() {
    let fixture = active_space();
    fixture.owner.load().await.unwrap();
    let loads_before = fixture.records.load_count();
    fixture.records.fail_next_commits(1);

    let error = fixture
        .owner
        .commit(remember_completed_transfer)
        .await
        .err()
        .unwrap();

    assert!(matches!(error, MembershipLedgerError::Unavailable { .. }));
    assert_eq!(fixture.records.record().revision(), 4);
    let view = fixture.owner.load().await.unwrap();
    assert_eq!(view.revision(), 4);
    assert_eq!(fixture.records.load_count(), loads_before + 1);
    assert!(fixture.events.committed_revisions().is_empty());
}

#[tokio::test]
async fn a_revision_conflict_is_reported_to_the_caller() {
    let fixture = active_space();
    fixture.records.conflict_next_commits(1);

    let error = fixture
        .owner
        .commit(remember_completed_transfer)
        .await
        .err()
        .unwrap();

    assert!(matches!(error, MembershipLedgerError::Conflict));
    let retried = fixture
        .owner
        .commit(remember_completed_transfer)
        .await
        .unwrap();
    assert_eq!(retried.view.revision(), 5);
}

#[tokio::test]
async fn a_reopened_owner_reads_the_committed_state() {
    let fixture = active_space();
    fixture
        .owner
        .commit(remember_completed_transfer)
        .await
        .unwrap();

    let reopened = fixture.reopen();
    let view = reopened.load().await.unwrap();

    assert_eq!(view.revision(), 5);
    assert_eq!(
        view.require_space()
            .unwrap()
            .history_exchange()
            .completed_inbound_transfers
            .len(),
        1
    );
}

#[tokio::test]
async fn reset_ends_the_space_and_keeps_the_revision_increasing() {
    let fixture = active_space();

    fixture.owner.reset().await.unwrap();

    assert!(matches!(
        fixture.records.record(),
        MembershipRecord::NoSpace { revision: 5 }
    ));
    assert_eq!(fixture.events.committed_revisions(), vec![5]);
    assert!(fixture.owner.load().await.unwrap().space().is_none());
    // 已无 Space 时重置不再写入。
    fixture.owner.reset().await.unwrap();
    assert_eq!(fixture.records.commit_count(), 1);
}

#[tokio::test]
async fn a_confirmed_history_exchange_offers_the_peer_to_group_update_delivery_once() {
    let fixture = active_space();
    let peer = DeviceId::new("device-b");

    let committed = fixture
        .owner
        .commit(|draft| {
            let position = draft
                .require_space()?
                .history()
                .current_position()
                .map_err(MembershipLedgerError::corrupt_from)?;
            draft
                .apply(LedgerInput::HistorySyncSelected { peers: vec![peer] })
                .map_err(MembershipLedgerError::corrupt_from)?;
            draft
                .apply(LedgerInput::HistorySyncFinished {
                    peer,
                    synced_position: position,
                    result: PeerSyncResult::Confirmed,
                })
                .map_err(MembershipLedgerError::corrupt_from)
        })
        .await
        .unwrap();

    assert_eq!(committed.output, LedgerOutcome::Applied);
    assert!(fixture.wake.wake_count() > 0);
    assert_eq!(fixture.owner.take_reachable_peers(), vec![peer]);
    assert!(fixture.owner.take_reachable_peers().is_empty());
}

#[tokio::test]
async fn a_replaced_database_is_reloaded_and_announced_before_the_next_read() {
    let fixture = active_space();
    fixture.owner.load().await.unwrap();
    let mut changes = fixture.owner.subscribe_changes();
    changes.mark_unchanged();
    let loads_before = fixture.records.load_count();

    // 同一数据库上重复读取只使用已发布状态。
    fixture.owner.load().await.unwrap();
    assert_eq!(fixture.records.load_count(), loads_before);
    assert!(!changes.has_changed().unwrap());

    fixture
        .records
        .replace_database(MembershipRecord::NoSpace { revision: 0 });
    let view = fixture.owner.load().await.unwrap();

    assert!(view.space().is_none());
    assert_eq!(fixture.records.load_count(), loads_before + 1);
    assert!(changes.has_changed().unwrap());
}

#[tokio::test]
async fn a_commit_after_a_database_replacement_starts_from_the_new_record() {
    let fixture = active_space();
    fixture.owner.load().await.unwrap();
    fixture
        .records
        .replace_database(MembershipRecord::NoSpace { revision: 9 });

    let committed = fixture
        .owner
        .commit(|draft| {
            assert!(draft.space().is_none());
            Ok(())
        })
        .await
        .unwrap();

    assert_eq!(committed.view.revision(), 9);
    assert_eq!(fixture.records.commit_count(), 0);
}
