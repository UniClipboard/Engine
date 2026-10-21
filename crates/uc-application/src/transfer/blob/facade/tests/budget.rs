//! 真实获取流程的本地结算与全局／每端调度边界。
use super::super::{
    BatchPosition, BlobTransferDeps, BlobTransferError, BlobTransferFacade, FetchBlobCommand,
    FetchTransferContext,
};
use super::{BlockingBlobTransfer, TestCipher, TestHash, TestReferences};
use crate::facade::host_event::{
    EmitError, HostEvent, HostEventBus, HostEventEmitterPort, TransferHostEvent,
};
use crate::transfer::blob::test_support::{FakeLocalStore, RejectUnexpectedNetwork};
use crate::transfer::blob::transport::{
    BlobDownloadResources, BlobExchangeError, BlobExchangeFailure, BlobShutdownWait, BlobSource,
    BlobSourcePort, BlobTransport,
};
use crate::transfer::receive::batch::ReceiveBatch;
use async_trait::async_trait;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use uc_core::file_transfer::OutboundProgressStatus;
use uc_core::ids::{DeviceId, EntryId};
use uc_core::ports::blob::{BlobTicket, BlobTransferPort};
use uc_core::ports::ReceiveItemRole;

struct BudgetSource(Arc<BlockingBlobTransfer>);
#[async_trait]
impl BlobSourcePort for BudgetSource {
    fn inspect(&self, ticket: &BlobTicket) -> Result<BlobSource, BlobExchangeError> {
        let digest = self.0.digest_of(ticket).map_err(|source| {
            BlobExchangeError::new(BlobExchangeFailure::InvalidRequest, source)
        })?;
        Ok(BlobSource {
            source_key: [digest.as_bytes()[0]; 32],
            digest,
            address_hint: Vec::new(),
        })
    }
    async fn resolve_peer(&self, key: &[u8; 32]) -> Result<DeviceId, BlobExchangeError> {
        Ok(DeviceId::new(format!("budget-peer-{}", key[0])))
    }
}

