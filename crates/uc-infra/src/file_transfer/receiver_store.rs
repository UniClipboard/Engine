use anyhow::Result;
use async_trait::async_trait;
use diesel::Connection;
use std::sync::Arc;

use crate::db::ports::DbExecutor;
use crate::file_transfer::event_store::sqlite::{
    append_prepared_event, decode_event_rows, load_event_rows, prepare_event_row,
    read_next_sequence, transfer_id_of, TransferCommitConflict,
};
use crate::file_transfer::persistence_cipher::TransferPersistenceProtection;
use crate::file_transfer::projection::sqlite::{
    apply_prepared_projection, load_projection_snapshot, prepare_projection,
};
use crate::security::ContentProtection;
use uc_core::file_transfer::{FileTransferEvent, FileTransferEventStorePort};
use uc_core::ports::security::current_profile::CurrentProfilePort;
use uc_core::ports::space::DeriveSpaceSubkeyPort;

const MAX_COMMIT_ATTEMPTS: usize = 4;

/// Receiver-side durable store that keeps event log and projection updates in one SQLite transaction.
pub struct SqliteReceiverFileTransferStore<E> {
    executor: E,
    protection: TransferPersistenceProtection,
}

impl<E> SqliteReceiverFileTransferStore<E> {
    pub fn new(
        executor: E,
        derive_subkey: Arc<dyn DeriveSpaceSubkeyPort>,
        current_profile: Arc<dyn CurrentProfilePort>,
    ) -> Self {
        Self {
            executor,
            protection: TransferPersistenceProtection::legacy(derive_subkey, current_profile),
        }
    }

    pub fn new_v3(executor: E, protection: Arc<ContentProtection>) -> Self {
        Self {
            executor,
            protection: TransferPersistenceProtection::v3(protection),
        }
    }
}

#[async_trait]
impl<E: DbExecutor> FileTransferEventStorePort for SqliteReceiverFileTransferStore<E> {
    async fn load(&self, transfer_id: &str) -> Result<Vec<FileTransferEvent>> {
        let transfer_id = transfer_id.to_string();
        let rows = self
            .executor
            .run(move |conn| load_event_rows(conn, &transfer_id))?;
        let protection = self.protection.resolve().await?;
        decode_event_rows(rows, &protection).await
    }

