//! 实际输出已删除，但本地交接存储仍失败；原资源和两个来源不得丢失。
use super::*;
use crate::transfer::blob::test_support::FailedFileExport;
use crate::transfer::blob::transport::BlobExchangeFailure;

#[tokio::test]
async fn failed_fetch_cleanup_ack_retains_original_source_and_resources_after_file_is_gone() {
    for cancelled in [false, true] {
        let fixture = Fixture::new(0).await;
        fixture
            .attempts
            .fail_file_settlement
            .store(true, Ordering::SeqCst);
        let failure = Arc::new(FailedFileExport {
            output: Mutex::new(None),
            cancelled,
            obstruct_cleanup: false,
        });
        let (facade, transport, protected) = local_file_probe(Some(failure.clone()));
        let journal = Arc::new(FailedCleanupJournal::default());
        let materializer = test_materializer(facade, fixture._cache.path().to_owned())
            .with_directory_receive_attempt_ports(
                fixture.attempts.clone(),
                fixture.attempts.clone(),
                fixture.attempts.clone(),
                fixture.attempts.clone(),
            )
            .with_receive_artifact_log(journal.clone());
        let apply = fixture
            .apply
            .as_ref()
            .clone()
            .with_blob_materializer(Arc::new(materializer));
        let (prepared, accepted) = fixture.prepared();
        let error = apply.execute_retry(prepared).await.unwrap_err();
        assert!(matches!(
            accepted.await.unwrap(),
            RetryEntryReceiveOutcome::Accepted { .. }
        ));
        let path = failure.output.lock().unwrap().clone().unwrap();
        assert!(!path.exists(), "实际物理清理已经成功");
        let ApplyInboundError::Internal(source) = &error else {
            panic!("wrong failure: {error:?}")
        };
        let original_preserved = if cancelled {
            source.chain().any(|source| {
                matches!(
                    source.downcast_ref::<BlobTransferError>(),
                    Some(BlobTransferError::Cancelled)
                )
            })
        } else {
            source.chain().any(|source| {
                source
                    .downcast_ref::<io::Error>()
                    .is_some_and(|source| source.kind() == io::ErrorKind::StorageFull)
            })
        };
        assert_eq!(
            (protected.load(Ordering::SeqCst), original_preserved),
            (1, true),
            "输出已清理但责任交接失败，原许可和获取来源必须仍存在"
        );
        let settlement = source
            .chain()
            .find_map(|source| source.downcast_ref::<ReceiveFailureSettlementError>())
            .unwrap();
        assert!(settlement.settlement.chain().any(|source| source
            .downcast_ref::<io::Error>()
            .is_some_and(|source| source.kind() == io::ErrorKind::PermissionDenied)));
        assert_eq!(
            *fixture.attempts.state.lock().unwrap(),
            AttemptState::Receiving
        );
        assert!(fixture.attempts.file_failures.lock().unwrap().is_empty());
        std::fs::write(&path, b"later user output").unwrap();
        fixture
            .attempts
            .fail_file_settlement
            .store(false, Ordering::SeqCst);
        let shutdown = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            transport.shutdown(BlobShutdownWait::Complete),
        )
        .await
        .unwrap();
        if cancelled {
            shutdown.unwrap();
        } else {
            assert_eq!(shutdown.unwrap_err().kind, BlobExchangeFailure::Store);
        }
        apply.shutdown().await.unwrap();
        assert_eq!(protected.load(Ordering::SeqCst), 0);
        assert_eq!(std::fs::read(&path).unwrap(), b"later user output");
        assert_eq!(std::fs::read(&fixture.saved).unwrap(), b"success");
        assert_eq!(
            *fixture.attempts.state.lock().unwrap(),
            if cancelled {
                AttemptState::Cancelled
            } else {
                AttemptState::Failed
            }
        );
        assert!(fixture.capture.snapshots.lock().unwrap().is_empty());
        assert!(fixture.attempts.file_failures.lock().unwrap().is_empty());
    }
}
