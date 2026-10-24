//! 完整接收取得目录提交权后，生命周期封口不能把真实提交降为部分取消。
use super::*;
use crate::clipboard::capture::InboundCaptureCommitContext;
use crate::clipboard::entry_identity::EntryIdentityCoordinator;
use crate::transfer::receive::reconciliation::ReceiveReadinessCoordinator;
use anyhow::bail;
use uc_core::ports::CompletedReceiveArtifacts;
use uc_core::SnapshotHash;

struct CloseAfterPublication {
    inner: FakeAtomicPublisher,
    readiness: Arc<ReceiveReadinessCoordinator>,
}
#[async_trait]
impl AtomicPublishPort for CloseAfterPublication {
    async fn publish_no_replace(
        &self,
        source: &Path,
        destination: &Path,
    ) -> Result<(), PublishError> {
        self.inner.publish_no_replace(source, destination).await?;
        if destination.file_name().is_some_and(|name| name == "beta") {
            self.readiness.mark_not_ready();
        }
        Ok(())
    }
    async fn publish_into_free_name(
        &self,
        source: &Path,
        destination: &Path,
    ) -> Result<(), PublishError> {
        self.inner
            .publish_into_free_name(source, destination)
            .await?;
        if destination.file_name().is_some_and(|name| name == "beta") {
            self.readiness.mark_not_ready();
        }
        Ok(())
    }
    async fn supports_no_replace(&self, directory: &Path) -> bool {
        self.inner.supports_no_replace(directory).await
    }
}

/// 通过既有完整捕获能力核对提交上下文，不把部分保存误当成目录成功。
struct CommitWinnerCapture {
    attempts: Arc<AttemptGate>,
    captures: Arc<AtomicUsize>,
}
#[async_trait]
impl InboundCapture for CommitWinnerCapture {
    async fn capture(
        &self,
        _: EntryId,
        _: DeviceId,
        _: SystemClipboardSnapshot,
    ) -> Result<Option<EntryId>> {
        bail!("directory capture must include the original complete commit context")
    }
    async fn capture_inbound_with_identity(
        &self,
        entry: EntryId,
        _: DeviceId,
        _: SystemClipboardSnapshot,
        _: Option<SnapshotHash>,
        commit: InboundCaptureCommitContext,
    ) -> Result<Option<EntryId>> {
        let InboundCaptureCommitContext::Complete {
            attempt_id,
            file_set: Some(_),
            artifacts: CompletedReceiveArtifacts::DirectoryPublished,
            ..
        } = commit
        else {
            bail!("committed directory must not be saved as a partial or unowned entry")
        };
        assert_eq!(attempt_id, *self.attempts.attempt_id.lock().unwrap());
        let mut state = self.attempts.state.lock().unwrap();
        assert_eq!(*state, AttemptState::Committing);
        *state = AttemptState::Completed;
        self.captures.fetch_add(1, Ordering::SeqCst);
        Ok(Some(entry))
    }
}

#[tokio::test]
async fn directory_commit_winner_finishes_durable_save_when_generation_closes_after_publication() {
    let cache = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let readiness = Arc::new(ReceiveReadinessCoordinator::new());
    readiness.ensure_ready(|| async { Ok(()) }).await.unwrap();
    let attempts = AttemptGate::empty();
    let publisher = Arc::new(CloseAfterPublication {
        inner: FakeAtomicPublisher::default(),
        readiness: readiness.clone(),
    });
    let materializer = FileCacheBlobMaterializer::new(
        Arc::new(writing_fetcher(2)),
        cache.path().to_owned(),
        publisher,
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
    let mut repo = MockEntryRepo::new();
    repo.expect_find_entry_id_by_snapshot_hash()
        .returning(|_| Ok(None));
    let captures = Arc::new(AtomicUsize::new(0));
    let capture = CommitWinnerCapture {
        attempts: attempts.clone(),
        captures: captures.clone(),
    };
    let receiver = ApplyInboundClipboardUseCase::store_only_pull(StoreOnlyPullDeps {
        common: InboundApplyCommonDeps {
            receive_retry: None,
            receive_work: readiness.work_registry(),
            entry_repo: Arc::new(repo),
            capture: Arc::new(capture),
            blob_materializer: Arc::new(materializer),
            receive_attempts: InboundReceiveAttemptDeps {
                get: attempts.clone(),
                begin: attempts.clone(),
                claim_commit: attempts.clone(),
                request_cancel: attempts.clone(),
                begin_failure: attempts.clone(),
                commit: attempts.clone(),
                clock: attempts.clone(),
            },
            receive_artifact_cleanup: Arc::new(NoopReceiveArtifactCleanup),
            receive_readiness: readiness.clone(),
            host_event_emitter: Arc::new(HostEventBus::new()),
            search_live_index: Arc::new(RecordingLiveIndex {
                calls: AtomicUsize::new(0),
            }),
            availability: Arc::new(FakeAvailability { available: false }),
            entry_identity_coordinator: Arc::new(EntryIdentityCoordinator::new()),
        },
    });
    let (plaintext, snapshot_hash) = two_root_wire_payload();
    let outcome = receiver
        .execute(ApplyInboundInput {
            source_entry: None,
            from_device: DeviceId::new("source"),
            snapshot_hash,
            plaintext,
            resurface_intent: ClipboardWriteIntent::RemotePush,
            activation_owner: InboundActivationOwner::Inbound,
        })
        .await;
    timeout(Duration::from_secs(10), receiver.shutdown())
        .await
        .unwrap()
        .unwrap();
    assert!(!readiness.is_ready(), "已封口代次不能被原任务重新打开");
    assert!(
        matches!(outcome, Ok(ApplyOutcome::Applied { .. })),
        "目录提交权已经胜出，原任务必须完成真实保存而非遗留Committing：{outcome:?}"
    );
    assert_eq!(captures.load(Ordering::SeqCst), 1);
    assert_eq!(*attempts.state.lock().unwrap(), AttemptState::Completed);
    assert_eq!(attempts.failure_begin_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        std::fs::read(destination.path().join("alpha/one.txt")).unwrap(),
        b"hello"
    );
    assert_eq!(
        std::fs::read(destination.path().join("beta/two.txt")).unwrap(),
        b"hello"
    );
    assert_eq!(
        std::fs::read_dir(destination.path()).unwrap().count(),
        2,
        "已保存目录不能留下未交接的staging"
    );
}
