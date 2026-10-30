//! 单项取消必须覆盖真实保存等待；调用超时不等于单项已经完成。
use super::*;
use crate::transfer::blob::facade::{
    BatchPosition, FetchBlobToPathCommand, FetchTransferContext, TransferCancellationWait,
};
use crate::transfer::blob::test_support::local_file_probe;
use crate::transfer::blob::transport::{BlobExchangeError, BlobExchangeFailure, BlobShutdownWait};
use std::time::Duration;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use uc_core::ports::ReceiveItemRole;

#[tokio::test]
async fn single_file_cancel_during_owned_save_waits_for_actual_settlement() {
    let fixture = Fixture::new(0).await;
    fixture.capture.slow.store(true, Ordering::SeqCst);
    let (facade, transport, protected) = local_file_probe(None);
    let apply = Arc::new(
        fixture
            .apply
            .as_ref()
            .clone()
            .with_blob_materializer(Arc::new(test_materializer(
                facade.clone(),
                fixture._cache.path().to_owned(),
            ))),
    );
    let (prepared, accepted) = fixture.prepared();
    let caller = {
        let apply = apply.clone();
        tokio::spawn(async move { apply.execute_retry(prepared).await })
    };
    fixture.capture.entered.notified().await;
    let RetryEntryReceiveOutcome::Accepted { attempt_id } = accepted.await.unwrap() else {
        panic!("retry not accepted")
    };
    let duplicate_path = fixture._cache.path().join("duplicate-output");
    let duplicate = facade
        .fetch_blob_to_path(FetchBlobToPathCommand {
            cancellation: CancellationToken::new(),
            ticket: BlobTicket::from_bytes(vec![1]),
            entry_id: EntryId::from("source"),
            target_path: duplicate_path.clone().into(),
            transfer_context: Some(FetchTransferContext {
                item_role: ReceiveItemRole::File,
                batch: None,
                transfer_id: format!("receiver:attempt:{attempt_id}:member:1"),
                entry_id: "receiver".into(),
                attempt_id: Some(attempt_id.clone()),
                peer_id: "source".into(),
                total_bytes: Some(7),
                filename: "same.bin".into(),
                batch_position: BatchPosition::Only,
                individual_lifecycle: true,
            }),
        })
        .await;
    assert!(
        matches!(duplicate, Err(BlobTransferError::Fetch(ref error)) if error.chain().any(|source| matches!(source.downcast_ref::<BlobExchangeError>(), Some(error) if error.kind == BlobExchangeFailure::Busy)))
    );
    assert!(!duplicate_path.exists());
    let result = facade
        .cancel_inbound_transfer(
            &format!("receiver:attempt:{attempt_id}:member:1"),
            FileTransferCancellationReason::LocalUser,
            TransferCancellationWait::Bounded,
        )
        .await;
    let mut abandoned = {
        let facade = facade.clone();
        let transfer = format!("receiver:attempt:{attempt_id}:member:1");
        tokio::spawn(async move {
            facade
                .cancel_inbound_transfer(
                    &transfer,
                    FileTransferCancellationReason::LocalUser,
                    TransferCancellationWait::Complete,
                )
                .await
        })
    };
    assert!(timeout(Duration::from_millis(30), &mut abandoned)
        .await
        .is_err());
    abandoned.abort();
    assert!(abandoned.await.unwrap_err().is_cancelled());
    let mut repeated = {
        let facade = facade.clone();
        let transfer = format!("receiver:attempt:{attempt_id}:member:1");
        tokio::spawn(async move {
            facade
                .cancel_inbound_transfer(
                    &transfer,
                    FileTransferCancellationReason::LocalUser,
                    TransferCancellationWait::Complete,
                )
                .await
        })
    };
    assert!(timeout(Duration::from_millis(30), &mut repeated)
        .await
        .is_err());
    let held = protected.load(Ordering::SeqCst);
    let saved_before_release = fixture.capture.snapshots.lock().unwrap().len();
    fixture.capture.release.add_permits(1);
    caller.await.unwrap().unwrap();
    assert!(matches!(
        repeated.await.unwrap().unwrap(),
        crate::facade::blob_transfer::InboundCancelOutcome::NotInflight
    ));
    let (_, landed) = fixture
        .work
        .retry_material(&EntryId::from("receiver"), &attempt_id)
        .unwrap();
    for item in landed.values() {
        if let ReceivedOutput::File(artifact) = &item.output {
            assert!(
                artifact.final_path.is_file(),
                "实际保存的成果不得因迟到的下载取消而删除"
            );
        }
    }
    apply.shutdown().await.unwrap();
    transport
        .shutdown(BlobShutdownWait::Complete)
        .await
        .unwrap();
    assert_eq!(held, 1);
    assert_eq!(saved_before_release, 0);
    assert!(
        matches!(result, Err(BlobTransferError::CancellationIncomplete(_))),
        "保存仍未完成，单项取消不得提前声称已结束: {result:?}"
    );
    assert_eq!(protected.load(Ordering::SeqCst), 0);
    assert_eq!(std::fs::read(&fixture.saved).unwrap(), b"success");
}

