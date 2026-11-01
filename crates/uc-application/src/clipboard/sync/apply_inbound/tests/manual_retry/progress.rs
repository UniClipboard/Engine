//! 完整接收／重试、真实Facade许可及本地输出；保存端口由明确替身控制。
mod cleanup;
use super::*;
use crate::transfer::blob::test_support::local_file_probe;
use crate::transfer::blob::transport::BlobShutdownWait;
use uc_core::ports::{
    ReceiveArtifactLogError, ReceiveArtifactRecord, ReceiveItemFailureReason,
    RecordReceiveArtifactsPort,
};

#[derive(Default)]
struct FailedCleanupJournal(Mutex<Vec<ReceiveArtifactRecord>>);
#[async_trait]
impl RecordReceiveArtifactsPort for FailedCleanupJournal {
    async fn record_receive_artifacts(
        &self,
        record: &ReceiveArtifactRecord,
    ) -> Result<(), ReceiveArtifactLogError> {
        if record.artifacts.is_empty() {
            return Err(ReceiveArtifactLogError::Backend(Box::new(io::Error::from(
                io::ErrorKind::PermissionDenied,
            ))));
        }
        assert_eq!(record.artifacts.len(), 1);
        assert!(record.artifacts[0].staged_path.is_file());
        self.0.lock().unwrap().push(record.clone());
        Ok(())
    }
}

#[tokio::test]
async fn nine_file_retry_commits_each_file_under_original_permit_without_accumulating_capacity() {
    let fixture = Fixture::with_items(0, 10).await;
    let (facade, transport, protected) = local_file_probe(None);
    *fixture.capture.guards.lock().unwrap() = Some(protected.clone());
    let apply = fixture
        .apply
        .as_ref()
        .clone()
        .with_blob_materializer(Arc::new(test_materializer(
            facade,
            fixture._cache.path().to_owned(),
        )));
    let (mut prepared, reply) = fixture.prepared();
    let selection = prepared.selection.as_mut().unwrap();
    selection.selected = (1..10).collect();
    selection.unselected.clear();
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        apply.execute_retry(prepared),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(matches!(
        reply.await.unwrap(),
        RetryEntryReceiveOutcome::Accepted { .. }
    ));
    assert_eq!(fixture.capture.progress_calls.load(Ordering::SeqCst), 9);
    let held = fixture.capture.held.lock().unwrap().clone();
    assert_eq!(
        held,
        [vec![(true, 1); 9], vec![(false, 0)]].concat(),
        "每次逐项保存持有原许可，最终汇总时这些文件均已交付"
    );
    let snapshots = fixture.capture.snapshots.lock().unwrap();
    assert_eq!(snapshots.len(), 10);
    assert_eq!(
        snapshots
            .iter()
            .map(|snapshot| snapshot.file_content_digests.len())
            .collect::<Vec<_>>(),
        (2..=10).chain([10]).collect::<Vec<_>>()
    );
    assert_eq!(protected.load(Ordering::SeqCst), 0);
    assert_eq!(
        *fixture.attempts.state.lock().unwrap(),
        AttemptState::Completed
    );
    assert_eq!(std::fs::read(&fixture.saved).unwrap(), b"success");
    drop(snapshots);
    transport
        .shutdown(BlobShutdownWait::Complete)
        .await
        .unwrap();
}

