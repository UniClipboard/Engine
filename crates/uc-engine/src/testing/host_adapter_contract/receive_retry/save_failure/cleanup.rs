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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_failed_export_cleanup_keeps_network_failure_and_prior_delivery() {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let mut case = receive_case(true).await;
    let _release = ReleaseOnDrop(case.exports.clone());
    let entry = &case.first.entry_id;
    case.exports
        .obstruct_failure
        .store(true, std::sync::atomic::Ordering::SeqCst);
    case.exports.release.add_permits(1);
    let source = case.source_id.clone();
    next_engine_event_matching(
        &mut case.events,
        |event| matches!(event, EngineEvent::InboundNotice(notice) if notice.from_device == source),
    )
    .await;
    let pending = progress(&case.receiver, entry, "receiving").await;
    assert_eq!(
        pending
            .items
            .iter()
            .map(|item| item.state.as_str())
            .collect::<Vec<_>>(),
        ["completed", "failed", "pending"]
    );
    assert_eq!(pending.items_completed, 1);
    assert_eq!(pending.items_total, 3);
    assert_eq!(
        pending.items[1].failure_reason,
        Some(ReceiveItemFailureReason::StorageUnavailable)
    );
    let first = saved_files(&case.receiver, entry, case.receiver_root.path()).await;
    assert_eq!(first.len(), 1);
    let original = first.get(CONTENTS[0]).unwrap().clone();
    let path = case.exports.paths.lock().unwrap()[1].clone();
    assert!(path.is_dir());
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
    let pending = attempt_settlement(&mut connection, entry);
    assert_eq!(pending.phase, "preparing");
    assert_eq!(pending.resolution, "pending");
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
    assert!(path.is_dir());
    assert_eq!(std::fs::read(&original).unwrap(), CONTENTS[0]);
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
        .bind::<Text,_>(entry).bind::<Text,_>(&case.first.items[1].item_id)
        .load::<Transfer>(&mut reopened).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].status, "failed",
        "原网络失败不能硬写为网络Completed后的失败事实"
    );
    assert_eq!(rows[0].failure_code.as_deref(), Some("storage_unavailable"));
    assert_eq!(rows[0].binding_state, "attempt_bound");
    let settled = attempt_settlement(&mut reopened, entry);
    assert!(["failed", "cancelled"].contains(&settled.attempt_state.as_str()));
    assert_eq!(settled.phase, "settled");
    assert_eq!(settled.resolution, "rolled_back");
    assert!(!path.exists());
    assert_eq!(std::fs::read(&original).unwrap(), CONTENTS[0]);
    assert_eq!(case.exports.exports.lock().unwrap().len(), 2);
    drop(reopened);
    drop(connection);
    case.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_failed_file_registration_cleans_prepared_output_and_delivers_other_files() {
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
    let failed_id = case.first.items[1].item_id.replace('\'', "''");
    // 首项实际导出仍暂停，第二项目标准备和网络登记均尚未开始。
    diesel::sql_query(format!("CREATE TRIGGER reject_file_registration BEFORE INSERT ON file_transfer WHEN NEW.receive_item_id = '{failed_id}' OR NEW.transfer_id = '{failed_id}' BEGIN SELECT RAISE(ABORT, 'injected file registration failure'); END"))
        .execute(&mut connection).unwrap();
    case.exports.release.add_permits(1);
    let source = case.source_id.clone();
    next_engine_event_matching(
        &mut case.events,
        |event| matches!(event, EngineEvent::InboundNotice(notice) if notice.from_device == source),
    )
    .await;
    let OperationResult::EntryReceiveProgress(Some(failed)) = case
        .receiver
        .execute(Operation::QueryEntryReceiveProgress(
            EntryReceiveProgressInput {
                entry_id: entry.clone(),
            },
        ))
        .await
        .unwrap()
    else {
        panic!("receive progress expected")
    };
    assert_eq!(
        failed.state, "failed",
        "登记失败不能把原清理任务永久留在接收中"
    );
    assert_eq!(
        failed
            .items
            .iter()
            .map(|item| item.state.as_str())
            .collect::<Vec<_>>(),
        ["completed", "failed", "completed"]
    );
    assert_eq!(
        failed.items[1].failure_reason,
        Some(ReceiveItemFailureReason::StorageUnavailable)
    );
    assert_eq!(failed.items_completed, 2);
    assert_eq!(failed.items_total, 3);
    let files = saved_files(&case.receiver, entry, case.receiver_root.path()).await;
    assert_eq!(files.len(), 2);
    assert!(files.contains_key(CONTENTS[0]) && files.contains_key(CONTENTS[2]));
    let parent = files.get(CONTENTS[0]).unwrap().parent().unwrap();
    assert!(files.values().all(|path| path.parent() == Some(parent)));
    assert_eq!(
        std::fs::read_dir(parent).unwrap().count(),
        2,
        "失败登记的空输出必须实际清理"
    );
    let rows = diesel::sql_query("SELECT status, failure_code, binding_state FROM file_transfer WHERE entry_id = ? AND receive_item_id = ?")
        .bind::<Text,_>(entry).bind::<Text,_>(&case.first.items[1].item_id)
        .load::<Transfer>(&mut connection).unwrap();
    assert!(
        rows.is_empty(),
        "尚未成功的网络登记不能补造成已失败的网络文件"
    );
    let settled = attempt_settlement(&mut connection, entry);
    assert_eq!(settled.attempt_state, "failed");
    assert_eq!(settled.phase, "settled");
    assert_eq!(settled.resolution, "landed");
    assert_eq!(
        case.exports.exports.lock().unwrap().len(),
        2,
        "第二项不导出，第三项仍正常交付"
    );
    diesel::sql_query("DROP TRIGGER reject_file_registration")
        .execute(&mut connection)
        .unwrap();
    let OperationResult::EntryReceiveRetry(EntryReceiveRetryOutcome::Accepted { .. }) = case
        .receiver
        .execute(Operation::RetryEntryReceive(RetryEntryReceiveInput {
            entry_id: entry.clone(),
            expected_attempt_id: failed.attempt_id,
            item_ids: vec![failed.items[1].item_id.clone()],
        }))
        .await
        .unwrap()
    else {
        panic!("registration failure must remain retryable")
    };
    progress(&case.receiver, entry, "completed").await;
    let final_files = saved_files(&case.receiver, entry, case.receiver_root.path()).await;
    assert_eq!(final_files.len(), 3);
    for index in [0, 2] {
        assert_eq!(
            final_files.get(CONTENTS[index]),
            files.get(CONTENTS[index]),
            "原成功输出不能被替换或重新获取"
        );
    }
    assert_eq!(
        case.exports.exports.lock().unwrap().len(),
        3,
        "仅补取未登记的原失败项"
    );
    drop(connection);
    case.shutdown().await;
}