#[tokio::test]
async fn save_failure_remains_the_same_failure_for_repeated_single_file_cancellation() {
    let fixture = Fixture::new(0).await;
    fixture.capture.slow.store(true, Ordering::SeqCst);
    let (facade, transport, protected) = local_file_probe(None);
    let apply = Arc::new(
        fixture
            .apply
            .as_ref()
            .clone()
            .with_blob_materializer(Arc::new(test_materializer(
                facade.clone(),
                fixture._cache.path().to_owned(),
            ))),
    );
    let (prepared, accepted) = fixture.prepared();
    let caller = {
        let apply = apply.clone();
        tokio::spawn(async move { apply.execute_retry(prepared).await })
    };
    fixture.capture.entered.notified().await;
    let RetryEntryReceiveOutcome::Accepted { attempt_id } = accepted.await.unwrap() else {
        panic!("not accepted")
    };
    let transfer = format!("receiver:attempt:{attempt_id}:member:1");
    let first = facade
        .cancel_inbound_transfer(
            &transfer,
            FileTransferCancellationReason::LocalUser,
            TransferCancellationWait::Bounded,
        )
        .await;
    assert!(matches!(
        first,
        Err(BlobTransferError::CancellationIncomplete(_))
    ));
    fixture.capture.fail.store(true, Ordering::SeqCst);
    fixture.capture.release.add_permits(1);
    assert!(caller.await.unwrap().is_err());
    for _ in 0..2 {
        let error = facade
            .cancel_inbound_transfer(
                &transfer,
                FileTransferCancellationReason::LocalUser,
                TransferCancellationWait::Complete,
            )
            .await
            .unwrap_err();
        let BlobTransferError::Fetch(source) = error else {
            panic!("actual save failure expected")
        };
        assert!(source.chain().any(|source| source
            .downcast_ref::<io::Error>()
            .is_some_and(|error| error.kind() == io::ErrorKind::StorageFull)));
    }
    let close = transport
        .shutdown(BlobShutdownWait::Complete)
        .await
        .unwrap_err();
    assert!(close.source.chain().any(|source| source
        .downcast_ref::<io::Error>()
        .is_some_and(|error| error.kind() == io::ErrorKind::StorageFull)));
    assert_eq!(protected.load(Ordering::SeqCst), 0);
    assert_eq!(std::fs::read(&fixture.saved).unwrap(), b"success");
}

#[tokio::test]
async fn observing_already_settled_file_failure_does_not_create_a_failed_stop_job() {
    let fixture = Fixture::new(0).await;
    *fixture.capture.fail_progress_at.lock().unwrap() = Some(0);
    let (facade, transport, protected) = local_file_probe(None);
    let apply = fixture
        .apply
        .as_ref()
        .clone()
        .with_blob_materializer(Arc::new(test_materializer(
            facade.clone(),
            fixture._cache.path().to_owned(),
        )));
    let (prepared, accepted) = fixture.prepared();
    apply.execute_retry(prepared).await.unwrap();
    let RetryEntryReceiveOutcome::Accepted { attempt_id } = accepted.await.unwrap() else {
        panic!("not accepted")
    };
    let result = facade
        .cancel_inbound_transfer(
            &format!("receiver:attempt:{attempt_id}:member:1"),
            FileTransferCancellationReason::LocalUser,
            TransferCancellationWait::Complete,
        )
        .await;
    let Err(BlobTransferError::Fetch(source)) = result else {
        panic!("actual file failure must stay visible")
    };
    assert!(source.chain().any(|source| source
        .downcast_ref::<io::Error>()
        .is_some_and(|error| error.kind() == io::ErrorKind::StorageFull)));
    assert_eq!(protected.load(Ordering::SeqCst), 0);
    transport
        .shutdown(BlobShutdownWait::Complete)
        .await
        .unwrap();
}
