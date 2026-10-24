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
    lose_once: AtomicBool,
}
#[async_trait]
impl CommitInboundReceivePort for RejectNoEntry {
    async fn commit_inbound_receive(
        &self,
        settlement: &InboundReceiveSettlement,
    ) -> Result<(), InboundReceiveCommitError> {
        if matches!(settlement, InboundReceiveSettlement::NoEntry { .. })
            && self.lose_once.swap(false, Ordering::SeqCst)
        {
            self.inner.commit_inbound_receive(settlement).await?;
            return Err(InboundReceiveCommitError::Backend(Box::new(
                io::Error::from(io::ErrorKind::BrokenPipe),
            )));
        }
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
        assert_eq!(root_map.len(), 2, "首次日志须包含完整根映射");
        assert!(!area.exists(), "日志落库前不能创建或探测staging");
        *self.staging.lock().unwrap() = Some(area);
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
async fn directory_staging_log_failure_creates_no_output_or_fallback_download() {
    assert_preparation_failure(true, false, false).await;
}

#[tokio::test]
async fn directory_preparation_failure_keeps_staging_cleanup_in_the_original_action() {
    assert_preparation_failure(false, false, false).await;
}

#[tokio::test]
async fn directory_preparation_cleanup_success_keeps_original_no_entry_commit_until_repaired() {
    assert_preparation_failure(false, true, false).await;
}

#[tokio::test]
async fn directory_original_action_finishes_after_lost_no_entry_acknowledgement_is_confirmed() {
    assert_preparation_failure(false, true, true).await;
}

async fn assert_preparation_failure(fail_log: bool, fail_commit: bool, lose_ack: bool) {
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
        lose_once: AtomicBool::new(lose_ack),
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
    if fail_log {
        assert!(!area.exists(), "首次日志失败不留下staging");
        timeout(Duration::from_secs(5), receiver.shutdown())
            .await
            .unwrap()
            .unwrap();
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
            panic!("原日志来源必须保留")
        };
        assert!(source.chain().any(|source| source
            .downcast_ref::<io::Error>()
            .is_some_and(|source| source.kind() == io::ErrorKind::StorageFull)));
        assert_eq!(
            std::fs::read(destination.path().join("unrelated.txt")).unwrap(),
            b"keep"
        );
        assert_eq!(std::fs::read_dir(destination.path()).unwrap().count(), 1);
        return;
    }
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
    assert_eq!(
        before.state,
        if lose_ack {
            AttemptState::Failed
        } else {
            AttemptState::Failing
        }
    );
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
        .is_some_and(|source| source.kind()
            == if lose_ack {
                io::ErrorKind::BrokenPipe
            } else {
                io::ErrorKind::PermissionDenied
            })));
    assert_eq!(
        std::fs::read(destination.path().join("unrelated.txt")).unwrap(),
        b"keep"
    );
    assert_eq!(std::fs::read_dir(destination.path()).unwrap().count(), 1);
}

