mod v3;
use super::*;
use crate::db::executor::DieselSqliteExecutor;
use crate::db::pool::init_db_pool;
use crate::db::repositories::DieselEntryReceiveAttemptRepository;
use crate::db::schema::entry_receive_attempt;
use uc_application::deps::{ReceiveRetryItem, ReceiveRetryItemState};
use uc_core::ports::{
    AttemptState, BeginReceiveAttemptPort, GetEntryReceiveProgressPort, ReceiveItemRole,
    TrackedFileTransferStatus,
};

fn input() -> BeginReceiveRetryInput {
    BeginReceiveRetryInput {
        entry_id: "entry".into(),
        expected_attempt_id: "old".into(),
        attempt_id: "new".into(),
        source_device_id: "source".into(),
        directory: false,
        now_ms: 10,
        items: (0..3)
            .map(|index| ReceiveRetryItem {
                item_id: format!("entry:attempt:new:member:{index}"),
                role: ReceiveItemRole::File,
                filename: "PRIVATE_ORIGINAL_FILENAME".into(),
                bytes: 7,
                state: match index {
                    0 => ReceiveRetryItemState::Completed,
                    1 => ReceiveRetryItemState::Pending,
                    _ => ReceiveRetryItemState::Cancelled { timed_out: false },
                },
            })
            .collect(),
    }
}

#[tokio::test]
async fn retry_acceptance_atomically_preserves_complete_original_items_after_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("retry.sqlite");
    let executor = DieselSqliteExecutor::new(init_db_pool(path.to_str().unwrap()).unwrap());
    let (keys, profile) = crate::file_transfer::persistence_cipher::test_keys::ports();
    let attempts = DieselEntryReceiveAttemptRepository::new(executor.clone());
    attempts
        .begin_first_receive("entry", "old", 1)
        .await
        .unwrap();
    executor
        .run(|conn| {
            diesel::update(entry_receive_attempt::table)
                .set(entry_receive_attempt::attempt_state.eq(AttemptState::Failed.to_string()))
                .execute(conn)?;
            Ok(())
        })
        .unwrap();
    let repo = DieselFileTransferRepository::new(executor.clone(), keys.clone(), profile.clone());
    assert_eq!(
        repo.begin_receive_retry(input()).await.unwrap(),
        BeginReceiveOutcome::Begun
    );
    drop(repo);
    let reopened = DieselFileTransferRepository::new(
        DieselSqliteExecutor::new(init_db_pool(path.to_str().unwrap()).unwrap()),
        keys,
        profile,
    );
    let progress = reopened
        .get_entry_receive_progress("entry")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(progress.attempt_id, "new");
    assert_eq!(
        progress.items.len(),
        3,
        "接受后重启仍须包含继承、所选与未选项"
    );
    assert_eq!(progress.items_total, 3);
    assert_eq!(progress.items_completed, 1);
    assert_eq!(progress.completed_bytes, 7);
    assert_eq!(progress.total_bytes, 21);
    assert_eq!(
        progress
            .items
            .iter()
            .map(|item| item.status)
            .collect::<Vec<_>>(),
        vec![
            TrackedFileTransferStatus::Completed,
            TrackedFileTransferStatus::Pending,
            TrackedFileTransferStatus::Cancelled
        ]
    );
}

type Repo = DieselFileTransferRepository<DieselSqliteExecutor>;

async fn database() -> (Repo, DieselSqliteExecutor, tempfile::TempDir) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("retry.sqlite");
    let executor = DieselSqliteExecutor::new(init_db_pool(path.to_str().unwrap()).unwrap());
    let (keys, profile) = crate::file_transfer::persistence_cipher::test_keys::ports();
    let attempts = DieselEntryReceiveAttemptRepository::new(executor.clone());
    attempts
        .begin_first_receive("entry", "old", 1)
        .await
        .unwrap();
    executor
        .run(|conn| {
            diesel::update(entry_receive_attempt::table)
                .set(entry_receive_attempt::attempt_state.eq(AttemptState::Failed.to_string()))
                .execute(conn)?;
            Ok(())
        })
        .unwrap();
    (
        DieselFileTransferRepository::new(executor.clone(), keys, profile),
        executor,
        directory,
    )
}

