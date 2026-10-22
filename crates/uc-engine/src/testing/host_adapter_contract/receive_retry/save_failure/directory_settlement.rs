//! 真实目录V3与SDK：成员网络终态未结算时父动作不能提前结束。
use super::*;
use crate::{HostClipboardRepresentation, ObserveClipboardChangeInput, ReceiveItemKind};

#[derive(QueryableByName)]
struct DirectoryAttempt {
    #[diesel(sql_type = Text)]
    attempt_state: String,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_directory_member_terminal_repair_keeps_parent_receiving_until_settled() {
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
    let mut connection = SqliteConnection::establish(
        generation
            .path()
            .join("v3-payloads/profile.sqlite")
            .to_str()
            .unwrap(),
    )
    .unwrap();
    // 触发器不支持参数，原条目标识按SQLite字面量规则转义；不阻塞旧接收的完成记录。
    let old_entry = case.first.entry_id.replace('\'', "''");
    diesel::sql_query(format!("CREATE TRIGGER reject_directory_terminal BEFORE UPDATE ON file_transfer WHEN NEW.entry_id != '{old_entry}' AND NEW.item_role = 'file' AND NEW.status = 'completed' BEGIN SELECT RAISE(ABORT, 'injected directory terminal write'); END")).execute(&mut connection).unwrap();
    let directory = case._source_root.path().join("directory-source");
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(directory.join("a.bin"), vec![0x61; 96 * 1024]).unwrap();
    std::fs::write(directory.join("b.bin"), vec![0x62; 96 * 1024]).unwrap();
    *case.clipboard.lock().unwrap() = HostClipboardSnapshot {
        observed_at_ms: 1_700_000_100_000,
        representations: vec![HostClipboardRepresentation::Inline {
            format: "files".into(),
            mime_type: Some("text/uri-list".into()),
            bytes: Url::from_directory_path(&directory)
                .unwrap()
                .to_string()
                .into_bytes(),
        }],
    };
    // 原fixture在第四次导出提供暂停能力；本例只阻塞SQLite终态交接。
    case.exports.release.add_permits(1);
    case.source
        .execute(Operation::ObserveClipboardChange(
            ObserveClipboardChangeInput { dispatch: true },
        ))
        .await
        .unwrap();
    let EngineEvent::IncomingPending(pending) = next_engine_event_matching(&mut case.events,
        |event| matches!(event, EngineEvent::IncomingPending(pending) if pending.entry_id != case.first.entry_id),
    ).await else { panic!("directory receive expected") };
    // 等待公开查询出现已知失败事实，不以等待父动作结束作为前置。
    let before = timeout(Duration::from_secs(20), async {
        loop {
            let OperationResult::EntryReceiveProgress(Some(current)) = case
                .receiver
                .execute(Operation::QueryEntryReceiveProgress(
                    EntryReceiveProgressInput {
                        entry_id: pending.entry_id.clone(),
                    },
                ))
                .await
                .unwrap()
            else {
                panic!("directory progress expected")
            };
            if current
                .items
                .iter()
                .any(|item| item.state == "failed" || item.failure_reason.is_some())
            {
                break current;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let rows = diesel::sql_query(
        "SELECT status, failure_code, binding_state FROM file_transfer WHERE entry_id = ?",
    )
    .bind::<Text, _>(&pending.entry_id)
    .load::<Transfer>(&mut connection)
    .unwrap();
    assert_eq!(
        rows.iter()
            .filter(|row| row.status == "transferring")
            .count(),
        1
    );
    assert!(rows
        .iter()
        .all(|row| ["transferring", "failed"].contains(&row.status.as_str())));

    let closing = case
        .receiver
        .shutdown(Duration::from_millis(100))
        .await
        .unwrap_err();
    assert_eq!(closing.category(), EngineErrorCategory::DeadlineExceeded);
    diesel::sql_query("DROP TRIGGER reject_directory_terminal")
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
    assert_eq!(
        rows.iter().filter(|row| row.status == "completed").count(),
        1,
        "原网络Completed意图不能被父失败覆盖"
    );
    assert!(rows
        .iter()
        .all(|row| ["completed", "failed", "cancelled"].contains(&row.status.as_str())));
    let settled =
        diesel::sql_query("SELECT attempt_state FROM entry_receive_attempt WHERE entry_id = ?")
            .bind::<Text, _>(&pending.entry_id)
            .get_result::<DirectoryAttempt>(&mut connection)
            .unwrap();
    assert!(["failed", "cancelled"].contains(&settled.attempt_state.as_str()));
    assert_eq!(
        case.exports.exports.lock().unwrap().len(),
        4,
        "本地恢复不重新导出旧文件，也不启动后续目录成员"
    );
    assert_eq!(
        before.state, "receiving",
        "原成员终态未落盘时，父目录不能提前终结"
    );
    assert_eq!(before.items.len(), 1);
    assert_eq!(before.items[0].kind, ReceiveItemKind::DirectoryUnit);
    assert_eq!(
        before.items[0].failure_reason,
        Some(ReceiveItemFailureReason::StorageUnavailable)
    );
    assert_eq!(before.items_completed, 0);
    assert_eq!(before.items_total, 2);

    assert!(
        !case.exports.paths.lock().unwrap()[3].exists(),
        "失败目录的原staging输出应实际清理"
    );
    for (bytes, path) in &case.saved {
        assert_eq!(
            std::fs::read(path).unwrap(),
            *bytes,
            "目录失败不能影响先前独立文件成果"
        );
    }
    drop(connection);
    case.shutdown().await;
}
