//! 完整接收驱动真实目录发布；文件系统port只注入可解除的撤回拒绝。
use super::*;
use crate::transfer::receive::materialization::ReceiveFailureSettlementError;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::timeout;

struct BlockWithdrawal {
    inner: FakeAtomicPublisher,
    destination: PathBuf,
    blocked: AtomicBool,
    blocked_root: Option<&'static str>,
    fail_later_publish: bool,
    withdrawals: AtomicUsize,
}
impl BlockWithdrawal {
    fn refusal(&self, source: &Path, destination: &Path) -> Option<PublishError> {
        if source.parent() == Some(self.destination.as_path()) {
            self.withdrawals.fetch_add(1, Ordering::SeqCst);
            if self.blocked.load(Ordering::SeqCst)
                && self
                    .blocked_root
                    .is_none_or(|name| source.file_name().is_some_and(|part| part == name))
            {
                return Some(PublishError::Io(
                    io::Error::from(io::ErrorKind::PermissionDenied).into(),
                ));
            }
        }
        if self.fail_later_publish
            && destination.parent() == Some(self.destination.as_path())
            && destination.file_name().is_some_and(|name| name == "beta")
        {
            return Some(PublishError::Io(
                io::Error::from(io::ErrorKind::StorageFull).into(),
            ));
        }
        None
    }
}
#[async_trait]
impl AtomicPublishPort for BlockWithdrawal {
    async fn publish_no_replace(
        &self,
        source: &Path,
        destination: &Path,
    ) -> std::result::Result<(), PublishError> {
        if let Some(error) = self.refusal(source, destination) {
            return Err(error);
        }
        self.inner.publish_no_replace(source, destination).await
    }
    async fn publish_into_free_name(
        &self,
        source: &Path,
        destination: &Path,
    ) -> std::result::Result<(), PublishError> {
        if let Some(error) = self.refusal(source, destination) {
            return Err(error);
        }
        self.inner.publish_into_free_name(source, destination).await
    }
    async fn supports_no_replace(&self, directory: &Path) -> bool {
        self.inner.supports_no_replace(directory).await
    }
}

#[tokio::test]
async fn directory_capture_failure_keeps_original_rollback_until_withdrawal_recovers() {
    assert_directory_rollback(false, DirectoryFailure::Capture).await;
}

#[tokio::test]
async fn directory_rollback_never_touches_a_previously_withdrawn_name_reused_by_the_user() {
    assert_directory_rollback(true, DirectoryFailure::Capture).await;
}

#[tokio::test]
async fn directory_mid_publication_failure_keeps_earlier_root_rollback_owned() {
    assert_directory_rollback(false, DirectoryFailure::Publication).await;
}

#[tokio::test]
async fn directory_commit_recheck_failure_keeps_original_rollback_and_source() {
    assert_directory_rollback(false, DirectoryFailure::CommitRecheck).await;
}

#[tokio::test]
async fn directory_identity_recheck_failure_keeps_original_rollback_until_settled() {
    assert_directory_rollback(false, DirectoryFailure::IdentityRecheck).await;
}

/// 正常物化器在发布前校验；故意破坏返回值以驱动外层拒绝分支的完整输出责任。
struct AlterPublishedIdentity {
    inner: FileCacheBlobMaterializer,
}
#[async_trait]
impl InboundBlobMaterializer for AlterPublishedIdentity {
    async fn materialize_plan(&self, plan: ReceiveWorkPlan) -> Result<MaterializeResult> {
        let mut result = self.inner.materialize_plan(plan).await?;
        result.snapshot.file_set_v1_component = Some([0; 32]);
        Ok(result)
    }
}

#[derive(Clone, Copy)]
enum DirectoryFailure {
    Capture,
    Publication,
    CommitRecheck,
    IdentityRecheck,
}

struct RejectCommitRecheck;
#[async_trait]
impl ClaimReceiveCommitPort for RejectCommitRecheck {
    async fn claim_receive_commit(&self, _: &str, _: &str, _: i64) -> Result<bool, AttemptError> {
        Err(AttemptError::Backend(Box::new(io::Error::from(
            io::ErrorKind::StorageFull,
        ))))
    }
}

