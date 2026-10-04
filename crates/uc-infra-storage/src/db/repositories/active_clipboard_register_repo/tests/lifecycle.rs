use std::sync::mpsc;
use std::time::Duration;

use diesel::connection::SimpleConnection;
use tokio::sync::oneshot;
use tokio::task::{spawn_blocking, JoinError};
use tokio::time::timeout;

use super::*;

struct FailingExecutor {
    panics: bool,
}

impl DbExecutor for FailingExecutor {
    fn run<T>(
        &self,
        _operation: impl FnOnce(&mut SqliteConnection) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        if self.panics {
            panic!("PRIVATE database task failure");
        }
        Err(IoError::new(ErrorKind::PermissionDenied, "PRIVATE database failure").into())
    }
}

#[tokio::test]
async fn database_and_task_failures_keep_their_sources_without_disclosing_them() {
    for panics in [false, true] {
        let repo = DieselActiveClipboardRegisterRepository::new(
            FailingExecutor { panics },
            Arc::new(FixedSubkey),
            Arc::new(FixedProfile),
        );
        let failure = repo.load().await.unwrap_err();
        assert!(failure.source().is_some());
        assert!(!failure.to_string().contains("PRIVATE"));
        assert!(!format!("{failure:?}").contains("PRIVATE"));
        let ActiveClipboardRegisterError::Storage(source) = failure else {
            panic!("expected storage failure");
        };
        if panics {
            assert!(source.downcast_ref::<JoinError>().unwrap().is_panic());
        } else {
            assert_eq!(
                source.downcast_ref::<IoError>().unwrap().kind(),
                ErrorKind::PermissionDenied
            );
        }
    }
}

struct UnavailableProfile;

#[async_trait]
impl CurrentProfilePort for UnavailableProfile {
    async fn current_profile(&self) -> Result<ProfileId, CurrentProfileError> {
        Err(CurrentProfileError::Unavailable)
    }
}

#[tokio::test]
async fn unavailable_profile_keeps_its_source_before_database_access() {
    let repo = DieselActiveClipboardRegisterRepository::new(
        FailingExecutor { panics: true },
        Arc::new(FixedSubkey),
        Arc::new(UnavailableProfile),
    );
    let failure = repo
        .advance(&state("blake3v1:a", 1, "dev-a"), true)
        .await
        .unwrap_err();
    assert!(failure.source().is_some());
    let ActiveClipboardRegisterError::Storage(source) = failure else {
        panic!("expected storage failure");
    };
    assert!(source.downcast_ref::<CurrentProfileError>().is_some());
}

#[tokio::test]
async fn database_contention_keeps_notifications_responsive_and_waits_for_write() {
    let (repo, reader, _directory) = make_repo();
    repo.advance(&state("blake3v1:held", 100, "dev-held"), false)
        .await
        .unwrap();
    let (entered, blocked) = oneshot::channel();
    let (release, wait) = mpsc::channel();
    let holder = spawn_blocking(move || {
        reader.run(|conn| {
            conn.batch_execute("BEGIN EXCLUSIVE")?;
            entered.send(()).unwrap();
            // 回归失败时也释放真实数据库锁。
            let _ = wait.recv_timeout(Duration::from_secs(2));
            conn.batch_execute("COMMIT")?;
            Ok(())
        })
    });
    blocked.await.unwrap();
    let mut write = Box::pin(repo.reset());
    let pending = timeout(Duration::from_millis(20), write.as_mut()).await;
    let _ = release.send(());
    holder.await.unwrap().unwrap();
    assert!(
        pending.is_err(),
        "database wait must not block notifications"
    );
    write.await.unwrap();
    assert_eq!(repo.load().await.unwrap(), None);
}
use std::error::Error as _;
use std::io::{Error as IoError, ErrorKind};