    async fn append(&self, event: FileTransferEvent) -> Result<()> {
        let protection = self.protection.resolve().await?;
        for attempt in 0..MAX_COMMIT_ATTEMPTS {
            let snapshot_transfer_id = transfer_id_of(&event).to_owned();
            let (sequence, projection_snapshot) = self.executor.run(move |conn| {
                conn.transaction::<_, anyhow::Error, _>(|conn| {
                    Ok((
                        read_next_sequence(conn, &snapshot_transfer_id)?,
                        load_projection_snapshot(conn, &snapshot_transfer_id)?,
                    ))
                })
            })?;
            let event_row = prepare_event_row(event.clone(), sequence, &protection).await?;
            let projection = prepare_projection(&event, projection_snapshot, &protection).await?;
            let result = self.executor.run(move |conn| {
                conn.transaction::<_, anyhow::Error, _>(|conn| {
                    append_prepared_event(conn, &event_row)?;
                    apply_prepared_projection(conn, &projection)?;
                    Ok(())
                })
            });
            match result {
                Ok(()) => return Ok(()),
                Err(error)
                    if error.downcast_ref::<TransferCommitConflict>().is_some()
                        && attempt + 1 < MAX_COMMIT_ATTEMPTS => {}
                Err(error) => return Err(error),
            }
        }
        Err(anyhow::anyhow!(
            "file transfer append exhausted commit retries"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::executor::DieselSqliteExecutor;
    use crate::db::models::FileTransferRow;
    use crate::db::pool::init_db_pool;
    use crate::db::ports::DbExecutor;
    use crate::db::repositories::DieselFileTransferRepository;
    use crate::db::schema::file_transfer;
    use diesel::prelude::*;
    use tempfile::{tempdir, TempDir};
    use uc_core::file_transfer::FileTransferCancellationReason;
    use uc_core::ports::file_transfer::{PendingInboundTransfer, TrackedFileTransferStatus};
    use uc_core::ports::RecordReceiverTransferPort;
    use uc_core::{FileTransferDirection, FileTransferProgress};

    fn make_store() -> (
        SqliteReceiverFileTransferStore<DieselSqliteExecutor>,
        DieselFileTransferRepository<DieselSqliteExecutor>,
        DieselSqliteExecutor,
        TempDir,
    ) {
        let tempdir = tempdir().unwrap();
        let database_url = tempdir.path().join("receiver-file-transfer-store.sqlite");
        let pool = init_db_pool(database_url.to_str().unwrap()).unwrap();
        let (derive_subkey, current_profile) =
            crate::file_transfer::persistence_cipher::test_keys::ports();
        let store = SqliteReceiverFileTransferStore::new(
            DieselSqliteExecutor::new(pool.clone()),
            derive_subkey.clone(),
            current_profile.clone(),
        );
        let repo = DieselFileTransferRepository::new(
            DieselSqliteExecutor::new(pool.clone()),
            derive_subkey,
            current_profile,
        );
        let reader = DieselSqliteExecutor::new(pool);

        (store, repo, reader, tempdir)
    }

    // Read projection rows back directly via the schema — the receiver
    // projection is verified at the infra layer, not through a domain port.
    fn load_rows(reader: &DieselSqliteExecutor, entry_id: &str) -> Vec<FileTransferRow> {
        let eid = entry_id.to_string();
        reader
            .run(move |conn| {
                Ok(file_transfer::table
                    .filter(file_transfer::entry_id.eq(&eid))
                    .load::<FileTransferRow>(conn)?)
            })
            .unwrap()
    }

    fn pending_transfer() -> PendingInboundTransfer {
        PendingInboundTransfer {
            transfer_id: "transfer-1".into(),
            entry_id: "entry-1".into(),
            attempt_id: None,
            origin_device_id: "device-1".into(),
            filename: "report.pdf".into(),
            file_size: None,
            cached_path: "/tmp/report.pdf".into(),
            created_at_ms: 10,
        }
    }

    #[tokio::test]
    async fn append_event_and_project_updates_both_event_log_and_projection() {
        let (store, repo, reader, _tempdir) = make_store();
        repo.upsert_pending_transfer(&pending_transfer())
            .await
            .unwrap();

        let started = FileTransferEvent::started("transfer-1", "peer-1", "report.pdf", Some(128));
        let progress = FileTransferEvent::Progress {
            transfer_id: "transfer-1".into(),
            peer_id: "peer-1".into(),
            progress: FileTransferProgress {
                direction: FileTransferDirection::Receiving,
                bytes_transferred: 64,
                total_bytes: Some(128),
            },
        };

        store.append(started.clone()).await.unwrap();
        store.append(progress.clone()).await.unwrap();

        assert_eq!(
            store.load("transfer-1").await.unwrap(),
            vec![started, progress]
        );

        let rows = load_rows(&reader, "entry-1");
        let row = &rows[0];
        assert_eq!(row.file_size, Some(128));
        assert_eq!(row.status, TrackedFileTransferStatus::Transferring.as_str());
    }

    #[tokio::test]
    async fn append_succeeds_without_receiver_context_for_sender_side_events() {
        // Sender-side transfers intentionally do not seed a receiver context.
        // The event log still records them; the receiver projection update is
        // simply a no-op when no row exists. This makes `store.append` safe to
        // call from both sides without the caller caring which one it is.
        let (store, _repo, _reader, _tempdir) = make_store();
        let event = FileTransferEvent::completed("sender-only-transfer", "peer-1");

        store.append(event.clone()).await.unwrap();

        assert_eq!(
            store.load("sender-only-transfer").await.unwrap(),
            vec![event]
        );
    }

    #[tokio::test]
    async fn late_completed_event_does_not_regress_a_cancelled_projection() {
        let (store, repo, reader, _tempdir) = make_store();
        repo.upsert_pending_transfer(&pending_transfer())
            .await
            .unwrap();
        store
            .append(FileTransferEvent::cancelled(
                "transfer-1",
                "peer-1",
                FileTransferCancellationReason::LocalUser,
            ))
            .await
            .unwrap();

        store
            .append(FileTransferEvent::completed("transfer-1", "peer-1"))
            .await
            .unwrap();

        let rows = load_rows(&reader, "entry-1");
        assert_eq!(
            rows[0].status,
            TrackedFileTransferStatus::Cancelled.as_str()
        );
        assert_eq!(store.load("transfer-1").await.unwrap().len(), 2);
    }
}