async fn assert_directory_rollback(reuse: bool, failure: DirectoryFailure) {
    let publish_failure = matches!(failure, DirectoryFailure::Publication);
    let identity_failure = matches!(failure, DirectoryFailure::IdentityRecheck);
    let cache = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    std::fs::write(destination.path().join("unrelated.txt"), b"keep").unwrap();
    let publisher = Arc::new(BlockWithdrawal {
        inner: FakeAtomicPublisher::default(),
        destination: destination.path().to_owned(),
        blocked: AtomicBool::new(true),
        blocked_root: reuse.then_some("alpha"),
        fail_later_publish: publish_failure,
        withdrawals: AtomicUsize::new(0),
    });
    let attempts = AttemptGate::empty();
    let materializer = FileCacheBlobMaterializer::new(
        Arc::new(writing_fetcher(2)),
        cache.path().to_owned(),
        publisher.clone(),
    )
    .with_save_dir_resolver(Arc::new(FakeSaveDir {
        dir: destination.path().to_owned(),
    }))
    .with_directory_receive_attempt_ports(
        attempts.clone(),
        attempts.clone(),
        attempts.clone(),
        attempts.clone(),
    );
    let materializer: Arc<dyn InboundBlobMaterializer> =
        if matches!(failure, DirectoryFailure::IdentityRecheck) {
            Arc::new(AlterPublishedIdentity {
                inner: materializer,
            })
        } else {
            Arc::new(materializer)
        };
    let mut repo = MockEntryRepo::new();
    repo.expect_find_entry_id_by_snapshot_hash()
        .returning(|_| Ok(None));
    let captured_entry = Arc::new(Mutex::new(None));
    let recorded = captured_entry.clone();
    let mut capture = MockCapture::new();
    capture
        .expect_capture()
        .times(usize::from(matches!(failure, DirectoryFailure::Capture)))
        .returning(move |entry, _, _| {
            *recorded.lock().unwrap() = Some(entry);
            Err(io::Error::from(io::ErrorKind::StorageFull).into())
        });
    let mut write = MockWrite::new();
    write.expect_write().times(0);
    let events = Arc::new(RecordingEmitter::default());
    let bus = Arc::new(HostEventBus::new());
    bus.register("recorder", events.clone());
    let claim_commit: Arc<dyn ClaimReceiveCommitPort> =
        if matches!(failure, DirectoryFailure::CommitRecheck) {
            Arc::new(RejectCommitRecheck)
        } else {
            attempts.clone()
        };
    let receiver = build(repo, capture, write)
        .with_blob_materializer(materializer)
        .with_host_event_emitter(bus)
        .with_receive_attempt_ports(
            attempts.clone(),
            attempts.clone(),
            claim_commit,
            attempts.clone(),
            attempts.clone(),
            attempts.clone(),
            attempts.clone(),
        );
    let (plaintext, snapshot_hash) = two_root_wire_payload();
    let error = receiver
        .execute(ApplyInboundInput {
            source_entry: None,
            from_device: DeviceId::new("source"),
            snapshot_hash,
            plaintext,
            resurface_intent: ClipboardWriteIntent::RemotePush,
            activation_owner: InboundActivationOwner::Inbound,
        })
        .await
        .unwrap_err();
    let entry = captured_entry
        .lock()
        .unwrap()
        .clone()
        .or_else(|| {
            events.snapshot().into_iter().find_map(|event| match event {
                HostEvent::Clipboard(ClipboardHostEvent::IncomingPending { entry_id, .. }) => {
                    Some(EntryId::from(entry_id))
                }
                _ => None,
            })
        })
        .unwrap();
    let before = attempts
        .get_entry_attempt(entry.as_ref())
        .await
        .unwrap()
        .unwrap();
    if reuse {
        assert!(!destination.path().join("beta").exists());
        std::fs::create_dir(destination.path().join("beta")).unwrap();
        std::fs::write(destination.path().join("beta/new.txt"), b"new user data").unwrap();
    }
    let expected = if reuse {
        vec!["beta".to_owned(), "unrelated.txt".to_owned()]
    } else {
        vec!["unrelated.txt".to_owned()]
    };
    let blocked_shutdown = timeout(Duration::from_millis(20), receiver.shutdown()).await;
    publisher.blocked.store(false, Ordering::SeqCst);
    let cleaned = timeout(Duration::from_secs(5), async {
        loop {
            if visible_names(destination.path()) == expected {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    timeout(Duration::from_secs(10), receiver.shutdown())
        .await
        .unwrap()
        .unwrap();
    let final_attempt = attempts
        .get_entry_attempt(entry.as_ref())
        .await
        .unwrap()
        .unwrap();
    assert!(blocked_shutdown.is_err(), "原回滚未完成时关闭不得提前成功");
    let source = match (&error, failure) {
        (ApplyInboundError::Capture(source), DirectoryFailure::Capture)
        | (
            ApplyInboundError::Internal(source),
            DirectoryFailure::Publication
            | DirectoryFailure::CommitRecheck
            | DirectoryFailure::IdentityRecheck,
        ) => source,
        _ => panic!("original failure classification required"),
    };
    let failure = source
        .chain()
        .find_map(|source| source.downcast_ref::<ReceiveFailureSettlementError>())
        .unwrap();
    if identity_failure {
        assert!(failure
            .cause
            .chain()
            .any(|source| source.to_string().contains("identity mismatch")));
    } else {
        assert!(failure.cause.chain().any(|source| source
            .downcast_ref::<io::Error>()
            .is_some_and(|source| source.kind() == io::ErrorKind::StorageFull)));
    }
    assert!(failure.settlement.chain().any(|source| source
        .downcast_ref::<io::Error>()
        .is_some_and(|source| source.kind() == io::ErrorKind::PermissionDenied)));
    assert!(publisher.withdrawals.load(Ordering::SeqCst) > 0);
    assert!(
        cleaned.is_ok(),
        "解除撤回拒绝后原动作必须自行移走全部未保存目录，不能只记录残留后结束"
    );
    assert_eq!(
        before.state,
        AttemptState::Failing,
        "输出仍未撤回时不得交接NoEntry终态"
    );

    assert_eq!(final_attempt.state, AttemptState::Failed);
    assert_eq!(
        std::fs::read(destination.path().join("unrelated.txt")).unwrap(),
        b"keep"
    );
    if reuse {
        assert_eq!(
            std::fs::read(destination.path().join("beta/new.txt")).unwrap(),
            b"new user data"
        );
    }
    assert_eq!(
        std::fs::read_dir(destination.path()).unwrap().count(),
        expected.len()
    );
}

#[cfg(unix)]
#[path = "directory_settlement/preparation.rs"]
mod preparation;

#[path = "directory_settlement/commit.rs"]
mod commit;

#[path = "directory_settlement/duplicate.rs"]
mod duplicate;

#[path = "directory_settlement/untracked.rs"]
mod untracked;
