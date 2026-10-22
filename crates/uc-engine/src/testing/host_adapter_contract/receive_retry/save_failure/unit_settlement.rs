//! 表示blob的底层终态仍在本地修复时，父接收不能提前声明已经结束。
use super::*;
use crate::SendImageInput;

#[derive(QueryableByName)]
struct UnitAttempt {
    #[diesel(sql_type = Text)]
    attempt_state: String,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_representation_terminal_repair_keeps_parent_receiving_until_settled() {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let mut case = receive_case(false).await;
    let generation = std::fs::read_dir(
        case.receiver_root
            .path()
            .join("private/profile-data-generations"),
    )
    .unwrap()
    .map(|entry| entry.unwrap())
    .find(|entry| entry.file_type().unwrap().is_dir())
    .unwrap();
    let database = generation.path().join("v3-payloads/profile.sqlite");
    let mut connection = SqliteConnection::establish(database.to_str().unwrap()).unwrap();
    diesel::sql_query("CREATE TRIGGER reject_representation_terminal BEFORE UPDATE ON file_transfer WHEN NEW.item_role = 'representation' AND NEW.status = 'completed' BEGIN SELECT RAISE(ABORT, 'injected representation terminal write'); END")
        .execute(&mut connection).unwrap();
    let receiver_id = member_id(&case.receiver).await;
    let OperationResult::EntrySent(report) = case
        .source
        .execute(Operation::SendImage(SendImageInput {
            bytes: vec![0x55; 96 * 1024],
            mime_type: "image/png".into(),
            target_devices: vec![receiver_id],
        }))
        .await
        .unwrap()
    else {
        panic!("image send expected")
    };
    assert_eq!(
        report.total_accepted + report.total_pending + report.total_errored,
        1
    );
    let EngineEvent::IncomingPending(pending) = next_engine_event_matching(&mut case.events,
        |event| matches!(event, EngineEvent::IncomingPending(pending) if pending.entry_id != case.first.entry_id),
    ).await else { panic!("representation receive expected") };
    let OperationResult::EntryReceiveProgress(Some(before)) = case
        .receiver
        .execute(Operation::QueryEntryReceiveProgress(
            EntryReceiveProgressInput {
                entry_id: pending.entry_id.clone(),
            },
        ))
        .await
        .unwrap()
    else {
        panic!("representation progress expected")
    };
    let rows = diesel::sql_query(
        "SELECT status, failure_code, binding_state FROM file_transfer WHERE entry_id = ?",
    )
    .bind::<Text, _>(&pending.entry_id)
    .load::<Transfer>(&mut connection)
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, "transferring");
    let closing = case
        .receiver
        .shutdown(Duration::from_millis(100))
        .await
        .unwrap_err();
    assert_eq!(closing.category(), EngineErrorCategory::DeadlineExceeded);
    diesel::sql_query("DROP TRIGGER reject_representation_terminal")
        .execute(&mut connection)
        .unwrap();
    timeout(
        Duration::from_secs(60),
        case.receiver.shutdown_until_complete(),
    )
    .await
    .unwrap()
    .unwrap();
    let rows = diesel::sql_query(
        "SELECT status, failure_code, binding_state FROM file_transfer WHERE entry_id = ?",
    )
    .bind::<Text, _>(&pending.entry_id)
    .load::<Transfer>(&mut connection)
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, "completed", "原网络事实必须保留");
    let settled =
        diesel::sql_query("SELECT attempt_state FROM entry_receive_attempt WHERE entry_id = ?")
            .bind::<Text, _>(&pending.entry_id)
            .get_result::<UnitAttempt>(&mut connection)
            .unwrap();
    assert!(["failed", "cancelled"].contains(&settled.attempt_state.as_str()));
    assert_eq!(
        case.exports.exports.lock().unwrap().len(),
        3,
        "本地恢复不能重新导出先前成功文件"
    );
    assert_eq!(before.items.len(), 1);
    assert_eq!(before.items[0].state, "failed");
    assert_eq!(
        before.items[0].failure_reason,
        Some(ReceiveItemFailureReason::StorageUnavailable),
        "等待原终态结算时仍须保留已知本地失败分类",
    );
    assert_eq!(before.items_completed, 0);
    assert_eq!(before.items_total, 1);
    assert_eq!(
        before.state, "receiving",
        "原终态尚未落盘，父尝试不能提前报告Failed／Cancelled"
    );
    drop(connection);
    case.shutdown().await;
}
