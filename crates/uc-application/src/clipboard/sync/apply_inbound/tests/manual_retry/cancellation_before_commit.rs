//! 原Facade结果交接屏障：取消先于提交时只撤回本项，不阻止同批后项保存。
//! 实际许可和文件输出运行，保存端沿用明确替身。
use super::*;
use crate::facade::blob_transfer::{
    BlobTransferFacade, FetchBlobCommand, FetchBlobResult, FetchBlobToPathCommand,
};
use crate::transfer::blob::facade::{InboundCancelOutcome, TransferCancellationWait};
use crate::transfer::blob::test_support::local_file_probe;
use crate::transfer::blob::transport::BlobShutdownWait;
use crate::transfer::receive::materialization::InboundBlobFetcher;
use std::time::Duration;
use tokio::time::timeout;

struct BeforeCommit {
    inner: Arc<BlobTransferFacade>,
    entered: Notify,
    release: Semaphore,
    first: AtomicBool,
}
#[async_trait]
impl InboundBlobFetcher for BeforeCommit {
    async fn fetch_blob(&self, _: FetchBlobCommand) -> Result<FetchBlobResult> {
        panic!("independent files must not use memory hydration")
    }
    async fn fetch_blob_to_path(
        &self,
        command: FetchBlobToPathCommand,
    ) -> Result<FetchBlobToPathResult> {
        let result = self.inner.fetch_blob_to_path(command).await?;
        if self.first.swap(false, Ordering::SeqCst) {
            self.entered.notify_one();
            self.release.acquire().await.unwrap().forget();
        }
        Ok(result)
    }
}

#[tokio::test]
async fn single_file_cancel_before_commit_refuses_save_and_preserves_sibling_delivery() {
    let fixture = Fixture::new(0).await;
    let (facade, transport, protected) = local_file_probe(None);
    let bridge = Arc::new(BeforeCommit {
        inner: facade.clone(),
        entered: Notify::new(),
        release: Semaphore::new(0),
        first: AtomicBool::new(true),
    });
    let apply = Arc::new(
        fixture
            .apply
            .as_ref()
            .clone()
            .with_blob_materializer(Arc::new(test_materializer(
                bridge.clone(),
                fixture._cache.path().to_owned(),
            ))),
    );
    let (mut prepared, accepted) = fixture.prepared();
    let selection = prepared.selection.as_mut().unwrap();
    selection.selected = HashSet::from([1, 2]);
    selection.unselected.clear();
    let caller = {
        let apply = apply.clone();
        tokio::spawn(async move { apply.execute_retry(prepared).await })
    };
    timeout(Duration::from_secs(5), bridge.entered.notified())
        .await
        .unwrap();
    let RetryEntryReceiveOutcome::Accepted { attempt_id } = accepted.await.unwrap() else {
        panic!("not accepted")
    };
    assert_eq!(fixture.capture.progress_calls.load(Ordering::SeqCst), 0);
    let cancelled_path = std::fs::read_dir(fixture.saved.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path != &fixture.saved)
        .unwrap();
    assert_eq!(std::fs::read(&cancelled_path).unwrap(), b"retried");
    let transfer = format!("receiver:attempt:{attempt_id}:member:1");
    let waiting = facade
        .cancel_inbound_transfer(
            &transfer,
            FileTransferCancellationReason::LocalUser,
            TransferCancellationWait::Bounded,
        )
        .await;
    assert!(matches!(
        waiting,
        Err(BlobTransferError::CancellationIncomplete(_))
    ));
    assert_eq!(protected.load(Ordering::SeqCst), 1);
    bridge.release.add_permits(1);
    caller.await.unwrap().unwrap();
    assert!(matches!(
        facade
            .cancel_inbound_transfer(
                &transfer,
                FileTransferCancellationReason::LocalUser,
                TransferCancellationWait::Complete
            )
            .await
            .unwrap(),
        InboundCancelOutcome::Cancelled
    ));
    assert!(!cancelled_path.exists());
    assert_eq!(
        fixture.capture.progress_calls.load(Ordering::SeqCst),
        1,
        "只保存未取消的后项"
    );
    let (_, landed) = fixture
        .work
        .retry_material(&EntryId::from("receiver"), &attempt_id)
        .unwrap();
    assert_eq!(
        landed.keys().copied().collect::<HashSet<_>>(),
        HashSet::from([0, 2])
    );
    for item in landed.values() {
        if let ReceivedOutput::File(artifact) = &item.output {
            assert!(artifact.final_path.is_file());
        }
    }
    assert_eq!(std::fs::read(&fixture.saved).unwrap(), b"success");
    assert_eq!(protected.load(Ordering::SeqCst), 0);
    apply.shutdown().await.unwrap();
    transport
        .shutdown(BlobShutdownWait::Complete)
        .await
        .unwrap();
}
