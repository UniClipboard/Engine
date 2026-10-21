use super::DeleteClipboardEntryUseCase;
use crate::transfer::receive::work::{ReceiveWorkError, ReceiveWorkRegistry};
use async_trait::async_trait;
use std::io;
use std::sync::{Arc, Mutex};
use uc_core::clipboard::{ClipboardEntry, ClipboardRepositoryError};
use uc_core::clipboard::{
    ClipboardEvent, ClipboardSelectionDecision, PersistedClipboardRepresentation,
};
use uc_core::ids::{EntryId, EventId, FormatId, RepresentationId};
use uc_core::ports::clipboard::ListRepresentationsForEventPort;
use uc_core::ports::clipboard::{
    DeleteClipboardEntryPort, DeleteClipboardEntryWithReceiveStatePort, GetClipboardEntryPort,
};
use uc_core::ports::{ClipboardEventWriterPort, ClipboardSelectionRepositoryPort};
use uc_core::MimeType;
use url::Url;

struct Store {
    entry: Mutex<Option<ClipboardEntry>>,
    deleted: Mutex<Vec<EventId>>,
    read_reps: Mutex<Vec<EventId>>,
    reps: Mutex<Vec<PersistedClipboardRepresentation>>,
}
#[async_trait]
impl GetClipboardEntryPort for Store {
    async fn get_entry(
        &self,
        _: &EntryId,
    ) -> Result<Option<ClipboardEntry>, ClipboardRepositoryError> {
        Ok(self.entry.lock().unwrap().clone())
    }
}
#[async_trait]
impl DeleteClipboardEntryPort for Store {
    async fn delete_entry(&self, _: &EntryId) -> Result<(), ClipboardRepositoryError> {
        panic!("existing entries must delete receive state atomically")
    }
}
#[async_trait]
impl DeleteClipboardEntryWithReceiveStatePort for Store {
    async fn delete_entry_with_receive_state(
        &self,
        _: &EntryId,
        event: &EventId,
    ) -> Result<(), ClipboardRepositoryError> {
        self.deleted.lock().unwrap().push(event.clone());
        self.entry.lock().unwrap().take();
        Ok(())
    }
}

#[tokio::test]
async fn delete_waits_for_receive_then_cleans_its_latest_event_and_only_managed_files() {
    for explicit_event in [false, true] {
        let cache = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();
        let managed = cache.path().join("newly-saved.bin");
        let user_file = user.path().join("kept.bin");
        let work = Arc::new(ReceiveWorkRegistry::default());
        let entry = EntryId::from("entry");
        let old_event = EventId::new();
        let new_event = EventId::new();
        let store = Arc::new(Store {
            entry: Mutex::new(if explicit_event {
                Some(ClipboardEntry::new(entry.clone(), old_event.clone(), 1, 1))
            } else {
                None
            }),
            deleted: Mutex::new(Vec::new()),
            read_reps: Mutex::new(Vec::new()),
            reps: Mutex::new(Vec::new()),
        });
        let action = work.action();
        action.register(&entry, "attempt").unwrap();
        let late = work.action();
        let deletion = Arc::new(
            deletion(store.clone(), work.clone()).with_file_cache_dir(cache.path().to_owned()),
        );
        let deleting = {
            let entry = entry.clone();
            tokio::spawn(async move { deletion.execute(&entry).await })
        };
        action.cancellation().cancelled().await;
        assert!(store.deleted.lock().unwrap().is_empty());
        *store.entry.lock().unwrap() =
            Some(ClipboardEntry::new(entry.clone(), new_event.clone(), 1, 1));
        std::fs::write(&managed, b"managed").unwrap();
        std::fs::write(&user_file, b"user-owned").unwrap();
        let uris = format!(
            "{}\r\n{}\r\n",
            Url::from_file_path(&managed).unwrap(),
            Url::from_file_path(&user_file).unwrap()
        );
        *store.reps.lock().unwrap() = vec![PersistedClipboardRepresentation::new(
            RepresentationId::new(),
            FormatId::from("files"),
            Some(MimeType("text/uri-list".into())),
            uris.len() as i64,
            Some(uris.into_bytes()),
            None,
        )];
        action.finish(Ok(()));
        deleting.await.unwrap().unwrap();
        assert_eq!(*store.deleted.lock().unwrap(), vec![new_event.clone()]);
        assert_eq!(*store.read_reps.lock().unwrap(), vec![new_event]);
        assert!(!managed.exists());
        assert_eq!(std::fs::read(&user_file).unwrap(), b"user-owned");
        assert!(store.entry.lock().unwrap().is_none());
        assert!(late.register(&entry, "late").is_err());
    }
}