#[tokio::test]
async fn retry_transaction_failure_restores_old_attempt_and_all_other_rows() {
    use crate::db::schema::{directory_publish_log, file_transfer};
    use uc_core::ports::{GetEntryAttemptPort, PendingInboundTransfer, RecordReceiverTransferPort};
    let (repo, executor, _directory) = database().await;
    repo.upsert_pending_transfer(&PendingInboundTransfer {
        item_role: Some(ReceiveItemRole::File),
        transfer_id: input().items[2].item_id.clone(),
        entry_id: "other-entry".into(),
        attempt_id: Some("other-attempt".into()),
        origin_device_id: "other-source".into(),
        filename: "PRIVATE_UNRELATED_FILENAME".into(),
        file_size: Some(7),
        cached_path: "PRIVATE_OLD_PATH".into(),
        created_at_ms: 1,
    })
    .await
    .unwrap();
    let mut command = input();
    command.directory = true;
    for item in &mut command.items {
        item.state = ReceiveRetryItemState::Pending;
    }
    let error = repo.begin_receive_retry(command).await.unwrap_err();
    assert!(std::error::Error::source(&error).is_some());
    let attempts = DieselEntryReceiveAttemptRepository::new(executor.clone());
    assert_eq!(
        attempts
            .get_entry_attempt("entry")
            .await
            .unwrap()
            .unwrap()
            .current_attempt_id,
        "old"
    );
    let counts = executor
        .run(|conn| {
            Ok((
                file_transfer::table.count().get_result::<i64>(conn)?,
                directory_publish_log::table
                    .count()
                    .get_result::<i64>(conn)?,
            ))
        })
        .unwrap();
    assert_eq!(counts, (1, 0));
    assert_eq!(
        repo.get_entry_receive_progress("other-entry")
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn retry_stale_or_active_authority_never_inserts_projection_rows() {
    use crate::db::schema::file_transfer;
    for active in [false, true] {
        let (repo, executor, _directory) = database().await;
        let mut command = input();
        if active {
            executor
                .run(|conn| {
                    diesel::update(entry_receive_attempt::table)
                        .set(
                            entry_receive_attempt::attempt_state
                                .eq(AttemptState::Receiving.to_string()),
                        )
                        .execute(conn)?;
                    Ok(())
                })
                .unwrap();
        } else {
            command.expected_attempt_id = "stale".into();
        }
        assert_eq!(
            repo.begin_receive_retry(command).await.unwrap(),
            if active {
                BeginReceiveOutcome::AlreadyReceiving
            } else {
                BeginReceiveOutcome::Superseded
            }
        );
        assert_eq!(
            executor
                .run(|conn| file_transfer::table
                    .count()
                    .get_result::<i64>(conn)
                    .map_err(Into::into))
                .unwrap(),
            0
        );
    }
}

#[tokio::test]
async fn retry_directory_is_one_unit_even_without_members_or_a_prepared_output_root() {
    use crate::db::repositories::DieselDirectoryPublishLogRepository;
    use uc_core::ports::{
        GetDirectoryPublishRecordPort, PublishPhase, ReceiveItemKind, RecordDirectoryPublishPort,
    };
    for empty in [false, true] {
        let (repo, executor, directory) = database().await;
        let mut command = input();
        command.directory = true;
        for item in &mut command.items {
            item.state = ReceiveRetryItemState::Pending;
        }
        if empty {
            command.items.clear();
        }
        repo.begin_receive_retry(command).await.unwrap();
        let progress = repo
            .get_entry_receive_progress("entry")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(progress.items.len(), 1);
        assert_eq!(progress.items[0].kind, ReceiveItemKind::DirectoryUnit);
        let (keys, profile) = crate::file_transfer::persistence_cipher::test_keys::ports();
        let publications = DieselDirectoryPublishLogRepository::new(executor, keys, profile);
        let before = publications
            .get_publish_record("entry", "new")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(before.phase, PublishPhase::Staging);
        assert!(before.root_map.is_empty());
        assert!(!before.landed);
        let roots = vec![(
            directory.path().join(".uniclip-incoming-entry/root"),
            directory.path().join("root"),
        )];
        publications
            .record_phase("entry", "new", PublishPhase::Staging, &roots, 11)
            .await
            .unwrap();
        assert_eq!(
            publications
                .get_publish_record("entry", "new")
                .await
                .unwrap()
                .unwrap()
                .root_map,
            roots
        );
    }
}

#[tokio::test]
async fn retry_projection_inherits_no_cleanup_paths_and_preserves_interrupted_and_timeout_reasons()
{
    use crate::db::models::FileTransferRow;
    use crate::db::schema::{file_transfer, file_transfer_events, receive_artifact_log};
    use uc_core::file_transfer::FileTransferCancellationReason;
    use uc_core::ports::{CancelDirectoryAttemptTransfersPort, ReceiveItemFailureReason};
    let (repo, executor, directory) = database().await;
    let saved = directory.path().join("saved.bin");
    std::fs::write(&saved, b"success").unwrap();
    let mut command = input();
    command.items[2].state = ReceiveRetryItemState::Failed(ReceiveItemFailureReason::Interrupted);
    repo.begin_receive_retry(command).await.unwrap();
    let rows = executor
        .run(|conn| {
            file_transfer::table
                .load::<FileTransferRow>(conn)
                .map_err(Into::into)
        })
        .unwrap();
    let protection = repo.protection.resolve().await.unwrap();
    for row in rows {
        assert!(!row
            .metadata_ciphertext
            .windows(b"PRIVATE_ORIGINAL_FILENAME".len())
            .any(|part| part == b"PRIVATE_ORIGINAL_FILENAME"));
        let metadata = protection
            .open_metadata(&row.transfer_id, &row.metadata_ciphertext)
            .await
            .unwrap();
        assert_eq!(metadata.filename, "PRIVATE_ORIGINAL_FILENAME");
        assert!(metadata.cached_path.is_none());
    }
    let counts = executor
        .run(|conn| {
            Ok((
                file_transfer_events::table
                    .count()
                    .get_result::<i64>(conn)?,
                receive_artifact_log::table
                    .count()
                    .get_result::<i64>(conn)?,
            ))
        })
        .unwrap();
    assert_eq!(counts, (0, 0));
    repo.cancel_attempt_transfers("entry", "new", FileTransferCancellationReason::Timeout, 20)
        .await
        .unwrap();
    executor
        .run(|conn| {
            diesel::update(entry_receive_attempt::table)
                .set(entry_receive_attempt::attempt_state.eq(AttemptState::Cancelled.to_string()))
                .execute(conn)?;
            Ok(())
        })
        .unwrap();
    let progress = repo
        .get_entry_receive_progress("entry")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        progress.items[0].status,
        TrackedFileTransferStatus::Completed
    );
    assert_eq!(
        progress.items[1].failure_reason,
        Some(ReceiveItemFailureReason::TimedOut)
    );
    assert_eq!(
        progress.items[2].failure_reason,
        Some(ReceiveItemFailureReason::Interrupted)
    );
    assert_eq!(std::fs::read(saved).unwrap(), b"success");
}

#[tokio::test]
async fn invalid_retry_plan_cannot_change_authority() {
    use uc_core::ports::GetEntryAttemptPort;
    for case in 0..4 {
        let (repo, executor, _directory) = database().await;
        let mut command = input();
        match case {
            0 => command.items[2].item_id = command.items[0].item_id.clone(),
            1 => command.attempt_id = command.expected_attempt_id.clone(),
            2 => command
                .items
                .iter_mut()
                .for_each(|item| item.state = ReceiveRetryItemState::Completed),
            _ => command.items[0].bytes = u64::MAX,
        }
        assert!(repo.begin_receive_retry(command).await.is_err());
        let current = DieselEntryReceiveAttemptRepository::new(executor)
            .get_entry_attempt("entry")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(current.current_attempt_id, "old");
        assert_eq!(current.state, AttemptState::Failed);
    }
}

#[tokio::test]
async fn downloaded_item_without_saved_record_is_not_delivered_after_settlement() {
    use crate::db::repositories::DieselInboundReceiveCommitRepository;
    use crate::db::schema::file_transfer;
    use uc_core::ids::EntryId;
    use uc_core::ports::{
        CommitInboundReceivePort, InboundReceiveSettlement, NoEntryReceiveArtifacts,
        PartialReceiveTerminal, ReceiveItemFailureReason,
    };
    let (repo, executor, _directory) = database().await;
    repo.begin_receive_retry(input()).await.unwrap();
    executor
        .run(|conn| {
            diesel::update(
                file_transfer::table
                    .filter(file_transfer::receive_item_id.eq("entry:attempt:new:member:1")),
            )
            .set(file_transfer::status.eq("completed"))
            .execute(conn)?;
            diesel::update(entry_receive_attempt::table)
                .set(entry_receive_attempt::attempt_state.eq("failing"))
                .execute(conn)?;
            Ok(())
        })
        .unwrap();
    let (keys, profile) = crate::file_transfer::persistence_cipher::test_keys::ports();
    DieselInboundReceiveCommitRepository::new(executor, keys, profile)
        .commit_inbound_receive(&InboundReceiveSettlement::NoEntry {
            entry_id: EntryId::from("entry"),
            attempt_id: "new".into(),
            terminal: PartialReceiveTerminal::Failed,
            artifacts: NoEntryReceiveArtifacts::None,
            now_ms: 20,
        })
        .await
        .unwrap();
    let progress = repo
        .get_entry_receive_progress("entry")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        progress.items[0].status,
        TrackedFileTransferStatus::Completed
    );
    assert_eq!(
        progress.items[1].status,
        TrackedFileTransferStatus::Failed,
        "只有下载完成，没有保存记录的项不能报告交付成功"
    );
    assert_eq!(
        progress.items[1].failure_reason,
        Some(ReceiveItemFailureReason::Interrupted)
    );
}
