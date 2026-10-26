//! 原生发布后的失败责任必须写入真实SQLite，重启只清理原尝试输出。
use super::{DirectoryProbe, Engine, EntryReceiveProgressInput, Operation, OperationResult};
use diesel::sql_types::{BigInt, Integer, Text};
use diesel::{Connection, QueryableByName, RunQueryDsl, SqliteConnection};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::time::{sleep, timeout};

#[derive(QueryableByName)]
struct Journal {
    #[diesel(sql_type = Text)]
    phase: String,
    #[diesel(sql_type = Integer)]
    partial_publication: i32,
    #[diesel(sql_type = Integer)]
    partial_root_count: i32,
    #[diesel(sql_type = Integer)]
    landed: i32,
    #[diesel(sql_type = Text)]
    attempt_state: String,
    #[diesel(sql_type = BigInt)]
    unsettled_artifacts: i64,
    #[diesel(sql_type = BigInt)]
    saved_entries: i64,
}

fn journal(root: &Path, entry_id: &str) -> Option<Journal> {
    let generation = std::fs::read_dir(root.join("private/profile-data-generations"))
        .unwrap()
        .map(|entry| entry.unwrap())
        .find(|entry| entry.file_type().unwrap().is_dir())
        .unwrap();
    let database = generation.path().join("v3-payloads/profile.sqlite");
    let mut connection = SqliteConnection::establish(database.to_str().unwrap()).unwrap();
    diesel::sql_query("PRAGMA busy_timeout = 5000")
        .execute(&mut connection)
        .unwrap();
    let rows = diesel::sql_query(
        "SELECT d.phase, d.partial_publication, d.partial_root_count, d.landed, a.attempt_state, \
         (SELECT COUNT(*) FROM receive_artifact_log r WHERE r.entry_id = d.entry_id AND r.attempt_id = d.attempt_id AND r.resolution = 'pending') AS unsettled_artifacts, \
         (SELECT COUNT(*) FROM clipboard_entry e WHERE e.entry_id = d.entry_id) AS saved_entries \
         FROM directory_publish_log d JOIN entry_receive_attempt a \
         ON a.entry_id = d.entry_id AND a.current_attempt_id = d.attempt_id WHERE d.entry_id = ?",
    )
    .bind::<Text, _>(entry_id)
    .load::<Journal>(&mut connection)
    .unwrap();
    assert!(rows.len() <= 1);
    rows.into_iter().next()
}

pub(super) async fn wait_for_failed_rollback(
    engine: &Engine,
    root: &Path,
    destination: &Path,
    entry_id: &str,
    fixture: &DirectoryProbe,
) {
    timeout(Duration::from_secs(20), async {
        loop {
            if journal(root, entry_id)
                .is_some_and(|row| row.partial_publication == 1 && row.partial_root_count == 1)
            {
                break;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("failed withdrawal must durably retain one visible root");
    assert_eq!(fixture.published.load(Ordering::SeqCst), 2);
    assert!(fixture.withdrawals.load(Ordering::SeqCst) >= 2);
    assert_eq!(fixture.withdrawn.load(Ordering::SeqCst), 1);
    assert!(fixture.supported.load(Ordering::SeqCst));
    assert_eq!(
        std::fs::read(destination.join("directory-source/a.bin")).unwrap(),
        vec![0x61; 96 * 1024]
    );
    assert_eq!(
        std::fs::read(destination.join("directory-source/b.bin")).unwrap(),
        vec![0x62; 96 * 1024]
    );
    assert!(!destination.join("directory-other").exists());
    assert_journal(root, entry_id, false);
    let OperationResult::EntryReceiveProgress(Some(progress)) = engine
        .execute(Operation::QueryEntryReceiveProgress(
            EntryReceiveProgressInput {
                entry_id: entry_id.into(),
            },
        ))
        .await
        .unwrap()
    else {
        panic!("original receive progress expected")
    };
    assert_eq!(progress.state, "failing", "rollback still has an owner");
}

pub(super) fn assert_journal(root: &Path, entry_id: &str, recovered: bool) {
    let row = journal(root, entry_id).expect("original directory journal must remain available");
    assert_eq!(row.phase, "publishing");
    assert_eq!(row.landed, 0);
    assert_eq!(
        row.saved_entries, 0,
        "partial directory must never be delivered"
    );
    assert_eq!(row.partial_root_count, if recovered { 0 } else { 1 });
    // 此标志表示已记录撤回检查点，零残留仍保留该证明。
    assert_eq!(row.partial_publication, 1);
    if recovered {
        assert_eq!(row.attempt_state, "failed");
        assert_eq!(row.unsettled_artifacts, 0);
    } else {
        assert!(matches!(
            row.attempt_state.as_str(),
            "committing" | "failing"
        ));
    }
}
