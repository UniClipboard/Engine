use super::*;
use uc_application::deps::{
    BeginReceiveRetryInput, BeginReceiveRetryPort, ReceiveRetryItem, ReceiveRetryItemState,
};
use uc_core::ports::{
    GetEntryReceiveProgressPort, ReceiveItemFailureReason, ReceiveItemRole,
    TrackedFileTransferStatus,
};
use uc_infra_storage::db::repositories::DieselFileTransferRepository;
use uc_infra_storage::db::schema::file_transfer;

#[tokio::test]
async fn saved_item_proof_and_record_commit_together_and_survive_reopen() {
    let directory = tempdir().unwrap();
    let database = directory.path().join("saved-items.sqlite");
    let executor = DieselSqliteExecutor::new(init_db_pool(database.to_str().unwrap()).unwrap());
    let subkey: Arc<dyn DeriveSpaceSubkeyPort> = Arc::new(FixedSubkey);
    let profile: Arc<dyn CurrentProfilePort> = Arc::new(FixedProfile);
    let attempts = DieselEntryReceiveAttemptRepository::new(executor.clone());
    attempts
        .begin_first_receive("entry", "a1", 1)
        .await
        .unwrap();
    attempts
        .begin_receive_failure("entry", "a1", 2)
        .await
        .unwrap();
    executor
        .run(|conn| {
            diesel::update(entry_receive_attempt::table)
                .set(entry_receive_attempt::attempt_state.eq("failed"))
                .execute(conn)?;
            Ok(())
        })
        .unwrap();
    let transfers =
        DieselFileTransferRepository::new(executor.clone(), subkey.clone(), profile.clone());
    let ids: Vec<String> = (0..4)
        .map(|index| format!("entry:attempt:a2:member:{index}"))
        .collect();
    transfers
        .begin_receive_retry(BeginReceiveRetryInput {
            entry_id: "entry".into(),
            expected_attempt_id: "a1".into(),
            attempt_id: "a2".into(),
            source_device_id: "peer".into(),
            directory: false,
            now_ms: 3,
            items: ids
                .iter()
                .enumerate()
                .map(|(index, id)| ReceiveRetryItem {
                    item_id: id.clone(),
                    role: ReceiveItemRole::File,
                    filename: "PRIVATE_FILENAME".into(),
                    bytes: 5,
                    state: match index {
                        0 => ReceiveRetryItemState::Completed,
                        1 | 2 => ReceiveRetryItemState::Pending,
                        _ => ReceiveRetryItemState::Failed(
                            ReceiveItemFailureReason::NetworkUnavailable,
                        ),
                    },
                })
                .collect(),
        })
        .await
        .unwrap();
    let downloaded = vec![ids[1].clone(), ids[2].clone()];
    executor
        .run(move |conn| {
            diesel::update(
                file_transfer::table.filter(file_transfer::receive_item_id.eq_any(downloaded)),
            )
            .set(file_transfer::status.eq("completed"))
            .execute(conn)?;
            Ok(())
        })
        .unwrap();
    attempts
        .begin_receive_failure("entry", "a2", 4)
        .await
        .unwrap();
    let commit = DieselInboundReceiveCommitRepository::new(
        executor.clone(),
        subkey.clone(),
        profile.clone(),
    );
    let settlement = |delivered_item_ids| InboundReceiveSettlement::Partial {
        record: record("entry"),
        attempt_id: "a2".into(),
        delivered_item_ids,
        terminal: PartialReceiveTerminal::Failed,
        file_set: None,
        artifacts: PartialReceiveArtifacts::None,
        now_ms: 5,
    };
    let error = commit
        .commit_inbound_receive(&settlement(vec![ids[1].clone(), "foreign-item".into()]))
        .await
        .unwrap_err();
    assert!(std::error::Error::source(&error).is_some());
    let selected_id = ids[1].clone();
    executor
        .run(move |conn| {
            assert_eq!(clipboard_entry::table.count().get_result::<i64>(conn)?, 0);
            assert_eq!(
                entry_receive_attempt::table
                    .select(entry_receive_attempt::attempt_state)
                    .first::<String>(conn)?,
                "failing"
            );
            assert_eq!(
                file_transfer::table
                    .filter(file_transfer::receive_item_id.eq(selected_id))
                    .select(file_transfer::binding_state)
                    .first::<String>(conn)?,
                "attempt_bound"
            );
            Ok(())
        })
        .unwrap();
    commit
        .commit_inbound_receive(&settlement(vec![ids[0].clone(), ids[1].clone()]))
        .await
        .unwrap();
    drop(commit);
    drop(transfers);
    let reopened = DieselFileTransferRepository::new(
        DieselSqliteExecutor::new(init_db_pool(database.to_str().unwrap()).unwrap()),
        subkey,
        profile,
    );
    let progress = reopened
        .get_entry_receive_progress("entry")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        progress
            .items
            .iter()
            .map(|item| item.status)
            .collect::<Vec<_>>(),
        vec![
            TrackedFileTransferStatus::Completed,
            TrackedFileTransferStatus::Completed,
            TrackedFileTransferStatus::Failed,
            TrackedFileTransferStatus::Failed,
        ]
    );
    assert_eq!(
        progress.items[2].failure_reason,
        Some(ReceiveItemFailureReason::Interrupted)
    );
    assert_eq!(
        progress.items[3].failure_reason,
        Some(ReceiveItemFailureReason::NetworkUnavailable)
    );
    executor
        .run(|conn| {
            assert_eq!(clipboard_entry::table.count().get_result::<i64>(conn)?, 1);
            Ok(())
        })
        .unwrap();
}
