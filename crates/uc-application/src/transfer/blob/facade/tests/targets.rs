//! 完整物化入口的目标准备必须先取得原下载许可。
use super::budget::BudgetSource;
use super::{BlockingBlobTransfer, TestCipher, TestHash, TestReferences};
use crate::clipboard::sync::payload_codec::V3BlobRef;
use crate::transfer::blob::facade::{
    BlobOutputTarget, BlobTransferDeps, BlobTransferError, BlobTransferFacade,
    FetchBlobToPathCommand, TransferCancellationWait,
};
use crate::transfer::blob::test_support::{FakeLocalStore, RejectUnexpectedNetwork};
use crate::transfer::blob::transport::{
    BlobDownloadResources, BlobExchangeError, BlobExchangeFailure, BlobShutdownWait, BlobTransport,
};
use crate::transfer::receive::materialization::{
    FileCacheBlobMaterializer, InboundBlobMaterializer, ReceiveWorkPlan,
};
use async_trait::async_trait;
use std::io::{Error, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio::time::{advance, timeout};
use tokio_util::sync::CancellationToken;
use uc_core::file_transfer::FileTransferCancellationReason;
use uc_core::ids::{DeviceId, EntryId};
use uc_core::ports::atomic_publish::{AtomicPublishPort, PublishError};
use uc_core::ports::blob::BlobTicket;
use uc_core::ports::inbound_file_target::ReserveInboundFileTargetPort;
use uc_core::SystemClipboardSnapshot;

struct BlockedTarget {
    root: PathBuf,
    entered: Semaphore,
    finish: Semaphore,
}
#[async_trait]
impl ReserveInboundFileTargetPort for BlockedTarget {
    async fn reserve_target(&self, name: &str) -> Option<PathBuf> {
        self.entered.add_permits(1);
        self.finish.acquire().await.unwrap().forget();
        let path = self.root.join(name);
        tokio::fs::write(&path, []).await.unwrap();
        Some(path)
    }
}
struct RejectDirectoryPublish;
#[async_trait]
impl AtomicPublishPort for RejectDirectoryPublish {
    async fn publish_no_replace(&self, _: &Path, _: &Path) -> Result<(), PublishError> {
        panic!("independent file must not publish a directory")
    }
    async fn publish_into_free_name(&self, _: &Path, _: &Path) -> Result<(), PublishError> {
        panic!("independent file must not publish a directory")
    }
    async fn supports_no_replace(&self, _: &Path) -> bool {
        panic!("independent file must not probe directory publication")
    }
}

pub(super) fn target_facade() -> (Arc<BlobTransferFacade>, Arc<BlobTransport>) {
    let transfer = Arc::new(BlockingBlobTransfer::new());
    let local = Arc::new(FakeLocalStore(transfer.clone()));
    let transport = BlobTransport::new(
        Arc::new(RejectUnexpectedNetwork),
        Arc::new(RejectUnexpectedNetwork),
        Arc::new(RejectUnexpectedNetwork),
        Arc::new(RejectUnexpectedNetwork),
    );
    let facade = Arc::new(BlobTransferFacade::new(BlobTransferDeps {
        downloads: Arc::new(BlobDownloadResources {
            transport: transport.clone(),
            source: Arc::new(BudgetSource(transfer.clone())),
            store: local,
        }),
        hash: Arc::new(TestHash),
        blob_transfer: transfer,
        blob_reference: Arc::new(TestReferences),
        transfer_cipher: Arc::new(TestCipher),
        host_event_emitter: None,
        outbound_progress_reporter: None,
        file_transfer: None,
    }));
    (facade, transport)
}

#[tokio::test]
async fn materializer_target_reservation_waits_for_download_admission_and_real_cancellation() {
    for active in [2, 4] {
        let root = tempfile::tempdir().unwrap();
        let targets = Arc::new(BlockedTarget {
            root: root.path().to_owned(),
            entered: Semaphore::new(0),
            finish: Semaphore::new(0),
        });
        let (facade, transport) = target_facade();
        let materializer = Arc::new(
            FileCacheBlobMaterializer::new(
                facade.clone(),
                root.path().join("cache"),
                Arc::new(RejectDirectoryPublish),
            )
            .with_target_reserver(targets.clone()),
        );
        let cancel = CancellationToken::new();
        let fetch = |index: usize| {
            let materializer = materializer.clone();
            let plan = ReceiveWorkPlan::new(
                DeviceId::new("peer"),
                EntryId::from(format!("receive-{index}")),
                SystemClipboardSnapshot {
                    ts_ms: 0,
                    representations: Vec::new(),
                    file_content_digests: Vec::new(),
                    file_set_v1_component: None,
                },
                vec![V3BlobRef {
                    ticket: BlobTicket::from_bytes(vec![
                        if active == 2 {
                            1
                        } else {
                            (index + 1) as u8
                        };
                        32
                    ]),
                    entry_id: EntryId::from("source"),
                    filename: Some(format!("file-{index}")),
                    mime: None,
                    size_bytes: 5,
                    representation_index: None,
                }],
                None,
                None,
                None,
            )
            .with_cancellation(cancel.clone());
            tokio::spawn(async move { materializer.materialize_plan(plan).await })
        };
        let running = (0..active).map(fetch).collect::<Vec<_>>();
        timeout(
            Duration::from_secs(3),
            targets.entered.acquire_many(active as u32),
        )
        .await
        .unwrap()
        .unwrap()
        .forget();
        let next = fetch(active);
        let escaped = timeout(Duration::from_millis(100), targets.entered.acquire())
            .await
            .is_ok();
        let incomplete = facade
            .cancel_inbound_transfer(
                "receive-0:member:0",
                FileTransferCancellationReason::LocalUser,
                TransferCancellationWait::Bounded,
            )
            .await
            .unwrap_err();
        assert!(matches!(
            incomplete,
            BlobTransferError::CancellationIncomplete(_)
        ));
        assert!(
            !running[0].is_finished(),
            "cancellation must wait for started target preparation"
        );
        assert_eq!(
            targets.entered.available_permits(),
            0,
            "cancelled preparation must still retain its original slot"
        );
        cancel.cancel();
        targets.finish.add_permits(active + 1);
        for task in running.into_iter().chain([next]) {
            assert!(task.await.unwrap().unwrap().is_partial());
        }
        transport
            .shutdown(BlobShutdownWait::Complete)
            .await
            .unwrap();
        assert!(
            !escaped,
            "third same-peer target was prepared before download admission"
        );
        for index in 0..=active {
            assert!(
                !root.path().join(format!("file-{index}")).exists(),
                "cancelled reservation must be cleaned"
            );
        }
    }
}

#[tokio::test(start_paused = true)]
async fn deferred_target_uses_original_deadline_and_preserves_storage_failure() {
    let (facade, transport) = target_facade();
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("reserved");
    let entered = Arc::new(Semaphore::new(0));
    let finish = Arc::new(Semaphore::new(0));
    let cancel = CancellationToken::new();
    let target = {
        let entered = entered.clone();
        let finish = finish.clone();
        let path = path.clone();
        BlobOutputTarget::preparing(Box::pin(async move {
            entered.add_permits(1);
            finish.acquire().await.unwrap().forget();
            std::fs::write(&path, [])?;
            Ok(path)
        }))
    };
    let worker = {
        let facade = facade.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move {
            facade
                .fetch_blob_to_path(FetchBlobToPathCommand {
                    cancellation: cancel,
                    ticket: BlobTicket::from_bytes(vec![1; 32]),
                    entry_id: EntryId::from("source"),
                    target_path: target,
                    transfer_context: None,
                })
                .await
        })
    };
    entered.acquire().await.unwrap().forget();
    advance(Duration::from_secs(59)).await;
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    assert!(!worker.is_finished());
    advance(Duration::from_secs(1)).await;
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    assert!(
        !worker.is_finished(),
        "deadline must wait for actual target preparation"
    );
    finish.add_permits(1);
    let error = worker.await.unwrap().unwrap_err();
    let BlobTransferError::Fetch(error) = error else {
        panic!("expected original deadline");
    };
    assert!(error.chain().any(|source| source
        .downcast_ref::<BlobExchangeError>()
        .is_some_and(|source| source.kind == BlobExchangeFailure::RequestExpired)));
    assert!(
        path.exists(),
        "preparation was really completed before returning"
    );
    std::fs::remove_file(path).unwrap();
    transport
        .shutdown(BlobShutdownWait::Complete)
        .await
        .unwrap();

    let (facade, transport) = target_facade();
    let error = facade
        .fetch_blob_to_path(FetchBlobToPathCommand {
            cancellation: CancellationToken::new(),
            ticket: BlobTicket::from_bytes(vec![1; 32]),
            entry_id: EntryId::from("source"),
            target_path: BlobOutputTarget::preparing(Box::pin(async {
                Err(Error::from(ErrorKind::StorageFull).into())
            })),
            transfer_context: None,
        })
        .await
        .unwrap_err();
    let BlobTransferError::Fetch(error) = error else {
        panic!("expected storage failure");
    };
    assert_eq!(
        error
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
    assert_eq!(stopped.kind, BlobExchangeFailure::Store);
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