/// 保存已经成功，后续清理拒绝不能撤回成功目录或提前释放原任务。
#[tokio::test]
async fn directory_saved_output_keeps_original_cleanup_until_permissions_recover() {
    let cache = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let permissions = Arc::new(StagingPermissions::default());
    let blocked = permissions.clone();
    let destination_path = destination.path().to_owned();
    let attempts = AttemptGate::empty();
    let captured_attempts = attempts.clone();
    let materializer = FileCacheBlobMaterializer::new(
        Arc::new(writing_fetcher(2)),
        cache.path().to_owned(),
        Arc::new(FakeAtomicPublisher::default()),
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
    let mut capture = MockCapture::new();
    capture
        .expect_capture()
        .times(1)
        .returning(move |entry, _, _| {
            assert_eq!(
                *captured_attempts.state.lock().unwrap(),
                AttemptState::Committing
            );
            *captured_attempts.state.lock().unwrap() = AttemptState::Completed;
            blocked.block(&destination_path);
            Ok(Some(entry))
        });
    let mut write = MockWrite::new();
    write.expect_write().times(0..=1).returning(|_, _| Ok(()));
    let receiver = build(repo, capture, write)
        .with_blob_materializer(Arc::new(materializer))
        .with_receive_attempt_ports(
            attempts.clone(),
            attempts.clone(),
            attempts.clone(),
            attempts.clone(),
            attempts.clone(),
            attempts.clone(),
            attempts.clone(),
        );
    let (plaintext, snapshot_hash) = two_root_wire_payload();
    let outcome = receiver
        .execute(ApplyInboundInput {
            from_device: DeviceId::new("source"),
            snapshot_hash,
            plaintext,
            resurface_intent: ClipboardWriteIntent::RemotePush,
            activation_owner: InboundActivationOwner::Inbound,
        })
        .await;
    let before = *attempts.state.lock().unwrap();
    let area = attempts.root_maps.lock().unwrap()[0][0]
        .0
        .parent()
        .unwrap()
        .to_owned();
    let pending = timeout(Duration::from_millis(20), receiver.shutdown()).await;
    permissions.restore();
    timeout(Duration::from_secs(10), receiver.shutdown())
        .await
        .unwrap()
        .unwrap();
    assert!(
        pending.is_err(),
        "已保存目录的暂存区清理仍归原动作，关闭不能提前成功"
    );
    let ApplyInboundError::Internal(source) = outcome.unwrap_err() else {
        panic!("must preserve cleanup error")
    };
    assert!(source.chain().any(|error| error
        .downcast_ref::<io::Error>()
        .is_some_and(|error| error.kind() == io::ErrorKind::PermissionDenied)));
    assert_eq!(before, AttemptState::Completed);
    assert_eq!(*attempts.state.lock().unwrap(), AttemptState::Completed);
    assert_eq!(attempts.failure_begin_calls.load(Ordering::SeqCst), 0);
    assert!(!area.exists());
    assert_eq!(
        std::fs::read(destination.path().join("alpha/one.txt")).unwrap(),
        b"hello"
    );
    assert_eq!(
        std::fs::read(destination.path().join("beta/two.txt")).unwrap(),
        b"hello"
    );
    assert_eq!(std::fs::read_dir(destination.path()).unwrap().count(), 2);
}

/// 目标卷不支持安全发布时，真实探测清理失败不能被缓存回退掩盖。
struct RefuseProbeCleanup {
    inner: FakeAtomicPublisher,
    permissions: Arc<StagingPermissions>,
    staging: Arc<Mutex<Option<PathBuf>>>,
}
#[async_trait]
impl AtomicPublishPort for RefuseProbeCleanup {
    async fn publish_no_replace(
        &self,
        source: &Path,
        destination: &Path,
    ) -> Result<(), PublishError> {
        self.inner.publish_no_replace(source, destination).await
    }
    async fn publish_into_free_name(
        &self,
        source: &Path,
        destination: &Path,
    ) -> Result<(), PublishError> {
        self.inner.publish_into_free_name(source, destination).await
    }
    async fn supports_no_replace(&self, staging: &Path) -> bool {
        *self.staging.lock().unwrap() = Some(staging.to_owned());
        self.permissions.block(staging.parent().unwrap());
        false
    }
}

#[tokio::test]
async fn directory_probe_cleanup_failure_keeps_original_owner_before_any_fallback_download() {
    let cache = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    std::fs::write(destination.path().join("unrelated.txt"), b"keep").unwrap();
    let permissions = Arc::new(StagingPermissions::default());
    let staging = Arc::new(Mutex::new(None));
    let publisher = Arc::new(RefuseProbeCleanup {
        inner: FakeAtomicPublisher::default(),
        permissions: permissions.clone(),
        staging: staging.clone(),
    });
    let fetches = Arc::new(AtomicUsize::new(0));
    let fetched = fetches.clone();
    let mut fetcher = MockBlobFetcher::new();
    fetcher
        .expect_fetch_blob_to_path()
        .times(0..=2)
        .returning(move |command| {
            fetched.fetch_add(1, Ordering::SeqCst);
            std::fs::write(&command.target_path, b"hello").unwrap();
            Ok(crate::facade::blob_transfer::FetchBlobToPathResult {
                entry_id: command.entry_id,
                plaintext_hash: PlaintextHash::from_bytes([5; 32]),
                digest: BlobDigest::from_bytes([7; 32]),
                bytes_written: 5,
            })
        });
    let attempts = AttemptGate::empty();
    let materializer =
        FileCacheBlobMaterializer::new(Arc::new(fetcher), cache.path().to_owned(), publisher)
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
    let captured = captures.clone();
    let mut capture = MockCapture::new();
    capture
        .expect_capture()
        .times(0..=1)
        .returning(move |entry, _, _| {
            captured.fetch_add(1, Ordering::SeqCst);
            Ok(Some(entry))
        });
    let mut write = MockWrite::new();
    write.expect_write().times(0..=1).returning(|_, _| Ok(()));
    let receiver = build(repo, capture, write)
        .with_blob_materializer(Arc::new(materializer))
        .with_receive_attempt_ports(
            attempts.clone(),
            attempts.clone(),
            attempts.clone(),
            attempts.clone(),
            attempts.clone(),
            attempts.clone(),
            attempts.clone(),
        );
    let (plaintext, snapshot_hash) = two_root_wire_payload();
    let outcome = receiver
        .execute(ApplyInboundInput {
            from_device: DeviceId::new("source"),
            snapshot_hash,
            plaintext,
            resurface_intent: ClipboardWriteIntent::RemotePush,
            activation_owner: InboundActivationOwner::Inbound,
        })
        .await;
    let before = *attempts.state.lock().unwrap();
    let area = staging.lock().unwrap().clone().unwrap();
    let pending = timeout(Duration::from_millis(20), receiver.shutdown()).await;
    permissions.restore();
    timeout(Duration::from_secs(10), receiver.shutdown())
        .await
        .unwrap()
        .unwrap();
    assert!(
        pending.is_err(),
        "探测暂存区未清理时不能丢弃原责任并报告关闭成功"
    );
    assert_eq!(before, AttemptState::Failing);
    assert_eq!(*attempts.state.lock().unwrap(), AttemptState::Failed);
    assert_eq!(
        fetches.load(Ordering::SeqCst),
        0,
        "清理失败不能开始回退下载"
    );
    assert_eq!(captures.load(Ordering::SeqCst), 0);
    assert!(!area.exists());
    let ApplyInboundError::Internal(source) = outcome.unwrap_err() else {
        panic!("must preserve probe cleanup error")
    };
    let failure = source
        .chain()
        .find_map(|error| error.downcast_ref::<ReceiveFailureSettlementError>())
        .unwrap();
    assert!(failure.settlement.chain().any(|error| error
        .downcast_ref::<io::Error>()
        .is_some_and(|error| error.kind() == io::ErrorKind::PermissionDenied)));
    assert_eq!(
        std::fs::read(destination.path().join("unrelated.txt")).unwrap(),
        b"keep"
    );
    assert_eq!(std::fs::read_dir(destination.path()).unwrap().count(), 1);
}

#[tokio::test]
async fn directory_early_journal_preserves_managed_fallback_for_inaccessible_save_directories() {
    for mode in [0o000, 0o500] {
        let cache = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        std::fs::write(destination.path().join("unrelated.bin"), b"keep").unwrap();
        let original = std::fs::metadata(destination.path()).unwrap().permissions();
        let permissions =
            StagingPermissions(Mutex::new(Some((destination.path().to_owned(), original))));
        std::fs::set_permissions(destination.path(), Permissions::from_mode(mode)).unwrap();
        let attempts = AttemptGate::new(AttemptState::Receiving, None);
        let materializer = gated_directory_materializer(
            writing_fetcher(2),
            cache.path(),
            destination.path(),
            Arc::new(FakeAtomicPublisher::default()),
            attempts.clone(),
        );
        let (blob_refs, manifest, snapshot) = two_root_payload();
        let result = materializer
            .materialize_plan(ReceiveWorkPlan::new(
                DeviceId::new("source"),
                EntryId::from("entry-fallback"),
                snapshot,
                blob_refs,
                Some(manifest),
                Some("attempt-1".to_owned()),
                None,
            ))
            .await;
        permissions.restore();
        let mut result = result.expect("保存目录不可读或不可写时仍应完整交付至受管缓存");
        result.take_publication().unwrap().commit().await.unwrap();
        assert_eq!(*attempts.state.lock().unwrap(), AttemptState::Committing);
        assert_eq!(
            std::fs::read(destination.path().join("unrelated.bin")).unwrap(),
            b"keep"
        );
        assert_eq!(std::fs::read_dir(destination.path()).unwrap().count(), 1);
        let managed = cache.path().join("iroh-blobs/entry-fallback");
        assert_eq!(
            std::fs::read(managed.join("alpha/one.txt")).unwrap(),
            b"hello"
        );
        assert_eq!(
            std::fs::read(managed.join("beta/two.txt")).unwrap(),
            b"hello"
        );
        let maps = attempts.root_maps.lock().unwrap();
        assert!(maps
            .last()
            .unwrap()
            .iter()
            .all(|(staged, target)| staged.starts_with(&managed) && target.starts_with(&managed)));
    }
}
