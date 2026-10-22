//! 实际Engine／网络／V3／SQLite：各位置保存失败，保留其他成果且只重试原失败项。
use super::*;
use diesel::sql_types::Nullable;

#[derive(QueryableByName)]
struct Transfer {
    #[diesel(sql_type = Text)]
    status: String,
    #[diesel(sql_type = Nullable<Text>)]
    failure_code: Option<String>,
    #[diesel(sql_type = Text)]
    binding_state: String,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_first_save_failure_does_not_prevent_later_files_from_being_delivered() {
    assert_save_failure_at(0).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_post_download_save_failure_is_persisted_and_retry_preserves_prior_delivery() {
    assert_save_failure_at(1).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_last_save_failure_never_erases_previously_delivered_files() {
    assert_save_failure_at(2).await;
}

async fn assert_save_failure_at(index: usize) {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let case = receive_case_with_progress(true, false, usize::MAX).await;
    let _release = ReleaseOnDrop(case.exports.clone());
    let receiver = &case.receiver;
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
    let mut connection = SqliteConnection::establish(
        generation
            .path()
            .join("v3-payloads/profile.sqlite")
            .to_str()
            .unwrap(),
    )
    .unwrap();
    // 首项仍暂停在实际导出结束处；后两项尚未开始，故障先安装再释放。
    // 触发器不支持绑定参数；标识来自本例的稳定查询并按SQLite字面量规则转义。
    let failed_id = case.first.items[index].item_id.replace('\'', "''");
    diesel::sql_query(format!("CREATE TRIGGER reject_unsettled_file_save BEFORE INSERT ON clipboard_event WHEN EXISTS (SELECT 1 FROM file_transfer WHERE receive_item_id = '{failed_id}' AND status = 'completed' AND failure_code IS NULL) BEGIN SELECT RAISE(ABORT, 'injected post-download save failure'); END"))
        .execute(&mut connection).unwrap();
    case.exports.release.add_permits(1);

    let failed = progress(receiver, entry, "failed").await;
    let mut expected = ["completed"; 3];
    expected[index] = "failed";
    assert_eq!(
        failed
            .items
            .iter()
            .map(|item| item.state.as_str())
            .collect::<Vec<_>>(),
        expected
    );
    assert_eq!(
        failed.items[index].failure_reason,
        Some(ReceiveItemFailureReason::StorageUnavailable)
    );
    assert_eq!(failed.items_completed, 2);
    let saved = saved_files(receiver, entry, case.receiver_root.path()).await;
    assert_eq!(saved.len(), 2);
    for (position, bytes) in CONTENTS.iter().enumerate() {
        assert_eq!(saved.contains_key(*bytes), position != index);
    }
    let exports = case.exports.exports.lock().unwrap().clone();
    let paths = case.exports.paths.lock().unwrap().clone();
    assert_eq!(exports.len(), 3);
    assert_eq!(paths.len(), 3);
    assert!(!paths[index].exists(), "保存失败只清理本项刚导出的输出");

    // 重新打开同一真实数据库，核对完整流程写出的事实，避免内存receipt遮住持久结果。
    let mut reopened = SqliteConnection::establish(
        generation
            .path()
            .join("v3-payloads/profile.sqlite")
            .to_str()
            .unwrap(),
    )
    .unwrap();
    let rows = diesel::sql_query("SELECT status, failure_code, binding_state FROM file_transfer WHERE entry_id = ? ORDER BY receive_item_id")
        .bind::<Text,_>(entry).load::<Transfer>(&mut reopened).unwrap();
    assert_eq!(rows.len(), 3);
    for (position, row) in rows.iter().enumerate() {
        assert_eq!(row.status, "completed", "原网络终态不得回退");
        assert_eq!(
            row.failure_code.as_deref(),
            (position == index).then_some("storage_unavailable")
        );
        assert_eq!(
            row.binding_state,
            if position == index {
                "attempt_bound"
            } else {
                "entry_bound"
            }
        );
    }
    diesel::sql_query("DROP TRIGGER reject_unsettled_file_save")
        .execute(&mut connection)
        .unwrap();

    let OperationResult::EntryReceiveRetry(EntryReceiveRetryOutcome::Accepted { attempt_id }) =
        receiver
            .execute(Operation::RetryEntryReceive(RetryEntryReceiveInput {
                entry_id: entry.clone(),
                expected_attempt_id: failed.attempt_id.clone(),
                item_ids: vec![failed.items[index].item_id.clone()],
            }))
            .await
            .unwrap()
    else {
        panic!("failed file retry must be accepted")
    };
    let retried = progress(receiver, entry, "completed").await;
    assert_eq!(retried.attempt_id, attempt_id);
    assert_eq!(
        retried
            .items
            .iter()
            .map(|item| item.state.as_str())
            .collect::<Vec<_>>(),
        ["completed", "completed", "completed"]
    );
    let final_files = saved_files(receiver, entry, case.receiver_root.path()).await;
    assert_eq!(final_files.len(), 3);
    for (bytes, path) in &saved {
        assert_eq!(final_files.get(bytes), Some(path));
        assert_eq!(&std::fs::read(path).unwrap(), bytes);
    }
    for bytes in CONTENTS {
        assert!(final_files.contains_key(bytes));
    }
    let retried_exports = case.exports.exports.lock().unwrap().clone();
    assert_eq!(retried_exports.len(), 4);
    assert_eq!(
        retried_exports[3], exports[index],
        "只重新物化原失败来源，不能下载当前另一份内容"
    );
    assert_eq!(case.exports.protects.lock().unwrap().len(), 4);
    drop(reopened);
    drop(connection);
    case.shutdown().await;
}
