//! 已登记Pending但Started事务失败时，原接收动作仍须补齐失败与清理责任。
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_started_write_failure_repairs_pending_item_without_starting_its_download() {
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
    let item = &case.first.items[1].item_id;
    let escaped = item.replace('\'', "''");
    diesel::sql_query(format!("CREATE TRIGGER reject_started_write BEFORE UPDATE ON file_transfer WHEN NEW.receive_item_id = '{escaped}' AND NEW.status = 'transferring' BEGIN SELECT RAISE(ABORT, 'injected started write'); END"))
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
    assert_eq!(
        case.exports.exports.lock().unwrap().len(),
        1,
        "准备失败不能开始第二项数据获取"
    );
    let saved = saved_files(&case.receiver, entry, case.receiver_root.path()).await;
    assert_eq!(saved.len(), 1);
    let original = saved.get(CONTENTS[0]).unwrap().clone();
    let rows = diesel::sql_query("SELECT status, failure_code, binding_state FROM file_transfer WHERE entry_id = ? AND receive_item_id = ?")
        .bind::<Text,_>(entry).bind::<Text,_>(item).load::<Transfer>(&mut connection).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, "pending");
    let waiting = case
        .receiver
        .shutdown(Duration::from_millis(100))
        .await
        .unwrap_err();
    assert_eq!(waiting.category(), EngineErrorCategory::DeadlineExceeded);
    diesel::sql_query("DROP TRIGGER reject_started_write")
        .execute(&mut connection)
        .unwrap();
    timeout(Duration::from_secs(10), async {
        loop {
            let rows = diesel::sql_query("SELECT status, failure_code, binding_state FROM file_transfer WHERE entry_id = ? AND receive_item_id = ?")
                .bind::<Text,_>(entry).bind::<Text,_>(item).load::<Transfer>(&mut connection).unwrap();
            if rows.len() == 1 && rows[0].status == "failed" {
                assert_eq!(rows[0].failure_code.as_deref(), Some("storage_unavailable"));
                break;
            }
            sleep(Duration::from_millis(20)).await;
        }
    }).await.expect("Started拒绝解除后原负责人必须补记已登记项的失败，不能遗留Pending");
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
    assert_eq!(
        std::fs::read_dir(original.parent().unwrap())
            .unwrap()
            .count(),
        1
    );
    assert_eq!(
        case.exports.exports.lock().unwrap().len(),
        1,
        "本地修复不能重新获取或启动后项"
    );
    drop(connection);
    case.shutdown().await;
}
