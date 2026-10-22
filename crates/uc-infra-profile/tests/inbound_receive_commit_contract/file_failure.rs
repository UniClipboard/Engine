//! 实际SQLite、产物AEAD与重新打开查询；网络终态和本地失败分别保留。
use super::*;
use uc_core::ports::{
    GetEntryReceiveProgressPort, ReceiveFileFailure, ReceiveItemFailureReason,
    TrackedFileTransferStatus,
};

async fn fail(
    fixture: &Fixture,
    index: usize,
    failure: ReceiveFileFailure,
) -> Result<(), InboundReceiveCommitError> {
    fixture
        .commit
        .commit_inbound_receive(&InboundReceiveSettlement::FileFailed {
            entry_id: EntryId::from("entry"),
            attempt_id: "a1".into(),
            artifact: fixture.artifacts[index].clone(),
            failure,
            now_ms: 3,
        })
        .await
}

#[tokio::test]
async fn post_download_save_failure_and_single_cancel_survive_reopen_without_reverting_network_terminal(
) {
    let fixture = Fixture::new().await;
    // 真正的保存事务失败，不以手动标记attempt代替故障。
    fixture.executor.run(|conn| {
        diesel::sql_query("CREATE TRIGGER reject_file_save BEFORE INSERT ON clipboard_entry BEGIN SELECT RAISE(ABORT, 'injected save failure'); END").execute(conn)?;
        Ok(())
    }).unwrap();
    let before = fixture.snapshot();
    let error = fixture
        .commit
        .commit_inbound_receive(&fixture.progress(0))
        .await
        .unwrap_err();
    assert!(std::error::Error::source(&error).is_some());
    assert_eq!(fixture.snapshot(), before);
    std::fs::remove_file(&fixture.artifacts[0].final_path).unwrap();
    fail(
        &fixture,
        0,
        ReceiveFileFailure::Failed(ReceiveItemFailureReason::StorageUnavailable),
    )
    .await
    .unwrap();
    std::fs::remove_file(&fixture.artifacts[1].final_path).unwrap();
    fail(&fixture, 1, ReceiveFileFailure::Cancelled)
        .await
        .unwrap();
    assert!(fixture
        .journal
        .list_unsettled_receive_artifacts()
        .await
        .unwrap()[0]
        .artifacts
        .is_empty());
    fixture
        .attempts
        .begin_receive_failure("entry", "a1", 4)
        .await
        .unwrap();
    fixture
        .commit
        .commit_inbound_receive(&InboundReceiveSettlement::NoEntry {
            entry_id: EntryId::from("entry"),
            attempt_id: "a1".into(),
            terminal: PartialReceiveTerminal::Failed,
            artifacts: NoEntryReceiveArtifacts::RolledBack,
            now_ms: 5,
        })
        .await
        .unwrap();
    let executor = DieselSqliteExecutor::new(
        init_db_pool(
            fixture
                .directory
                .path()
                .join("progress.sqlite")
                .to_str()
                .unwrap(),
        )
        .unwrap(),
    );
    let rows = executor
        .run(|conn| {
            Ok(file_transfer::table
                .order(file_transfer::transfer_id)
                .select((file_transfer::status, file_transfer::failure_code))
                .load::<(String, Option<String>)>(conn)?)
        })
        .unwrap();
    assert_eq!(
        rows,
        vec![
            ("completed".into(), Some("storage_unavailable".into())),
            ("completed".into(), Some("local_user".into()))
        ]
    );
    let reader =
        DieselFileTransferRepository::new(executor, Arc::new(FixedSubkey), Arc::new(FixedProfile));
    let progress = reader
        .get_entry_receive_progress("entry")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(progress.items_completed, 0);
    assert_eq!(progress.completed_bytes, 0);
    assert_eq!(progress.items[0].status, TrackedFileTransferStatus::Failed);
    assert_eq!(
        progress.items[0].failure_reason,
        Some(ReceiveItemFailureReason::StorageUnavailable)
    );
    assert_eq!(
        progress.items[1].status,
        TrackedFileTransferStatus::Cancelled
    );
    assert_eq!(progress.items[1].failure_reason, None);
}

#[tokio::test]
async fn failure_fact_and_artifact_release_roll_back_together_on_late_sql_error() {
    let fixture = Fixture::new().await;
    let before = fixture.snapshot();
    fixture.executor.run(|conn| {
        diesel::sql_query("CREATE TRIGGER reject_failure_fact BEFORE UPDATE OF failure_code ON file_transfer BEGIN SELECT RAISE(ABORT, 'injected settlement failure'); END").execute(conn)?;
        Ok(())
    }).unwrap();
    let error = fail(&fixture, 0, ReceiveFileFailure::Cancelled)
        .await
        .unwrap_err();
    assert!(std::error::Error::source(&error).is_some());
    assert_eq!(fixture.snapshot(), before);
    let codes = fixture
        .executor
        .run(|conn| {
            Ok(file_transfer::table
                .select(file_transfer::failure_code)
                .load::<Option<String>>(conn)?)
        })
        .unwrap();
    assert_eq!(codes, vec![None, None]);
}

#[tokio::test]
async fn local_failure_cannot_withdraw_delivered_file_or_change_terminal_attempt() {
    let fixture = Fixture::new().await;
    fixture
        .commit
        .commit_inbound_receive(&fixture.progress(0))
        .await
        .unwrap();
    let before = fixture.snapshot();
    assert!(fail(&fixture, 0, ReceiveFileFailure::Cancelled)
        .await
        .is_err());
    assert_eq!(fixture.snapshot(), before);
    assert_eq!(
        std::fs::read(&fixture.artifacts[0].final_path).unwrap(),
        b"saved-content"
    );
    fixture
        .attempts
        .begin_receive_failure("entry", "a1", 4)
        .await
        .unwrap();
    fixture
        .commit
        .commit_inbound_receive(&InboundReceiveSettlement::NoEntry {
            entry_id: EntryId::from("entry"),
            attempt_id: "a1".into(),
            terminal: PartialReceiveTerminal::Failed,
            artifacts: NoEntryReceiveArtifacts::None,
            now_ms: 5,
        })
        .await
        .unwrap();
    let before = fixture.snapshot();
    assert!(matches!(
        fail(&fixture, 1, ReceiveFileFailure::Cancelled).await,
        Err(InboundReceiveCommitError::AttemptStateMismatch)
    ));
    assert_eq!(fixture.snapshot(), before);
}

#[tokio::test]
async fn later_aggregate_cannot_claim_a_locally_failed_file_as_delivered() {
    let fixture = Fixture::new().await;
    fail(&fixture, 0, ReceiveFileFailure::Cancelled)
        .await
        .unwrap();
    assert!(fixture
        .attempts
        .claim_receive_commit("entry", "a1", 4)
        .await
        .unwrap());
    let before = fixture.snapshot();
    assert!(fixture
        .commit
        .commit_inbound_receive(&InboundReceiveSettlement::Complete {
            record: record("entry"),
            attempt_id: "a1".into(),
            delivered_item_ids: vec![fixture.artifacts[0].item_id.clone()],
            file_set: None,
            artifacts: CompletedReceiveArtifacts::None,
            now_ms: 5,
        })
        .await
        .is_err());
    assert_eq!(fixture.snapshot(), before);
}
