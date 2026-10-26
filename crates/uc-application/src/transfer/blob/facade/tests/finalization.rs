//! 最终持久化失败必须进入原任务和关闭报告，不能只记录诊断后伪报成功。
use super::super::finalization::FetchFinalization;
use super::super::preparation::FetchPreparationState;
use super::super::{BatchPosition, FetchTerminalFailure, FetchTransferContext};
use super::targets::target_facade;
use crate::transfer::blob::fetch_failure_reason;
use crate::transfer::blob::transport::{
    BlobExchangeError, BlobExchangeFailure, BlobPreparationStop, BlobShutdownWait,
};
use crate::transfer::file::facade::{BeginReceiverTransfer, ReceiverTransferRegistration};
use crate::transfer::file::session::ReceiverTransferHandle;
use crate::transfer::file::FileTransferApplicationError;
use async_trait::async_trait;
use std::io::{Error, ErrorKind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use tokio_util::sync::CancellationToken;
use uc_core::file_transfer::{
    FileTransferCancellationReason, FileTransferEvent, FileTransferEventPublisherPort,
    FileTransferEventStorePort, FileTransferFailureReason,
};
use uc_core::ports::{ReceiveItemRole, TrackedFileTransferStatus};

struct FailedTerminalStore {
    blocked: AtomicBool,
}
#[async_trait]
impl FileTransferEventStorePort for FailedTerminalStore {
    async fn load(&self, _: &str) -> anyhow::Result<Vec<FileTransferEvent>> {
        panic!("terminal settlement must not reload history")
    }
    async fn append(&self, event: FileTransferEvent) -> anyhow::Result<()> {
        assert!(matches!(
            event,
            FileTransferEvent::Completed { .. }
                | FileTransferEvent::Cancelled { .. }
                | FileTransferEvent::Failed { .. }
        ));
        if self.blocked.load(Ordering::SeqCst) {
            Err(Error::from(ErrorKind::StorageFull).into())
        } else {
            Ok(())
        }
    }
}
struct SettledTerminalPublish;
#[async_trait]
impl FileTransferEventPublisherPort for SettledTerminalPublish {
    async fn publish(&self, _: FileTransferEvent) -> anyhow::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn terminal_write_failures_preserve_success_failure_and_cancel_settlement_sources() {
    for scenario in 0..4 {
        let (_facade, transport) = target_facade();
        let state = Arc::new(FetchPreparationState::default());
        let store = Arc::new(FailedTerminalStore {
            blocked: AtomicBool::new(true),
        });
        let session = Arc::new(ReceiverTransferHandle::new(
            BeginReceiverTransfer {
                item_role: Some(ReceiveItemRole::Representation),
                transfer_id: "item".into(),
                peer_id: "peer".into(),
                filename: String::new(),
                file_size: Some(5),
                registration: ReceiverTransferRegistration::Entry {
                    entry_id: "entry".into(),
                    attempt_id: None,
                    cached_path: String::new(),
                },
            },
            store.clone(),
            Arc::new(SettledTerminalPublish),
            Weak::new(),
            Some(0),
            true,
        ));
        let preparation_failure = if scenario == 3 {
            state.stop_attempted.store(true, Ordering::SeqCst);
            *state.stop.lock().unwrap() = Some(BlobPreparationStop::Cancelled(
                FileTransferCancellationReason::LocalUser,
            ));
            Some(
                session
                    .cancel(FileTransferCancellationReason::LocalUser)
                    .await
                    .unwrap_err(),
            )
        } else {
            None
        };
        *state.session.lock().unwrap() = Some(session);
        let cancel = CancellationToken::new();
        if scenario == 2 || scenario == 3 {
            cancel.cancel();
        }
        let finishing = FetchFinalization {
            prepared: state,
            cancel: cancel.clone(),
            settlement: Default::default(),
            context: Some(FetchTransferContext {
                item_role: ReceiveItemRole::Representation,
                batch: None,
                transfer_id: "item".into(),
                entry_id: "entry".into(),
                attempt_id: None,
                peer_id: "peer".into(),
                total_bytes: Some(5),
                filename: String::new(),
                batch_position: BatchPosition::Only,
                individual_lifecycle: true,
            }),
        };
        let result = if let Some(source) = preparation_failure {
            Err(BlobExchangeError::new(
                BlobExchangeFailure::Settlement,
                source,
            ))
        } else if scenario == 1 {
            Err(BlobExchangeError::new(
                BlobExchangeFailure::Network,
                Error::from(ErrorKind::ConnectionReset),
            ))
        } else {
            Ok(((), 5))
        };
        let job = transport.supervise(cancel, move |response| async move {
            finishing.finish(result, response).await
        });
        let completion = job.completion();
        let error = job.await.unwrap_err();
        assert!(
            completion.borrow().is_none(),
            "首次错误不能充当实际交接完成"
        );
        store.blocked.store(false, Ordering::SeqCst);
        assert_eq!(error.kind, BlobExchangeFailure::Settlement);
        let marker = error
            .source
            .chain()
            .find_map(|source| source.downcast_ref::<FetchTerminalFailure>())
            .unwrap()
            .clone();
        if scenario >= 2 {
            assert_eq!(marker.status, TrackedFileTransferStatus::Cancelled);
        }
        let report = &marker.source;
        let terminal = if scenario == 0 {
            report.as_ref()
        } else {
            &report.additional[0]
        };
        assert!(terminal
            .source
            .chain()
            .any(|source| source.is::<FileTransferApplicationError>()));
        assert_eq!(
            terminal
                .source
                .chain()
                .find_map(|source| source.downcast_ref::<Error>())
                .unwrap()
                .kind(),
            ErrorKind::StorageFull
        );
        if scenario == 1 {
            assert_eq!(
                report
                    .source
                    .chain()
                    .find_map(|source| source.downcast_ref::<Error>())
                    .unwrap()
                    .kind(),
                ErrorKind::ConnectionReset
            );
        } else if scenario == 2 {
            assert!(
                !error.settled_cancellation(),
                "failed cancellation persistence is not a successful stop"
            );
            assert!(report
                .source
                .chain()
                .any(|source| source.downcast_ref::<BlobExchangeFailure>()
                    == Some(&BlobExchangeFailure::Cancelled)));
        }
        if scenario == 0 {
            let returned = anyhow::Error::new(super::super::BlobTransferError::Fetch(error.into()));
            assert_eq!(
                fetch_failure_reason(&returned),
                FileTransferFailureReason::StorageUnavailable,
                "final terminal storage failure must not become unknown"
            );
        }
        assert_eq!(
            transport
                .shutdown(BlobShutdownWait::Complete)
                .await
                .unwrap_err()
                .kind,
            BlobExchangeFailure::Settlement
        );
        assert!(marker.is_settled(), "完成屏障必须等待原交接真正修复");
    }
}

#[derive(Default)]
struct RecordedTerminalStore(std::sync::Mutex<Vec<FileTransferEvent>>);
#[async_trait]
impl FileTransferEventStorePort for RecordedTerminalStore {
    async fn load(&self, _: &str) -> anyhow::Result<Vec<FileTransferEvent>> {
        panic!("terminal settlement must not reload history")
    }
    async fn append(&self, event: FileTransferEvent) -> anyhow::Result<()> {
        self.0.lock().unwrap().push(event);
        Ok(())
    }
}
struct BlockedTerminalPublisher {
    entered: tokio::sync::Semaphore,
    finish: tokio::sync::Semaphore,
}
#[async_trait]
impl FileTransferEventPublisherPort for BlockedTerminalPublisher {
    async fn publish(&self, event: FileTransferEvent) -> anyhow::Result<()> {
        assert!(matches!(event, FileTransferEvent::Completed { .. }));
        self.entered.add_permits(1);
        self.finish.acquire().await.unwrap().forget();
        Ok(())
    }
}

#[tokio::test]
async fn persisted_terminal_wins_late_cancel_but_shutdown_waits_for_real_event_handoff() {
    let (_facade, transport) = target_facade();
    let state = Arc::new(FetchPreparationState::default());
    let store = Arc::new(RecordedTerminalStore::default());
    let publisher = Arc::new(BlockedTerminalPublisher {
        entered: tokio::sync::Semaphore::new(0),
        finish: tokio::sync::Semaphore::new(0),
    });
    *state.session.lock().unwrap() = Some(Arc::new(ReceiverTransferHandle::new(
        BeginReceiverTransfer {
            item_role: Some(ReceiveItemRole::Representation),
            transfer_id: "item".into(),
            peer_id: "peer".into(),
            filename: String::new(),
            file_size: Some(5),
            registration: ReceiverTransferRegistration::Entry {
                entry_id: "entry".into(),
                attempt_id: None,
                cached_path: String::new(),
            },
        },
        store.clone(),
        publisher.clone(),
        Weak::new(),
        Some(0),
        true,
    )));
    let cancel = CancellationToken::new();
    let finishing = FetchFinalization {
        prepared: state,
        cancel: cancel.clone(),
        settlement: Default::default(),
        context: Some(FetchTransferContext {
            item_role: ReceiveItemRole::Representation,
            batch: None,
            transfer_id: "item".into(),
            entry_id: "entry".into(),
            attempt_id: None,
            peer_id: "peer".into(),
            total_bytes: Some(5),
            filename: String::new(),
            batch_position: BatchPosition::Only,
            individual_lifecycle: true,
        }),
    };
    let job = transport.supervise(cancel.clone(), move |response| async move {
        finishing.finish(Ok(((), 5)), response).await
    });
    let completion = job.completion();
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        publisher.entered.acquire(),
    )
    .await
    .unwrap()
    .unwrap()
    .forget();
    assert!(matches!(
        store.0.lock().unwrap().as_slice(),
        [FileTransferEvent::Completed { .. }]
    ));
    drop(job);
    assert!(cancel.is_cancelled());
    assert_eq!(
        transport
            .shutdown(BlobShutdownWait::Bounded)
            .await
            .unwrap_err()
            .kind,
        BlobExchangeFailure::ShutdownIncomplete
    );
    assert!(
        completion.borrow().is_none(),
        "abandoning the waiter must not abandon an already-started event handoff"
    );
    publisher.finish.add_permits(1);
    transport
        .shutdown(BlobShutdownWait::Complete)
        .await
        .unwrap();
    assert!(
        completion.borrow().as_ref().unwrap().is_ok(),
        "persisted completion remains authoritative after late cancel"
    );
    assert_eq!(
        store.0.lock().unwrap().len(),
        1,
        "late cancel must not append a conflicting terminal event"
    );
}
