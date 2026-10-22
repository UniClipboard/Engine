//! 成员获取失败后真实staging清理拒绝仍归原完整接收动作。
use super::*;
use crate::facade::host_event::TransferHostEvent;
use std::fs::Permissions;
use std::os::unix::fs::PermissionsExt;

#[derive(Default)]
struct StagingPermissions(Mutex<Option<(PathBuf, Permissions)>>);
impl StagingPermissions {
    fn block(&self, path: &Path) {
        let original = std::fs::metadata(path).unwrap().permissions();
        *self.0.lock().unwrap() = Some((path.to_owned(), original));
        std::fs::set_permissions(path, Permissions::from_mode(0o500)).unwrap();
    }
    fn restore(&self) {
        if let Some((path, permissions)) = self.0.lock().unwrap().take() {
            std::fs::set_permissions(path, permissions).unwrap();
        }
    }
}
impl Drop for StagingPermissions {
    fn drop(&mut self) {
        self.restore();
    }
}

struct RejectNoEntry {
    inner: Arc<AttemptGate>,
    blocked: AtomicBool,
}
#[async_trait]
impl CommitInboundReceivePort for RejectNoEntry {
    async fn commit_inbound_receive(
        &self,
        settlement: &InboundReceiveSettlement,
    ) -> Result<(), InboundReceiveCommitError> {
        if self.blocked.load(Ordering::SeqCst)
            && matches!(settlement, InboundReceiveSettlement::NoEntry { .. })
        {
            return Err(InboundReceiveCommitError::Backend(Box::new(
                io::Error::from(io::ErrorKind::PermissionDenied),
            )));
        }
        self.inner.commit_inbound_receive(settlement).await
    }
}

struct RejectStagingLog {
    permissions: Arc<StagingPermissions>,
    destination: PathBuf,
    staging: Arc<Mutex<Option<PathBuf>>>,
}
#[async_trait]
impl RecordDirectoryPublishPort for RejectStagingLog {
    async fn record_phase(
        &self,
        _: &str,
        _: &str,
        phase: PublishPhase,
        root_map: &[(PathBuf, PathBuf)],
        _: i64,
    ) -> Result<(), PublishLogError> {
        assert_eq!(phase, PublishPhase::Staging);
        let area = root_map[0].0.parent().unwrap().to_owned();
        *self.staging.lock().unwrap() = Some(area);
        self.permissions.block(&self.destination);
        Err(PublishLogError::Backend(Box::new(io::Error::from(
            io::ErrorKind::StorageFull,
        ))))
    }
    async fn record_partial(
        &self,
        _: &str,
        _: &str,
        _: u32,
        _: i64,
    ) -> Result<(), PublishLogError> {
        unreachable!("首次日志未落库，不能更新不存在的检查点")
    }
}

#[tokio::test]
async fn directory_staging_log_failure_keeps_cleanup_without_assuming_the_record_exists() {
    assert_preparation_failure(true, false).await;
}

#[tokio::test]
async fn directory_preparation_failure_keeps_staging_cleanup_in_the_original_action() {
    assert_preparation_failure(false, false).await;
}

#[tokio::test]
async fn directory_preparation_cleanup_success_keeps_original_no_entry_commit_until_repaired() {
    assert_preparation_failure(false, true).await;
}

