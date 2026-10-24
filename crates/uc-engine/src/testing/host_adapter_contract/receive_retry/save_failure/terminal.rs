//! 真实网络失败已经发生，但原网络终态的SQLite提交仍被拒绝时必须继续修复。
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_failed_network_terminal_write_repairs_before_output_settlement_and_shutdown() {
    assert_terminal_repair(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_completed_network_terminal_write_repairs_without_rewriting_network_as_failed()
{
    assert_terminal_repair(true).await;
}

async fn assert_terminal_repair(completed: bool) {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let mut case =
        receive_case_with_progress(true, false, if completed { usize::MAX } else { 1 }).await;
    let _release = ReleaseOnDrop(case.exports.clone());
    let entry = &case.first.entry_id;
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
    let mut connection = open_control_connection(&database);
    let failed_id = case.first.items[1].item_id.replace('\'', "''");
    let expected_status = if completed { "completed" } else { "failed" };
    diesel::sql_query(format!("CREATE TRIGGER reject_network_terminal BEFORE UPDATE ON file_transfer WHEN NEW.receive_item_id = '{failed_id}' AND NEW.status = '{expected_status}' BEGIN SELECT RAISE(ABORT, 'injected network terminal write'); END"))
        .execute(&mut connection).unwrap();
    case.exports.release.add_permits(1);
    let source = case.source_id.clone();
    next_engine_event_matching(
        &mut case.events,
        |event| matches!(event, EngineEvent::InboundNotice(notice) if notice.from_device == source),
    )
    .await;
    let pending = progress(&case.receiver, entry, "receiving").await;
    assert_eq!(pending.items_completed, 1);
    assert_eq!(pending.items_total, 3);
    assert_eq!(case.exports.exports.lock().unwrap().len(), 2);
    let first = saved_files(&case.receiver, entry, case.receiver_root.path()).await;
    assert_eq!(first.len(), 1);
    let original = first.get(CONTENTS[0]).unwrap().clone();
    let path = case.exports.paths.lock().unwrap()[1].clone();
    assert!(!path.exists());
    let rows = diesel::sql_query("SELECT status, failure_code, binding_state FROM file_transfer WHERE entry_id = ? AND receive_item_id = ?")
        .bind::<Text,_>(entry).bind::<Text,_>(&case.first.items[1].item_id)
        .load::<Transfer>(&mut connection).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, "transferring");
    let closing = case
        .receiver
        .shutdown(Duration::from_millis(100))
        .await
        .unwrap_err();
    assert_eq!(closing.category(), EngineErrorCategory::DeadlineExceeded);
    diesel::sql_query("DROP TRIGGER reject_network_terminal")
        .execute(&mut connection)
        .unwrap();
    timeout(Duration::from_secs(10), async {
        loop {
            let rows = diesel::sql_query("SELECT status, failure_code, binding_state FROM file_transfer WHERE entry_id = ? AND receive_item_id = ?")
                .bind::<Text,_>(entry).bind::<Text,_>(&case.first.items[1].item_id)
                .load::<Transfer>(&mut connection).unwrap();
            if rows.len() == 1 && rows[0].status == expected_status
                && rows[0].failure_code.as_deref() == Some("storage_unavailable") {
                assert_eq!(rows[0].failure_code.as_deref(), Some("storage_unavailable"));
                break;
            }
            sleep(Duration::from_millis(20)).await;
        }
    }).await.expect("原网络负责人必须在解除实际存储拒绝后补记原失败，不能永久遗留活动事实");
    timeout(
        Duration::from_secs(60),
        case.receiver.shutdown_until_complete(),
    )
    .await
    .unwrap()
    .unwrap();
    let settled = attempt_settlement(&mut connection, entry);
    assert!(["failed", "cancelled"].contains(&settled.attempt_state.as_str()));
    assert_eq!(settled.phase, "settled");
    assert_eq!(settled.resolution, "rolled_back");
    assert_eq!(std::fs::read(&original).unwrap(), CONTENTS[0]);
    assert!(!path.exists());
    assert_eq!(
        case.exports.exports.lock().unwrap().len(),
        2,
        "终态修复不能重新导出或启动后项"
    );
    drop(connection);
    case.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_cancelled_network_terminal_write_stays_cancelled_until_actual_settlement() {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let mut case = receive_case(true).await;
    let _release = ReleaseOnDrop(case.exports.clone());
    let entry = &case.first.entry_id;
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
    let mut connection = open_control_connection(&database);
    let item = &case.first.items[0].item_id;
    let escaped = item.replace('\'', "''");
    diesel::sql_query(format!("CREATE TRIGGER reject_cancel_terminal BEFORE UPDATE ON file_transfer WHEN NEW.receive_item_id = '{escaped}' AND NEW.status = 'cancelled' BEGIN SELECT RAISE(ABORT, 'injected cancelled terminal write'); END"))
        .execute(&mut connection).unwrap();
    assert!(matches!(
        case.receiver
            .execute(Operation::CancelEntryReceive(CancelEntryReceiveInput {
                entry_id: entry.clone(),
                attempt_id: case.first.attempt_id.clone(),
            }))
            .await
            .unwrap(),
        OperationResult::EntryReceiveCancellation(
            EntryReceiveCancellationOutcome::CancellationIncomplete
        )
    ));
    case.exports.release.add_permits(1);
    let source = case.source_id.clone();
    next_engine_event_matching(
        &mut case.events,
        |event| matches!(event, EngineEvent::InboundNotice(notice) if notice.from_device == source),
    )
    .await;
    let pending = progress(&case.receiver, entry, "cancelling").await;
    assert_eq!(pending.items_completed, 0);
    assert_eq!(pending.items_total, 3);
    assert_eq!(case.exports.exports.lock().unwrap().len(), 1);
    let path = case.exports.paths.lock().unwrap()[0].clone();
    assert!(!path.exists());
    let rows = diesel::sql_query("SELECT status, failure_code, binding_state FROM file_transfer WHERE entry_id = ? AND receive_item_id = ?")
        .bind::<Text,_>(entry).bind::<Text,_>(item).load::<Transfer>(&mut connection).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, "transferring");
    let waiting = case
        .receiver
        .shutdown(Duration::from_millis(100))
        .await
        .unwrap_err();
    assert_eq!(waiting.category(), EngineErrorCategory::DeadlineExceeded);
    diesel::sql_query("DROP TRIGGER reject_cancel_terminal")
        .execute(&mut connection)
        .unwrap();
    timeout(
        Duration::from_secs(60),
        case.receiver.shutdown_until_complete(),
    )
    .await
    .unwrap()
    .unwrap();
    let rows = diesel::sql_query("SELECT status, failure_code, binding_state FROM file_transfer WHERE entry_id = ? AND receive_item_id = ?")
        .bind::<Text,_>(entry).bind::<Text,_>(item).load::<Transfer>(&mut connection).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, "cancelled");
    assert_eq!(rows[0].failure_code.as_deref(), Some("local_user"));
    let settled = attempt_settlement(&mut connection, entry);
    assert_eq!(settled.attempt_state, "cancelled");
    assert_eq!(settled.phase, "settled");
    assert_eq!(settled.resolution, "rolled_back");
    assert!(!path.exists());
    assert_eq!(case.exports.exports.lock().unwrap().len(), 1);
    drop(connection);
    case.shutdown().await;
}
