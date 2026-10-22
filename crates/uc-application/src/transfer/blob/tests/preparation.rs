//! 实际下载监督中的准备、排队与首批数据期限，不以独立计时模型代替。
use super::{
    FakeBlobReferenceRepository, FakeBlobTransfer, FakeCipher, FakeHash, SlowIdentitySource,
};
use crate::transfer::blob::test_support::{FakeLocalStore, RejectUnexpectedNetwork};
use crate::transfer::blob::transport::{
    BlobDownloadPreparation, BlobDownloadResources, BlobExchangeError, BlobExchangeFailure,
    BlobShutdownWait, BlobTransport,
};
use crate::transfer::blob::{FetchBlobInput, FetchBlobUseCase};
use std::io::{Error, ErrorKind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio::time::advance;
use tokio_util::sync::CancellationToken;
use uc_core::ids::EntryId;
use uc_core::ports::blob::{BlobTicket, BlobTransferPort, TagReason};

#[tokio::test(start_paused = true)]
async fn preparation_uses_original_first_data_deadline_and_holds_capacity_until_real_completion() {
    let transfer = Arc::new(FakeBlobTransfer::default());
    let digest = transfer
        .publish(
            bytes::Bytes::from_static(b"preparation"),
            TagReason::ClipboardEntry(EntryId::new()),
        )
        .await
        .unwrap();
    let ticket: BlobTicket = transfer.issue_ticket(&digest).await.unwrap();
    let source = Arc::new(SlowIdentitySource {
        local: FakeLocalStore(transfer.clone()),
        entered: Semaphore::new(0),
        finish: Semaphore::new(0),
        calls: AtomicUsize::new(0),
    });
    let transport = BlobTransport::new(
        Arc::new(RejectUnexpectedNetwork),
        Arc::new(RejectUnexpectedNetwork),
        Arc::new(RejectUnexpectedNetwork),
        Arc::new(RejectUnexpectedNetwork),
    );
    let usecase = Arc::new(FetchBlobUseCase::new(
        Arc::new(BlobDownloadResources {
            transport: transport.clone(),
            source: source.clone(),
            store: Arc::new(FakeLocalStore(transfer.clone())),
        }),
        Arc::new(FakeHash),
        transfer,
        Arc::new(FakeBlobReferenceRepository::default()),
        Arc::new(FakeCipher),
    ));
    let entered = Arc::new(Semaphore::new(0));
    let finish = Arc::new(Semaphore::new(0));
    let fetch = |cancel: CancellationToken| {
        let entered = entered.clone();
        let finish = finish.clone();
        tokio::spawn(usecase.execute(FetchBlobInput {
            settlement: Default::default(),
            ticket: ticket.clone(),
            entry_id: EntryId::new(),
            cancel,
            preparation: BlobDownloadPreparation {
                work: Box::pin(async move {
                    entered.add_permits(1);
                    finish.acquire().await.unwrap().forget();
                    Ok(None)
                }),
                stop: Default::default(),
            },
        }))
    };
    let first_cancel = CancellationToken::new();
    let second_cancel = CancellationToken::new();
    let first = fetch(first_cancel.clone());
    let second = fetch(second_cancel.clone());
    entered.acquire_many(2).await.unwrap().forget();
    advance(Duration::from_secs(59)).await;
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    assert!(!first_cancel.is_cancelled() && !second_cancel.is_cancelled());
    advance(Duration::from_secs(1)).await;
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    assert!(first_cancel.is_cancelled() && second_cancel.is_cancelled());
    assert!(!first.is_finished() && !second.is_finished());
    let third_cancel = CancellationToken::new();
    let third = fetch(third_cancel.clone());
    advance(Duration::from_secs(30)).await;
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        entered.available_permits(),
        0,
        "expired preparation still owns capacity"
    );
    assert_eq!(source.calls.load(Ordering::SeqCst), 0);
    finish.add_permits(2);
    for task in [first, second] {
        assert_eq!(
            task.await.unwrap().unwrap_err().kind,
            BlobExchangeFailure::RequestExpired
        );
    }
    entered.acquire().await.unwrap().forget();
    advance(Duration::from_secs(59)).await;
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    assert!(
        !third_cancel.is_cancelled(),
        "queue wait must not consume the next active first-data interval"
    );
    advance(Duration::from_secs(1)).await;
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    assert!(third_cancel.is_cancelled() && !third.is_finished());
    finish.add_permits(1);
    assert_eq!(
        third.await.unwrap().unwrap_err().kind,
        BlobExchangeFailure::RequestExpired
    );
    assert_eq!(
        source.calls.load(Ordering::SeqCst),
        0,
        "expired preparation must not begin identity or network work"
    );
    let fourth_cancel = CancellationToken::new();
    let fourth = fetch(fourth_cancel.clone());
    entered.acquire().await.unwrap().forget();
    advance(Duration::from_secs(30)).await;
    finish.add_permits(1);
    source.entered.acquire().await.unwrap().forget();
    advance(Duration::from_secs(29)).await;
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    assert!(!fourth_cancel.is_cancelled());
    advance(Duration::from_secs(1)).await;
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    assert!(
        fourth_cancel.is_cancelled() && !fourth.is_finished(),
        "identity lookup must use the first-data time left after preparation"
    );
    source.finish.add_permits(1);
    assert_eq!(
        fourth.await.unwrap().unwrap_err().kind,
        BlobExchangeFailure::RequestExpired
    );
    assert_eq!(source.calls.load(Ordering::SeqCst), 1);
    let failure = usecase
        .execute(FetchBlobInput {
            settlement: Default::default(),
            ticket,
            entry_id: EntryId::new(),
            cancel: CancellationToken::new(),
            preparation: BlobDownloadPreparation {
                work: Box::pin(async {
                    Err(BlobExchangeError::new(
                        BlobExchangeFailure::Settlement,
                        Error::from(ErrorKind::StorageFull),
                    ))
                }),
                stop: Default::default(),
            },
        })
        .await
        .unwrap_err();
    assert_eq!(failure.kind, BlobExchangeFailure::Settlement);
    assert_eq!(
        failure
            .source
            .chain()
            .find_map(|source| source.downcast_ref::<Error>())
            .unwrap()
            .kind(),
        ErrorKind::StorageFull
    );
    let stopped = transport
        .shutdown(BlobShutdownWait::Complete)
        .await
        .unwrap_err();
    assert_eq!(stopped.kind, BlobExchangeFailure::Settlement);
    assert_eq!(
        stopped
            .source
            .chain()
            .find_map(|source| source.downcast_ref::<Error>())
            .unwrap()
            .kind(),
        ErrorKind::StorageFull
    );
}