async fn assert_preparation_failure(fail_log: bool, fail_commit: bool) {
    let cache = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    std::fs::write(destination.path().join("unrelated.txt"), b"keep").unwrap();
    let permissions = Arc::new(StagingPermissions::default());
    let blocked = permissions.clone();
    let staging = Arc::new(Mutex::new(None));
    let recorded_staging = staging.clone();
    let mut fetcher = MockBlobFetcher::new();
    fetcher
        .expect_fetch_blob_to_path()
        .times(usize::from(!fail_log))
        .returning(move |command| {
            std::fs::write(&command.target_path, b"partial receive").unwrap();
            let area = command.target_path.parent().unwrap();
            *recorded_staging.lock().unwrap() = Some(area.to_owned());
            if !fail_commit {
                blocked.block(area);
            }
            Err(io::Error::from(io::ErrorKind::StorageFull).into())
        });
    fetcher
        .expect_record_unfetched_failure()
        .times(usize::from(!fail_log))
        .returning(|_, _| ());
    let attempts = AttemptGate::empty();
    let publish_log: Arc<dyn RecordDirectoryPublishPort> = if fail_log {
        Arc::new(RejectStagingLog {
            permissions: permissions.clone(),
            destination: destination.path().to_owned(),
            staging: staging.clone(),
        })
    } else {
        attempts.clone()
    };
    let materializer = FileCacheBlobMaterializer::new(
        Arc::new(fetcher),
        cache.path().to_owned(),
        Arc::new(FakeAtomicPublisher::default()),
    )
    .with_save_dir_resolver(Arc::new(FakeSaveDir {
        dir: destination.path().to_owned(),
    }))
    .with_directory_receive_attempt_ports(
        attempts.clone(),
        attempts.clone(),
        publish_log,
        attempts.clone(),
    );
    let mut repo = MockEntryRepo::new();
    repo.expect_find_entry_id_by_snapshot_hash()
        .returning(|_| Ok(None));
    let mut capture = MockCapture::new();
    capture.expect_capture().times(0);
    let mut write = MockWrite::new();
    write.expect_write().times(0);
    let events = Arc::new(RecordingEmitter::default());
    let bus = Arc::new(HostEventBus::new());
    bus.register("recorder", events.clone());
    let commit = Arc::new(RejectNoEntry {
        inner: attempts.clone(),
        blocked: AtomicBool::new(fail_commit),
    });
    let receiver = build(repo, capture, write)
        .with_blob_materializer(Arc::new(materializer))
        .with_host_event_emitter(bus)
        .with_receive_attempt_ports(
            attempts.clone(),
            attempts.clone(),
            attempts.clone(),
            attempts.clone(),
            attempts.clone(),
            commit.clone(),
            attempts.clone(),
        );
    let (plaintext, snapshot_hash) = two_root_wire_payload();
    let error = receiver
        .execute(ApplyInboundInput {
            from_device: DeviceId::new("source"),
            snapshot_hash,
            plaintext,
            resurface_intent: ClipboardWriteIntent::RemotePush,
            activation_owner: InboundActivationOwner::Inbound,
        })
        .await
        .unwrap_err();
    let entry = events
        .snapshot()
        .into_iter()
        .find_map(|event| match event {
            HostEvent::Clipboard(ClipboardHostEvent::IncomingPending { entry_id, .. }) => {
                Some(entry_id)
            }
            _ => None,
        })
        .unwrap();
    let area = staging.lock().unwrap().clone().unwrap();
    let before = attempts.get_entry_attempt(&entry).await.unwrap().unwrap();
    let before_events = events.snapshot();
    let blocked_shutdown = timeout(Duration::from_millis(20), receiver.shutdown()).await;
    permissions.restore();
    commit.blocked.store(false, Ordering::SeqCst);
    let cleaned = timeout(Duration::from_secs(5), async {
        while area.exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    timeout(Duration::from_secs(10), receiver.shutdown())
        .await
        .unwrap()
        .unwrap();
    assert!(
        cleaned.is_ok(),
        "权限恢复后必须由原任务自动清理失败获取留下的staging"
    );
    assert!(
        blocked_shutdown.is_err(),
        "真实staging尚未清理时关闭不能提前完成"
    );
    assert!(!before_events.iter().any(|event| matches!(event, HostEvent::Transfer(TransferHostEvent::StatusChanged { status, .. }) if matches!(status.as_str(), "failed" | "cancelled"))));
    let final_events = events.snapshot();
    assert_eq!(final_events.iter().filter(|event| matches!(event, HostEvent::Transfer(TransferHostEvent::StatusChanged { status, .. }) if status == "failed")).count(), 1, "真实交接后只发一次目录失败通知");
    assert_eq!(before.state, AttemptState::Failing);
    assert_eq!(
        attempts
            .get_entry_attempt(&entry)
            .await
            .unwrap()
            .unwrap()
            .state,
        AttemptState::Failed
    );
    let ApplyInboundError::Internal(source) = error else {
        panic!("must preserve original materialization error");
    };
    let failure = source
        .chain()
        .find_map(|source| source.downcast_ref::<ReceiveFailureSettlementError>())
        .unwrap();
    assert!(failure.cause.chain().any(|source| source
        .downcast_ref::<io::Error>()
        .is_some_and(|source| source.kind() == io::ErrorKind::StorageFull)));
    assert!(failure.settlement.chain().any(|source| source
        .downcast_ref::<io::Error>()
        .is_some_and(|source| source.kind() == io::ErrorKind::PermissionDenied)));
    assert_eq!(
        std::fs::read(destination.path().join("unrelated.txt")).unwrap(),
        b"keep"
    );
    assert_eq!(std::fs::read_dir(destination.path()).unwrap().count(), 1);
}
