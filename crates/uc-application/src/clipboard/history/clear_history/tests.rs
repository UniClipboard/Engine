use super::{ClearClipboardHistoryUseCase, ClearHistoryFailure, HistoryDeletionTargetsPort};
use crate::clipboard::history::delete_entry::DeleteClipboardEntryUseCase;
use crate::transfer::receive::work::ReceiveWorkRegistry;
use anyhow::Result;
use async_trait::async_trait;
use std::io;
use std::sync::Arc;
use uc_core::clipboard::{
    ClipboardEntry, ClipboardRepositoryError, PersistedClipboardRepresentation,
};
use uc_core::ids::{EntryId, EventId};
use uc_core::ports::clipboard::{
    DeleteClipboardEntryWithReceiveStatePort, GetClipboardEntryPort,
    ListRepresentationsForEventPort,
};

struct Targets {
    second_succeeds: bool,
}
#[async_trait]
impl HistoryDeletionTargetsPort for Targets {
    async fn history_deletion_targets(&self) -> Result<Vec<EntryId>> {
        Ok(vec![EntryId::from("first"), EntryId::from("second")])
    }
}
#[async_trait]
impl GetClipboardEntryPort for Targets {
    async fn get_entry(
        &self,
        _: &EntryId,
    ) -> Result<Option<ClipboardEntry>, ClipboardRepositoryError> {
        Ok(None)
    }
}
#[async_trait]
impl ListRepresentationsForEventPort for Targets {
    async fn get_representations_for_event(
        &self,
        _: &EventId,
    ) -> Result<Vec<PersistedClipboardRepresentation>, ClipboardRepositoryError> {
        panic!("无保存条目不能读取虚构的event")
    }
}
#[async_trait]
impl DeleteClipboardEntryWithReceiveStatePort for Targets {
    async fn delete_entry_with_receive_state(
        &self,
        entry: &EntryId,
    ) -> Result<bool, ClipboardRepositoryError> {
        if entry.as_ref() == "second" && self.second_succeeds {
            return Ok(true);
        }
        Err(ClipboardRepositoryError::Storage(
            io::Error::new(io::ErrorKind::PermissionDenied, "PRIVATE_CLEAR_ERROR_BODY").into(),
        ))
    }
}

#[tokio::test]
async fn clear_keeps_each_deletion_source_and_leaves_failed_admission_closed() {
    for second_succeeds in [false, true] {
        let targets = Arc::new(Targets { second_succeeds });
        let work = Arc::new(ReceiveWorkRegistry::default());
        let deletion = Arc::new(DeleteClipboardEntryUseCase::from_ports(
            targets.clone(),
            targets.clone(),
            targets.clone(),
            work.clone(),
        ));
        let clear = ClearClipboardHistoryUseCase::new(targets, deletion, work.clone());
        let result = clear.execute().await;
        if second_succeeds {
            let result = result.unwrap();
            assert_eq!(result.deleted_count, 1);
            assert_eq!(result.failed_entries.len(), 1);
            assert!(result.failed_entries[0]
                .1
                .chain()
                .any(|source| source.downcast_ref::<io::Error>().is_some()));
        } else {
            let error = result.unwrap_err();
            assert!(!error.to_string().contains("PRIVATE_CLEAR_ERROR_BODY"));
            let failure = error.downcast_ref::<ClearHistoryFailure>().unwrap();
            assert_eq!(failure.0.len(), 2);
            for (_, source) in &failure.0 {
                assert_eq!(
                    source
                        .chain()
                        .find_map(|source| source.downcast_ref::<io::Error>())
                        .unwrap()
                        .kind(),
                    io::ErrorKind::PermissionDenied
                );
            }
        }
        assert!(!work.is_open());
    }
}
