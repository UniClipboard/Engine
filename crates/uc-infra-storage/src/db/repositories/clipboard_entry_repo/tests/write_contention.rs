//! 组合删除必须在既有写锁预算内等待短竞争，不能在读后升级事务时提前失败。
use super::{assert_receive_delete_fixture_counts, make_repo, seed_entry_with_receive_state};
use crate::db::connection::{establish_waiting, sqlite_failure_class};
use diesel::connection::SimpleConnection;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::time::timeout;
use uc_core::ports::clipboard::DeleteClipboardEntryWithReceiveStatePort;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn combined_delete_waits_for_a_short_writer_before_reading_receive_state() {
    let (repo, executor, directory) = make_repo();
    let (entry, _) = seed_entry_with_receive_state(&executor, "failed", "landed");
    let database = directory.path().join("entry-repo.sqlite");
    let mut writer = establish_waiting(database.to_str().unwrap()).unwrap();
    writer.batch_execute("BEGIN IMMEDIATE").unwrap();

    let repo = Arc::new(repo);
    let deleting_repo = repo.clone();
    let (started, entered) = oneshot::channel();
    let mut deleting = tokio::spawn(async move {
        started.send(()).unwrap();
        deleting_repo.delete_entry_with_receive_state(&entry).await
    });
    timeout(Duration::from_secs(5), entered)
        .await
        .unwrap()
        .unwrap();
    let early = timeout(Duration::from_millis(100), &mut deleting).await;
    let waited = early.is_err();
    assert_receive_delete_fixture_counts(&executor, 1);
    // 先释放真实写事务并回收任务，再判断，失败路径也不遗留锁或后台任务。
    writer.batch_execute("ROLLBACK").unwrap();
    let result = match early {
        Ok(joined) => joined.unwrap(),
        Err(_) => timeout(Duration::from_secs(5), deleting)
            .await
            .unwrap()
            .unwrap(),
    };
    let failure = result
        .as_ref()
        .err()
        .and_then(|source| sqlite_failure_class(source));
    assert!(
        waited,
        "短写锁释放前组合删除应等待，不能提前返回失败：{failure:?}"
    );
    assert!(result.unwrap().removed);
    assert_receive_delete_fixture_counts(&executor, 0);
}