#[tokio::test]
async fn deletion_preserves_actual_stop_failure_and_does_not_delete_after_it() {
    let work = Arc::new(ReceiveWorkRegistry::default());
    let entry = EntryId::from("entry");
    let store = Arc::new(Store {
        entry: Mutex::new(Some(ClipboardEntry::new(
            entry.clone(),
            EventId::new(),
            1,
            1,
        ))),
        deleted: Mutex::new(Vec::new()),
        read_reps: Mutex::new(Vec::new()),
        reps: Mutex::new(Vec::new()),
    });
    let action = work.action();
    action.register(&entry, "attempt").unwrap();
    let deletion = deletion(store.clone(), work);
    let deleting = tokio::spawn(async move { deletion.execute(&entry).await });
    action.cancellation().cancelled().await;
    action.finish(Err(ReceiveWorkError::Action(
        io::Error::from(io::ErrorKind::PermissionDenied).into(),
    )));
    let error = deleting.await.unwrap().unwrap_err();
    let mut next: Option<&(dyn std::error::Error + 'static)> = Some(error.as_ref());
    let mut found = false;
    while let Some(source) = next {
        found |= source
            .downcast_ref::<io::Error>()
            .is_some_and(|source| source.kind() == io::ErrorKind::PermissionDenied);
        next = source.source();
    }
    assert!(found);
    assert!(store.deleted.lock().unwrap().is_empty());
    assert!(store.entry.lock().unwrap().is_some());
}

fn deletion(store: Arc<Store>, work: Arc<ReceiveWorkRegistry>) -> DeleteClipboardEntryUseCase {
    DeleteClipboardEntryUseCase::from_ports(
        store.clone(),
        store.clone(),
        store.clone(),
        store.clone(),
        store.clone(),
    )
    .with_receive_state_delete(store)
    .with_receive_work(work)
}
#[async_trait]
impl ListRepresentationsForEventPort for Store {
    async fn get_representations_for_event(
        &self,
        event: &EventId,
    ) -> Result<Vec<PersistedClipboardRepresentation>, ClipboardRepositoryError> {
        assert_eq!(
            &self.entry.lock().unwrap().as_ref().unwrap().event_id,
            event
        );
        self.read_reps.lock().unwrap().push(event.clone());
        Ok(self.reps.lock().unwrap().clone())
    }
}
#[async_trait]
impl ClipboardSelectionRepositoryPort for Store {
    async fn get_selection(
        &self,
        _: &EntryId,
    ) -> anyhow::Result<Option<ClipboardSelectionDecision>> {
        panic!("unused selection lookup")
    }
    async fn delete_selection(&self, _: &EntryId) -> anyhow::Result<()> {
        panic!("must use atomic delete")
    }
}
#[async_trait]
impl ClipboardEventWriterPort for Store {
    async fn insert_event(
        &self,
        _: &ClipboardEvent,
        _: &Vec<PersistedClipboardRepresentation>,
    ) -> anyhow::Result<()> {
        panic!("unused event insertion")
    }
    async fn delete_event_and_representations(&self, _: &EventId) -> anyhow::Result<()> {
        panic!("must use atomic delete")
    }
}
