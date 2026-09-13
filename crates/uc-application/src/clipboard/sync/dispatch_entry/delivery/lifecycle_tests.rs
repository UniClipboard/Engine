use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{oneshot, Notify};
use tokio::task::JoinSet;
use tokio::time::timeout;
use uc_core::clipboard::{EntryDeliveryError, EntryDeliveryRecord, EntryDeliveryStatus};
use uc_core::ids::{DeviceId, EntryId};
use uc_core::ports::{DispatchAck, EntryDeliveryRepositoryPort};

use super::super::super::work::WorkOwner;
use super::super::test_support::FixedClock;
use super::{spawn_deferred_drain, DeliveryRecorder};
use crate::facade::HostEventBus;

struct HeldRecorder {
    entered: Notify,
    release: Mutex<Option<oneshot::Receiver<()>>>,
    written: AtomicBool,
}

#[async_trait]
impl EntryDeliveryRepositoryPort for HeldRecorder {
    async fn record_attempt(
        &self,
        _record: &EntryDeliveryRecord,
    ) -> Result<(), EntryDeliveryError> {
        let release = self.release.lock().unwrap().take().unwrap();
        let disk = tokio::task::spawn_blocking(move || release.blocking_recv().unwrap());
        self.entered.notify_one();
        disk.await.unwrap();
        self.written.store(true, Ordering::SeqCst);
        Ok(())
    }

    async fn list_by_entry(
        &self,
        _id: &EntryId,
    ) -> Result<Vec<EntryDeliveryRecord>, EntryDeliveryError> {
        Ok(Vec::new())
    }
}

#[tokio::test]
async fn abandoned_foreground_waiter_keeps_the_record_in_the_shutdown_drain() {
    let (release, wait) = oneshot::channel();
    let repository = Arc::new(HeldRecorder {
        entered: Notify::new(),
        release: Mutex::new(Some(wait)),
        written: AtomicBool::new(false),
    });
    let recorder = Arc::new(DeliveryRecorder::new(
        repository.clone(),
        Arc::new(HostEventBus::new()),
    ));
    let owner = WorkOwner::default();
    let work = owner.begin().unwrap();
    let waiter = tokio::spawn(async move {
        recorder
            .flush_owned(
                work,
                vec![EntryDeliveryRecord {
                    entry_id: EntryId::from("entry"),
                    target_device_id: DeviceId::new("peer"),
                    status: EntryDeliveryStatus::Delivered,
                    reason_detail: None,
                    updated_at_ms: 0,
                }],
            )
            .await;
    });
    repository.entered.notified().await;
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    let early = timeout(Duration::from_millis(20), owner.shutdown()).await;
    release.send(()).unwrap();
    assert!(early.is_err());
    owner.shutdown().await.unwrap();
    assert!(repository.written.load(Ordering::SeqCst));
}

#[tokio::test]
async fn abandoned_shutdown_keeps_the_real_deferred_write_in_the_drain() {
    let (release, wait) = oneshot::channel();
    let repository = Arc::new(HeldRecorder {
        entered: Notify::new(),
        release: Mutex::new(Some(wait)),
        written: AtomicBool::new(false),
    });
    let recorder = Arc::new(DeliveryRecorder::new(
        repository.clone(),
        Arc::new(HostEventBus::new()),
    ));
    let owner = WorkOwner::default();
    let mut peers = JoinSet::new();
    peers.spawn(async { (DeviceId::new("peer"), Ok(DispatchAck::Accepted)) });
    spawn_deferred_drain(
        owner.begin().unwrap(),
        peers,
        Some(EntryId::from("entry")),
        Arc::new(FixedClock(0)),
        recorder,
        "test".into(),
    );
    repository.entered.notified().await;
    assert!(timeout(Duration::from_millis(10), owner.shutdown())
        .await
        .is_err());
    let mut retry = Box::pin(owner.shutdown());
    let premature = timeout(Duration::from_millis(10), retry.as_mut()).await;
    release.send(()).unwrap();
    assert!(premature.is_err());
    retry.await.unwrap();
    assert!(repository.written.load(Ordering::SeqCst));
}
