//! 保存和物理清理均失败时，原动作仍承担结算；已清理路径不能重复删除。
use super::*;

#[tokio::test]
async fn failed_save_cleanup_retains_original_resources_and_never_deletes_a_reused_path() {
    let fixture = Fixture::new(0).await;
    *fixture.capture.fail_progress_at.lock().unwrap() = Some(0);
    fixture
        .capture
        .obstruct_cleanup
        .store(true, Ordering::SeqCst);
    fixture
        .attempts
        .fail_begin_failure
        .store(true, Ordering::SeqCst);
    let (facade, transport, protected) = local_file_probe();
    let apply = fixture
        .apply
        .as_ref()
        .clone()
        .with_blob_materializer(Arc::new(test_materializer(
            facade,
            fixture._cache.path().to_owned(),
        )));
    let (prepared, accepted) = fixture.prepared();
    let error = apply.execute_retry(prepared).await.unwrap_err();
    assert!(matches!(
        accepted.await.unwrap(),
        RetryEntryReceiveOutcome::Accepted { .. }
    ));
    assert_eq!(
        protected.load(Ordering::SeqCst),
        1,
        "物理清理未完成，原保护和下载许可不能释放"
    );
    let ApplyInboundError::Internal(source) = &error else {
        panic!("wrong failure: {error:?}")
    };
    let settlement = source
        .chain()
        .find_map(|source| {
            source.downcast_ref::<
        crate::transfer::receive::materialization::FileFailureSettlementError>()
        })
        .unwrap();
    assert_eq!(
        settlement
            .settlement
            .downcast_ref::<io::Error>()
            .unwrap()
            .kind(),
        io::ErrorKind::IsADirectory
    );
    assert!(source.chain().any(|source| source
        .downcast_ref::<io::Error>()
        .is_some_and(|source| source.kind() == io::ErrorKind::StorageFull)));
    let path = fixture
        .capture
        .obstructed_output
        .lock()
        .unwrap()
        .clone()
        .unwrap();
    assert!(path.is_dir());
    assert!(fixture.attempts.file_failures.lock().unwrap().is_empty());
    assert_eq!(
        *fixture.attempts.state.lock().unwrap(),
        AttemptState::Receiving
    );
    std::fs::remove_dir(&path).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if !fixture.attempts.file_failures.lock().unwrap().is_empty()
                && fixture.attempts.failure_begin_calls.load(Ordering::SeqCst) > 0
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(protected.load(Ordering::SeqCst), 1);
    std::fs::write(&path, b"new user-owned content").unwrap();
    fixture
        .attempts
        .fail_begin_failure
        .store(false, Ordering::SeqCst);
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        transport.shutdown(BlobShutdownWait::Complete),
    )
    .await
    .unwrap()
    .unwrap();
    apply.shutdown().await.unwrap();
    assert_eq!(protected.load(Ordering::SeqCst), 0);
    assert_eq!(std::fs::read(&path).unwrap(), b"new user-owned content");
    assert_eq!(std::fs::read(&fixture.saved).unwrap(), b"success");
    assert_eq!(fixture.capture.progress_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.attempts.file_failures.lock().unwrap().len(), 1);
    assert_eq!(
        *fixture.attempts.state.lock().unwrap(),
        AttemptState::Failed
    );
}