struct BlockedFinalProgress {
    entered: Semaphore,
    released: Mutex<bool>,
    changed: Condvar,
}
impl BlockedFinalProgress {
    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.changed.notify_all();
    }
}
impl HostEventEmitterPort for BlockedFinalProgress {
    fn emit(&self, event: HostEvent) -> Result<(), EmitError> {
        if matches!(
            event,
            HostEvent::Transfer(TransferHostEvent::Progress {
                bytes_transferred: 5,
                ..
            })
        ) {
            self.entered.add_permits(1);
            let mut released = self.released.lock().unwrap();
            while !*released {
                released = self.changed.wait(released).unwrap();
            }
        }
        Ok(())
    }
}
struct ReleaseFinalProgress(Arc<BlockedFinalProgress>);
impl Drop for ReleaseFinalProgress {
    fn drop(&mut self) {
        self.0.release();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn local_final_progress_keeps_global_and_peer_download_capacity_until_settled() {
    for (grouped, active) in [(false, 2usize), (true, 2), (false, 4), (true, 4)] {
        let progress = Arc::new(BlockedFinalProgress {
            entered: Semaphore::new(0),
            released: Mutex::new(false),
            changed: Condvar::new(),
        });
        let release = ReleaseFinalProgress(progress.clone());
        let bus = Arc::new(HostEventBus::new());
        bus.register("budget-final-progress", progress.clone());
        let transfer = Arc::new(BlockingBlobTransfer::new());
        transfer.release.add_permits(active + 5);
        let local = Arc::new(FakeLocalStore(transfer.clone()));
        let facade = Arc::new(BlobTransferFacade::new(BlobTransferDeps {
            downloads: Arc::new(BlobDownloadResources {
                transport: BlobTransport::new(
                    Arc::new(RejectUnexpectedNetwork),
                    Arc::new(RejectUnexpectedNetwork),
                    Arc::new(RejectUnexpectedNetwork),
                    Arc::new(RejectUnexpectedNetwork),
                ),
                source: Arc::new(BudgetSource(transfer.clone())),
                store: local,
            }),
            hash: Arc::new(TestHash),
            blob_transfer: transfer.clone(),
            blob_reference: Arc::new(TestReferences),
            transfer_cipher: Arc::new(TestCipher),
            host_event_emitter: Some(bus),
            outbound_progress_reporter: None,
            file_transfer: None,
        }));
        let batch = grouped.then(|| Arc::new(ReceiveBatch::default()));
        if let Some(batch) = &batch {
            batch.plan(active + 5, ((active + 5) * 5) as u64);
        }
        let fetch = |index: usize| {
            let facade = facade.clone();
            let batch = batch.clone();
            tokio::spawn(async move {
                facade
                    .fetch_blob(FetchBlobCommand {
                        cancellation: CancellationToken::new(),
                        ticket: BlobTicket::from_bytes(vec![
                            if active == 2 {
                                1
                            } else {
                                (index + 1) as u8
                            };
                            32
                        ]),
                        entry_id: EntryId::from("sender"),
                        transfer_context: Some(FetchTransferContext {
                            item_role: ReceiveItemRole::Representation,
                            batch,
                            transfer_id: format!("item-{index}"),
                            entry_id: format!("entry-{index}"),
                            attempt_id: None,
                            peer_id: "peer".into(),
                            total_bytes: Some(5),
                            filename: String::new(),
                            outbound_transfer_id: None,
                            outbound_target: None,
                            batch_position: BatchPosition::Only,
                            individual_lifecycle: true,
                        }),
                    })
                    .await
            })
        };
        let running = (0..active).map(fetch).collect::<Vec<_>>();
        timeout(
            Duration::from_secs(3),
            progress.entered.acquire_many(active as u32),
        )
        .await
        .unwrap()
        .unwrap()
        .forget();
        assert_eq!(
            transfer.protected.load(Ordering::SeqCst),
            active,
            "content protection was dropped before local settlement"
        );
        transfer
            .started
            .acquire_many(active as u32)
            .await
            .unwrap()
            .forget();
        let next = fetch(active);
        let escaped = timeout(Duration::from_millis(50), transfer.started.acquire())
            .await
            .is_ok();
        release.0.release();
        for task in running {
            task.await.unwrap().unwrap();
        }
        let local_cleanup_escaped = if let Some(batch) = &batch {
            let escaped = timeout(Duration::from_millis(50), transfer.started.acquire())
                .await
                .is_ok();
            batch.item_result("item-0", 5, OutboundProgressStatus::Completed, None);
            escaped
        } else {
            false
        };
        next.await.unwrap().unwrap();
        if let Some(batch) = &batch {
            for index in 1..=active {
                batch.item_result(
                    &format!("item-{index}"),
                    5,
                    OutboundProgressStatus::Completed,
                    None,
                );
            }
            for index in active + 1..active + 5 {
                let result = timeout(Duration::from_secs(3), fetch(index)).await;
                if result.is_err() {
                    batch.finish(false);
                }
                result.unwrap().unwrap().unwrap();
                batch.item_result(
                    &format!("item-{index}"),
                    5,
                    OutboundProgressStatus::Completed,
                    None,
                );
            }
            batch.finish(true);
        }
        assert!(
            !local_cleanup_escaped,
            "fetch returned but parent item had not settled its local work"
        );
        if !grouped {
            while let Ok(entered) = progress.entered.try_acquire() {
                entered.forget();
            }
            *progress.released.lock().unwrap() = false;
            let cached = fetch(active + 5);
            timeout(Duration::from_secs(3), progress.entered.acquire())
                .await
                .unwrap()
                .unwrap()
                .forget();
            if active == 2 {
                let incomplete = facade
                    .transport
                    .shutdown(BlobShutdownWait::Bounded)
                    .await
                    .expect_err("five seconds end only the caller wait");
                assert_eq!(incomplete.kind, BlobExchangeFailure::ShutdownIncomplete);
                assert_eq!(transfer.protected.load(Ordering::SeqCst), 1);
            }
            let mut closing = tokio::spawn(facade.transport.shutdown(BlobShutdownWait::Complete));
            let observed = timeout(Duration::from_millis(50), &mut closing).await;
            let closed_before_local_work = observed.is_ok();
            release.0.release();
            cached.await.unwrap().unwrap();
            match observed {
                Ok(result) => result.unwrap().unwrap(),
                Err(_) => closing.await.unwrap().unwrap(),
            }
            assert!(
                !closed_before_local_work,
                "shutdown reported complete while cached local work retained content and capacity"
            );
        }
        facade
            .transport
            .shutdown(BlobShutdownWait::Complete)
            .await
            .unwrap();
        assert_eq!(transfer.protected.load(Ordering::SeqCst), 0);
        assert!(
            !escaped,
            "another fetch began before local settlement released the active capacity"
        );
    }
}

#[tokio::test]
async fn failed_fetch_keeps_capacity_until_its_parent_finishes_local_cleanup() {
    let transfer = Arc::new(BlockingBlobTransfer::new());
    transfer.fail_reads.store(true, Ordering::SeqCst);
    transfer.release.add_permits(3);
    let local = Arc::new(FakeLocalStore(transfer.clone()));
    let facade = Arc::new(BlobTransferFacade::new(BlobTransferDeps {
        downloads: Arc::new(BlobDownloadResources {
            transport: BlobTransport::new(
                Arc::new(RejectUnexpectedNetwork),
                Arc::new(RejectUnexpectedNetwork),
                Arc::new(RejectUnexpectedNetwork),
                Arc::new(RejectUnexpectedNetwork),
            ),
            source: local.clone(),
            store: local,
        }),
        hash: Arc::new(TestHash),
        blob_transfer: transfer.clone(),
        blob_reference: Arc::new(TestReferences),
        transfer_cipher: Arc::new(TestCipher),
        host_event_emitter: None,
        outbound_progress_reporter: None,
        file_transfer: None,
    }));
    let batch = Arc::new(ReceiveBatch::default());
    batch.plan(3, 15);
    let fetch = |index: usize| {
        let facade = facade.clone();
        let batch = batch.clone();
        tokio::spawn(async move {
            facade
                .fetch_blob(FetchBlobCommand {
                    cancellation: CancellationToken::new(),
                    ticket: BlobTicket::from_bytes(vec![1; 32]),
                    entry_id: EntryId::from("sender"),
                    transfer_context: Some(FetchTransferContext {
                        item_role: ReceiveItemRole::Representation,
                        batch: Some(batch),
                        transfer_id: format!("failed-{index}"),
                        entry_id: "entry".into(),
                        attempt_id: Some("attempt".into()),
                        peer_id: "peer".into(),
                        total_bytes: Some(5),
                        filename: String::new(),
                        outbound_transfer_id: None,
                        outbound_target: None,
                        batch_position: BatchPosition::Only,
                        individual_lifecycle: true,
                    }),
                })
                .await
        })
    };
    let first = fetch(0).await.unwrap().unwrap_err();
    let second = fetch(1).await.unwrap().unwrap_err();
    for error in [&first, &second] {
        let BlobTransferError::Fetch(source) = error else {
            panic!("storage source must survive")
        };
        assert_eq!(
            source
                .chain()
                .find_map(|source| source.downcast_ref::<std::io::Error>())
                .unwrap()
                .kind(),
            std::io::ErrorKind::StorageFull
        );
    }
    assert_eq!(transfer.protected.load(Ordering::SeqCst), 2);
    transfer.started.acquire_many(2).await.unwrap().forget();
    let third = fetch(2);
    let escaped = timeout(Duration::from_millis(50), transfer.started.acquire())
        .await
        .is_ok();
    // 这一步代表完整单项负责人已完成输出清理及日志交接；不是底层 fetch 退出。
    batch.item_result(
        "failed-0",
        0,
        OutboundProgressStatus::Failed,
        Some(first.into()),
    );
    let third = third.await.unwrap().unwrap_err();
    batch.item_result(
        "failed-1",
        0,
        OutboundProgressStatus::Failed,
        Some(second.into()),
    );
    batch.item_result(
        "failed-2",
        0,
        OutboundProgressStatus::Failed,
        Some(third.into()),
    );
    batch.finish(false);
    let stopped = facade
        .transport
        .shutdown(BlobShutdownWait::Complete)
        .await
        .expect_err("original storage failure must remain observable at shutdown");
    assert_eq!(transfer.protected.load(Ordering::SeqCst), 0);
    assert_eq!(stopped.kind, BlobExchangeFailure::Store);
    assert_eq!(
        stopped
            .source
            .chain()
            .find_map(|source| source.downcast_ref::<std::io::Error>())
            .unwrap()
            .kind(),
        std::io::ErrorKind::StorageFull
    );
    assert!(
        !escaped,
        "a failed network worker released capacity before its parent cleanup"
    );
}