#[tokio::test]
async fn failed_file_commit_cleans_only_its_output_and_next_file_can_commit() {
    let fixture = Fixture::new(0).await;
    *fixture.capture.fail_progress_at.lock().unwrap() = Some(0);
    let (facade, transport, protected) = local_file_probe(None);
    *fixture.capture.guards.lock().unwrap() = Some(protected.clone());
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
    let (mut prepared, reply) = fixture.prepared();
    let selection = prepared.selection.as_mut().unwrap();
    selection.selected = HashSet::from([1, 2]);
    selection.unselected.clear();
    apply.execute_retry(prepared).await.unwrap();
    let RetryEntryReceiveOutcome::Accepted { attempt_id } = reply.await.unwrap() else {
        panic!("not accepted")
    };
    assert_eq!(
        journal.0.lock().unwrap().len(),
        2,
        "本项输出已删除，拒绝再次写清理日志不应覆盖原保存错误或阻止后项"
    );
    assert_eq!(fixture.capture.progress_calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        fixture.attempts.file_failures.lock().unwrap().as_slice(),
        &[(
            format!("receiver:attempt:{attempt_id}:member:1"),
            uc_core::ports::ReceiveFileFailure::Failed(
                ReceiveItemFailureReason::StorageUnavailable
            ),
        )]
    );
    assert_eq!(
        fixture.capture.held.lock().unwrap().as_slice(),
        &[(true, 1), (true, 1), (false, 0)]
    );
    assert_eq!(
        *fixture.attempts.state.lock().unwrap(),
        AttemptState::Failed
    );
    let (_, landed) = fixture
        .work
        .retry_material(&EntryId::from("receiver"), &attempt_id)
        .unwrap();
    assert_eq!(
        landed.keys().copied().collect::<HashSet<_>>(),
        HashSet::from([0, 2])
    );
    let mut result = EntryReceiveProgress {
        snapshot_version: None,
        entry_id: "receiver".into(),
        attempt_id,
        state: AttemptState::Failed,
        total_bytes: 0,
        completed_bytes: 0,
        items_total: 0,
        items_completed: 0,
        items: Vec::new(),
    };
    fixture.work.merge_progress(&mut result).unwrap();
    assert_eq!(
        result
            .items
            .iter()
            .map(|item| item.status)
            .collect::<Vec<_>>(),
        vec![
            TrackedFileTransferStatus::Completed,
            TrackedFileTransferStatus::Failed,
            TrackedFileTransferStatus::Completed
        ]
    );
    assert_eq!(
        result.items[1].failure_reason,
        Some(ReceiveItemFailureReason::StorageUnavailable)
    );
    assert_eq!(std::fs::read(&fixture.saved).unwrap(), b"success");
    assert_eq!(
        std::fs::read_dir(fixture.saved.parent().unwrap())
            .unwrap()
            .count(),
        2,
        "失败保存的独立输出必须移除，旧成功及后项成功均保留"
    );
    assert_eq!(protected.load(Ordering::SeqCst), 0);
    transport
        .shutdown(BlobShutdownWait::Complete)
        .await
        .unwrap();
}

#[tokio::test]
async fn cancellation_cas_before_stop_notification_prevents_next_file_and_keeps_saved_success() {
    let fixture = Fixture::new(0).await;
    *fixture.capture.cancel_progress_at.lock().unwrap() = Some(0);
    let (facade, transport, protected) = local_file_probe(None);
    let apply = fixture
        .apply
        .as_ref()
        .clone()
        .with_blob_materializer(Arc::new(test_materializer(
            facade,
            fixture._cache.path().to_owned(),
        )));
    let (mut prepared, reply) = fixture.prepared();
    let selection = prepared.selection.as_mut().unwrap();
    selection.selected = HashSet::from([1, 2]);
    selection.unselected.clear();
    apply.execute_retry(prepared).await.unwrap();
    let RetryEntryReceiveOutcome::Accepted { attempt_id } = reply.await.unwrap() else {
        panic!("not accepted")
    };
    assert_eq!(
        fixture.capture.progress_calls.load(Ordering::SeqCst),
        1,
        "CAS封口即阻止后项，不能依赖尚未抵达的停止通知"
    );
    assert_eq!(
        *fixture.attempts.state.lock().unwrap(),
        AttemptState::Cancelled
    );
    let (_, landed) = fixture
        .work
        .retry_material(&EntryId::from("receiver"), &attempt_id)
        .unwrap();
    assert_eq!(
        landed.keys().copied().collect::<HashSet<_>>(),
        HashSet::from([0])
    );
    assert_eq!(std::fs::read(&fixture.saved).unwrap(), b"success");
    assert_eq!(
        std::fs::read_dir(fixture.saved.parent().unwrap())
            .unwrap()
            .count(),
        1
    );
    assert_eq!(protected.load(Ordering::SeqCst), 0);
    transport
        .shutdown(BlobShutdownWait::Complete)
        .await
        .unwrap();
}

