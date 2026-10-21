#[cfg(unix)]
use super::cache_files::CacheOwnershipRefusal;
use super::DeleteClipboardEntryUseCase;
use crate::transfer::receive::work::{ReceiveWorkError, ReceiveWorkRegistry};
use async_trait::async_trait;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use uc_core::clipboard::{
    ClipboardEntry, ClipboardRepositoryError, PersistedClipboardRepresentation,
};
use uc_core::ids::{EntryId, EventId, FormatId, RepresentationId};
use uc_core::ports::clipboard::{
    DeleteClipboardEntryWithReceiveStatePort, GetClipboardEntryPort,
    ListRepresentationsForEventPort,
};
use uc_core::MimeType;
use url::Url;

struct Store {
    entry: Mutex<Option<ClipboardEntry>>,
    receive: AtomicBool,
    read_error: AtomicBool,
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
impl DeleteClipboardEntryWithReceiveStatePort for Store {
    async fn delete_entry_with_receive_state(
        &self,
        _: &EntryId,
    ) -> Result<bool, ClipboardRepositoryError> {
        let entry = self.entry.lock().unwrap().take();
        let had_entry = entry.is_some();
        if let Some(entry) = entry {
            self.deleted.lock().unwrap().push(entry.event_id);
        }
        Ok(self.receive.swap(false, Ordering::SeqCst) || had_entry)
    }
}

#[tokio::test]
async fn deleting_an_unsaved_receive_uses_atomic_state_deletion_after_actual_stop() {
    for active in [false, true] {
        let work = Arc::new(ReceiveWorkRegistry::default());
        let entry = EntryId::from("pending-entry");
        let store = Arc::new(Store {
            receive: AtomicBool::new(true),
            read_error: AtomicBool::new(false),
            entry: Mutex::new(None),
            deleted: Mutex::new(Vec::new()),
            read_reps: Mutex::new(Vec::new()),
            reps: Mutex::new(Vec::new()),
        });
        let action = work.action();
        if active {
            action.register(&entry, "attempt").unwrap();
        }
        let deletion = deletion(store.clone(), work);
        let deleting = tokio::spawn(async move { deletion.execute(&entry).await });
        if active {
            action.cancellation().cancelled().await;
            action.finish(Ok(()));
        }
        deleting.await.unwrap().unwrap();
        assert!(store.read_reps.lock().unwrap().is_empty());
        assert!(!store.receive.load(Ordering::SeqCst));
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
            receive: AtomicBool::new(true),
            read_error: AtomicBool::new(false),
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
            let deletion = deletion.clone();
            tokio::spawn(async move { deletion.execute(&entry).await })
        };
        action.cancellation().cancelled().await;
        assert!(store.deleted.lock().unwrap().is_empty());
        *store.entry.lock().unwrap() =
            Some(ClipboardEntry::new(entry.clone(), new_event.clone(), 1, 1));
        std::fs::write(&managed, b"managed").unwrap();
        std::fs::write(&user_file, b"user-owned").unwrap();
        let uris = format!(
            "{}\r\n{}\r\n{}\r\nfile://history-file-host.example/shared/kept.bin\r\n",
            Url::from_file_path(&managed).unwrap(),
            Url::from_file_path(&user_file).unwrap(),
            Url::from_file_path(cache.path().join("already-missing.bin")).unwrap()
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
        let missing = deletion.execute(&entry).await.unwrap_err();
        assert!(matches!(
            missing.downcast_ref::<ClipboardRepositoryError>(),
            Some(ClipboardRepositoryError::NotFound(_))
        ));
    }
}

#[tokio::test]
async fn deletion_preserves_actual_stop_failure_and_does_not_delete_after_it() {
    let work = Arc::new(ReceiveWorkRegistry::default());
    let entry = EntryId::from("entry");
    let store = Arc::new(Store {
        receive: AtomicBool::new(true),
        read_error: AtomicBool::new(false),
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
    DeleteClipboardEntryUseCase::from_ports(store.clone(), store.clone(), store, work)
}
#[async_trait]
impl ListRepresentationsForEventPort for Store {
    async fn get_representations_for_event(
        &self,
        event: &EventId,
    ) -> Result<Vec<PersistedClipboardRepresentation>, ClipboardRepositoryError> {
        if self.read_error.load(Ordering::SeqCst) {
            return Err(ClipboardRepositoryError::Storage(
                io::Error::from(io::ErrorKind::PermissionDenied).into(),
            ));
        }
        assert_eq!(
            &self.entry.lock().unwrap().as_ref().unwrap().event_id,
            event
        );
        self.read_reps.lock().unwrap().push(event.clone());
        Ok(self.reps.lock().unwrap().clone())
    }
}

#[tokio::test]
async fn deletion_keeps_record_and_source_when_cleanup_representations_cannot_be_read() {
    let entry = EntryId::from("entry");
    let store = Arc::new(Store {
        entry: Mutex::new(Some(ClipboardEntry::new(
            entry.clone(),
            EventId::new(),
            1,
            1,
        ))),
        receive: AtomicBool::new(true),
        read_error: AtomicBool::new(true),
        deleted: Mutex::new(Vec::new()),
        read_reps: Mutex::new(Vec::new()),
        reps: Mutex::new(Vec::new()),
    });
    let error = deletion(store.clone(), Arc::new(ReceiveWorkRegistry::default()))
        .execute(&entry)
        .await
        .unwrap_err();
    assert_eq!(
        error
            .chain()
            .find_map(|source| source.downcast_ref::<io::Error>())
            .unwrap()
            .kind(),
        io::ErrorKind::PermissionDenied
    );
    assert!(store.entry.lock().unwrap().is_some());
    assert!(store.receive.load(Ordering::SeqCst));
    assert!(store.deleted.lock().unwrap().is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn deletion_refuses_cache_parent_escape_and_symlink_before_changing_records() {
    for through_symlink in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let cache = root.path().join("cache");
        let user = root.path().join("user");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::create_dir_all(&user).unwrap();
        let user_file = user.join("kept.bin");
        std::fs::write(&user_file, b"user-owned").unwrap();
        let reference = if through_symlink {
            let alias = cache.join("alias");
            std::os::unix::fs::symlink(&user, &alias).unwrap();
            Url::from_file_path(alias.join("kept.bin"))
                .unwrap()
                .to_string()
        } else {
            // 历史表示允许原始路径；不能只以词法前缀认定归属。
            cache
                .join("..")
                .join("user/kept.bin")
                .to_string_lossy()
                .into_owned()
        };
        let managed = cache.join("managed.bin");
        std::fs::write(&managed, b"managed").unwrap();
        let reference = format!(
            "{}\n{}\n",
            Url::from_file_path(&managed).unwrap(),
            reference
        );
        let entry = EntryId::from("entry");
        let store = Arc::new(Store {
            entry: Mutex::new(Some(ClipboardEntry::new(
                entry.clone(),
                EventId::new(),
                1,
                1,
            ))),
            receive: AtomicBool::new(true),
            read_error: AtomicBool::new(false),
            deleted: Mutex::new(Vec::new()),
            read_reps: Mutex::new(Vec::new()),
            reps: Mutex::new(vec![PersistedClipboardRepresentation::new(
                RepresentationId::new(),
                FormatId::from("files"),
                Some(MimeType("text/uri-list".into())),
                reference.len() as i64,
                Some(reference.into_bytes()),
                None,
            )]),
        });
        let outcome = deletion(store.clone(), Arc::new(ReceiveWorkRegistry::default()))
            .with_file_cache_dir(cache)
            .execute(&entry)
            .await;
        assert_eq!(std::fs::read(&user_file).unwrap(), b"user-owned");
        let failure = outcome.unwrap_err();
        assert!(matches!(
            failure.downcast_ref::<CacheOwnershipRefusal>(),
            Some(CacheOwnershipRefusal::ParentTraversal | CacheOwnershipRefusal::SymbolicLink)
        ));
        assert_eq!(std::fs::read(&managed).unwrap(), b"managed");
        assert!(store.entry.lock().unwrap().is_some());
        assert!(store.receive.load(Ordering::SeqCst));
        assert!(store.deleted.lock().unwrap().is_empty());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn deleting_through_the_configured_cache_root_alias_keeps_the_root_itself() {
    let root = tempfile::tempdir().unwrap();
    let cache = root.path().join("cache");
    std::fs::create_dir(&cache).unwrap();
    let alias = root.path().join("configured-cache");
    std::os::unix::fs::symlink(&cache, &alias).unwrap();
    let file = cache.join("received.bin");
    std::fs::write(&file, b"managed").unwrap();
    let reference = Url::from_file_path(alias.join("received.bin"))
        .unwrap()
        .to_string();
    let entry = EntryId::from("entry");
    let store = Arc::new(Store {
        entry: Mutex::new(Some(ClipboardEntry::new(
            entry.clone(),
            EventId::new(),
            1,
            1,
        ))),
        receive: AtomicBool::new(true),
        read_error: AtomicBool::new(false),
        deleted: Mutex::new(Vec::new()),
        read_reps: Mutex::new(Vec::new()),
        reps: Mutex::new(vec![PersistedClipboardRepresentation::new(
            RepresentationId::new(),
            FormatId::from("files"),
            Some(MimeType("text/uri-list".into())),
            reference.len() as i64,
            Some(reference.into_bytes()),
            None,
        )]),
    });
    deletion(store.clone(), Arc::new(ReceiveWorkRegistry::default()))
        .with_file_cache_dir(alias.clone())
        .execute(&entry)
        .await
        .unwrap();
    assert!(!file.exists());
    assert!(cache.is_dir() && alias.is_dir());
    assert!(store.entry.lock().unwrap().is_none());
}
