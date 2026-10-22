//! 真实导出后保存与物理清理同时失败，关闭必须等待原动作修复。
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_shutdown_waits_for_failed_save_output_cleanup() {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let mut case = receive_case_with_progress(true, false, usize::MAX).await;
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
    let mut connection = SqliteConnection::establish(database.to_str().unwrap()).unwrap();
    let failed_id = case.first.items[0].item_id.replace('\'', "''");
    diesel::sql_query(format!("CREATE TRIGGER reject_cleanup_pending_save BEFORE INSERT ON clipboard_event WHEN EXISTS (SELECT 1 FROM file_transfer WHERE receive_item_id = '{failed_id}' AND status = 'completed' AND failure_code IS NULL) BEGIN SELECT RAISE(ABORT, 'injected cleanup pending save'); END"))
        .execute(&mut connection).unwrap();
    let path = case.exports.paths.lock().unwrap()[0].clone();
    assert_eq!(std::fs::read(&path).unwrap(), CONTENTS[0]);
    // 原导出已完成，实际文件系统拒绝remove_file；不是fake清理返回。
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    case.exports.release.add_permits(1);
    timeout(Duration::from_secs(10), async {
        loop {
            let rows = diesel::sql_query("SELECT status, failure_code, binding_state FROM file_transfer WHERE entry_id = ? AND receive_item_id = ?")
                .bind::<Text,_>(entry).bind::<Text,_>(&case.first.items[0].item_id)
                .load::<Transfer>(&mut connection).unwrap();
            if rows.len() == 1 && rows[0].status == "completed" { break; }
            sleep(Duration::from_millis(20)).await;
        }
    }).await.unwrap();
    // 既有完整入口的结果通知证明保存失败已返回，避免关闭先取消保存而改变失败原因。
    let source = case.source_id.clone();
    next_engine_event_matching(
        &mut case.events,
        |event| matches!(event, EngineEvent::InboundNotice(notice) if notice.from_device == source),
    )
    .await;
    let pending = progress(&case.receiver, entry, "receiving").await;
    assert_eq!(pending.items_completed, 0);
    assert_eq!(pending.completed_bytes, 0);
    assert_eq!(pending.items_total, 3);
    assert!(path.is_dir());
    let error = case
        .receiver
        .shutdown(Duration::from_millis(100))
        .await
        .unwrap_err();
    assert_eq!(
        error.category(),
        EngineErrorCategory::DeadlineExceeded,
        "{error:?}"
    );
    let pending = attempt_settlement(&mut connection, entry);
    assert!(["receiving", "cancelling", "failing"].contains(&pending.attempt_state.as_str()));
    assert_eq!(pending.phase, "preparing");
    assert_eq!(pending.resolution, "pending");
    assert_eq!(case.exports.exports.lock().unwrap().len(), 1);
    assert!(path.is_dir());
    std::fs::remove_dir(&path).unwrap();
    timeout(
        Duration::from_secs(60),
        case.receiver.shutdown_until_complete(),
    )
    .await
    .unwrap()
    .unwrap();
    let mut reopened = SqliteConnection::establish(database.to_str().unwrap()).unwrap();
    let rows = diesel::sql_query("SELECT status, failure_code, binding_state FROM file_transfer WHERE entry_id = ? AND receive_item_id = ?")
        .bind::<Text,_>(entry).bind::<Text,_>(&case.first.items[0].item_id)
        .load::<Transfer>(&mut reopened).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, "completed");
    assert_eq!(rows[0].failure_code.as_deref(), Some("storage_unavailable"));
    assert_eq!(rows[0].binding_state, "attempt_bound");
    let settled = attempt_settlement(&mut reopened, entry);
    assert!(["failed", "cancelled"].contains(&settled.attempt_state.as_str()));
    assert_eq!(settled.phase, "settled");
    assert_eq!(settled.resolution, "rolled_back");
    assert!(!path.exists());
    assert_eq!(
        case.exports.exports.lock().unwrap().len(),
        1,
        "清理修复不能重新导出或启动后项"
    );
    diesel::sql_query("DROP TRIGGER reject_cleanup_pending_save")
        .execute(&mut reopened)
        .unwrap();
    drop(reopened);
    drop(connection);
    case.shutdown().await;
}