#[tokio::test]
async fn failure_settlement_error_preserves_both_sources_and_keeps_attempt_nonterminal() {
    let fixture = Fixture::new(0).await;
    *fixture.capture.fail_progress_at.lock().unwrap() = Some(0);
    fixture
        .attempts
        .fail_file_settlement
        .store(true, Ordering::SeqCst);
    fixture
        .attempts
        .fail_begin_failure
        .store(true, Ordering::SeqCst);
    let (facade, transport, protected) = local_file_probe(None);
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
    let RetryEntryReceiveOutcome::Accepted { .. } = accepted.await.unwrap() else {
        panic!("not accepted")
    };
    let ApplyInboundError::Internal(source) = &error else {
        panic!("wrong failure category: {error:?}")
    };
    let failure = source
        .chain()
        .find_map(|source| {
            source
        .downcast_ref::<crate::transfer::receive::materialization::ReceiveFailureSettlementError>()
        })
        .unwrap();
    assert!(failure.cause.chain().any(|source| source
        .downcast_ref::<io::Error>()
        .is_some_and(|source| source.kind() == io::ErrorKind::StorageFull)));
    assert!(failure.settlement.chain().any(|source| source
        .downcast_ref::<io::Error>()
        .is_some_and(|source| source.kind() == io::ErrorKind::PermissionDenied)));
    assert_eq!(
        *fixture.attempts.state.lock().unwrap(),
        AttemptState::Receiving
    );
    assert_eq!(
        fixture.attempts.failure_begin_calls.load(Ordering::SeqCst),
        0,
        "未完成的文件结算直接移交恢复，不再写批次失败"
    );
    assert_eq!(fixture.capture.progress_calls.load(Ordering::SeqCst), 1);
    assert!(fixture.attempts.file_failures.lock().unwrap().is_empty());
    assert_eq!(std::fs::read(&fixture.saved).unwrap(), b"success");
    assert_eq!(
        std::fs::read_dir(fixture.saved.parent().unwrap())
            .unwrap()
            .count(),
        1
    );
    assert_eq!(
        protected.load(Ordering::SeqCst),
        1,
        "失败事实尚未结算，原保护和许可必须仍由实际负责人持有"
    );
    assert!(
        matches!(transport.shutdown(BlobShutdownWait::Bounded).await,
        Err(error) if error.kind == crate::transfer::blob::transport::BlobExchangeFailure::ShutdownIncomplete)
    );
    assert_eq!(protected.load(Ordering::SeqCst), 1);
    fixture
        .attempts
        .fail_file_settlement
        .store(false, Ordering::SeqCst);
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
    assert_eq!(
        protected.load(Ordering::SeqCst),
        1,
        "单项失败已记录但父尝试仍未结算，原资源仍须保留"
    );
    assert_eq!(
        *fixture.attempts.state.lock().unwrap(),
        AttemptState::Receiving
    );
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
    assert_eq!(
        fixture.capture.progress_calls.load(Ordering::SeqCst),
        1,
        "本地结算恢复不重新保存数据或启动后续文件"
    );
    assert_eq!(fixture.attempts.file_failures.lock().unwrap().len(), 1);
    assert_eq!(std::fs::read(&fixture.saved).unwrap(), b"success");
    assert_eq!(
        *fixture.attempts.state.lock().unwrap(),
        AttemptState::Failed
    );
}
