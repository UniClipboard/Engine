//! 无权威尝试的内部组合也必须由原接收任务承担实际撤回。
use super::*;

#[tokio::test]
async fn directory_without_attempt_tracking_retains_failed_and_empty_capture_rollback() {
    for empty_capture in [false, true] {
        let cache = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        std::fs::write(destination.path().join("unrelated.txt"), b"keep").unwrap();
        let publisher = Arc::new(BlockWithdrawal {
            inner: FakeAtomicPublisher::default(),
            destination: destination.path().to_owned(),
            blocked: AtomicBool::new(true),
            blocked_root: None,
            fail_later_publish: false,
            withdrawals: AtomicUsize::new(0),
        });
        let materializer = FileCacheBlobMaterializer::new(
            Arc::new(writing_fetcher(2)),
            cache.path().to_owned(),
            publisher.clone(),
        )
        .with_save_dir_resolver(Arc::new(FakeSaveDir {
            dir: destination.path().to_owned(),
        }));
        let mut repo = MockEntryRepo::new();
        repo.expect_find_entry_id_by_snapshot_hash()
            .returning(|_| Ok(None));
        let mut capture = MockCapture::new();
        capture.expect_capture().times(1).returning(move |_, _, _| {
            if empty_capture {
                Ok(None)
            } else {
                Err(io::Error::from(io::ErrorKind::StorageFull).into())
            }
        });
        let mut write = MockWrite::new();
        write.expect_write().times(0);
        let receiver = build(repo, capture, write).with_blob_materializer(Arc::new(materializer));
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
        let pending = timeout(Duration::from_millis(20), receiver.shutdown()).await;
        publisher.blocked.store(false, Ordering::SeqCst);
        timeout(Duration::from_secs(10), receiver.shutdown())
            .await
            .unwrap()
            .unwrap();
        assert!(
            pending.is_err(),
            "没有权威尝试也不能在撤回失败后提前完成关闭"
        );
        let error = outcome.unwrap_err();
        let source = match (&error, empty_capture) {
            (ApplyInboundError::Internal(source), true)
            | (ApplyInboundError::Capture(source), false) => source,
            _ => panic!("must preserve the original capture failure classification"),
        };
        let failure = source
            .chain()
            .find_map(|source| source.downcast_ref::<ReceiveFailureSettlementError>())
            .unwrap();
        if !empty_capture {
            assert!(failure.cause.chain().any(|source| source
                .downcast_ref::<io::Error>()
                .is_some_and(|source| source.kind() == io::ErrorKind::StorageFull)));
        }
        assert!(failure.settlement.chain().any(|source| source
            .downcast_ref::<io::Error>()
            .is_some_and(|source| source.kind() == io::ErrorKind::PermissionDenied)));
        assert_eq!(
            std::fs::read(destination.path().join("unrelated.txt")).unwrap(),
            b"keep"
        );
        assert_eq!(
            std::fs::read_dir(destination.path()).unwrap().count(),
            1,
            "原任务须自动撤回所有未保存根并清理暂存区"
        );
    }
}
