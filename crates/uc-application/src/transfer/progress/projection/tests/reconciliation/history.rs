//! 历史读取必须服从原删除/成员围栏，不能恢复slot或掩盖存储失败。
use super::{Fixture, UsableScope};
use crate::transfer::progress::{
    ProgressHistoryEvidence, ProgressHistoryReadError, ProgressHistoryRepresentation,
    ProgressViewOwner, ReadProgressHistoryPort, TransferProgressQueryError,
};
use async_trait::async_trait;
use std::error::Error;
use std::io;
use std::sync::Arc;
use tokio::sync::{Notify, Semaphore};
use uc_core::clipboard::MimeType;
use uc_core::ids::{DeviceId, EntryId};
use uuid::Uuid;

struct PausedHistory {
    peer: DeviceId,
    entry: EntryId,
    entered: Notify,
    release: Semaphore,
    fail: bool,
}
#[async_trait]
impl ReadProgressHistoryPort for PausedHistory {
    async fn list_owned(
        &self,
        _: DeviceId,
        _: &[DeviceId],
    ) -> Result<Vec<(DeviceId, EntryId)>, ProgressHistoryReadError> {
        Ok(vec![(self.peer, self.entry.clone())])
    }
    async fn read_owned(
        &self,
        _: DeviceId,
        _: DeviceId,
        _: &EntryId,
    ) -> Result<Option<ProgressHistoryEvidence>, ProgressHistoryReadError> {
        self.entered.notify_one();
        self.release.acquire().await.unwrap().forget();
        if self.fail {
            return Err(ProgressHistoryReadError(
                io::Error::new(io::ErrorKind::PermissionDenied, "private storage detail").into(),
            ));
        }
        Ok(Some(ProgressHistoryEvidence {
            file_set: None,
            representations: vec![ProgressHistoryRepresentation {
                mime_type: Some(MimeType("image/png".into())),
                size_bytes: 70_000,
            }],
            sole_paste: true,
        }))
    }
}
#[tokio::test]
async fn historical_get_and_list_cannot_outlive_entry_or_peer_retirement() {
    for list in [false, true] {
        for delete in [false, true] {
            let fixture = Fixture::new(Vec::new(), true).await;
            fixture.registry.remove(fixture.key);
            let source = EntryId::from(Uuid::from_bytes(fixture.key.source.bytes()).to_string());
            let history = Arc::new(PausedHistory {
                peer: fixture.key.peer,
                entry: source.clone(),
                entered: Notify::new(),
                release: Semaphore::new(0),
                fail: false,
            });
            let owner = Arc::new(ProgressViewOwner::new(
                fixture.registry.clone(),
                fixture.reconciler.clone(),
                Arc::new(UsableScope(fixture.key.peer)),
                history.clone(),
                DeviceId::new("local"),
            ));
            let peer = fixture.key.peer;
            let pending = tokio::spawn(async move {
                if list {
                    owner.list().await.map(|views| views.is_empty())
                } else {
                    owner.get(peer, &source).await.map(|view| view.is_none())
                }
            });
            history.entered.notified().await;
            let deletion = if delete {
                Some(fixture.registry.deleting_entry(&history.entry))
            } else {
                fixture.registry.remove_peer(peer);
                None
            };
            history.release.add_permits(1);
            assert!(pending.await.unwrap().unwrap());
            assert!(fixture.registry.snapshots().is_empty());
            drop(deletion);
            fixture.shutdown().await;
        }
    }
}
#[tokio::test]
async fn historical_read_keeps_storage_source_and_does_not_create_a_runtime_task() {
    let fixture = Fixture::new(Vec::new(), true).await;
    fixture.registry.remove(fixture.key);
    let source = EntryId::from(Uuid::from_bytes(fixture.key.source.bytes()).to_string());
    let history = Arc::new(PausedHistory {
        peer: fixture.key.peer,
        entry: source.clone(),
        entered: Notify::new(),
        release: Semaphore::new(1),
        fail: true,
    });
    let owner = ProgressViewOwner::new(
        fixture.registry.clone(),
        fixture.reconciler.clone(),
        Arc::new(UsableScope(fixture.key.peer)),
        history,
        DeviceId::new("local"),
    );
    let error = owner.get(fixture.key.peer, &source).await.unwrap_err();
    assert!(matches!(error, TransferProgressQueryError::Unavailable(_)));
    let mut current: &(dyn Error + 'static) = &error;
    loop {
        if let Some(io) = current.downcast_ref::<io::Error>() {
            assert_eq!(io.kind(), io::ErrorKind::PermissionDenied);
            break;
        }
        current = current.source().expect("complete source chain");
    }
    assert!(fixture.registry.snapshots().is_empty());
    fixture.shutdown().await;
}
