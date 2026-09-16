use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use diesel::prelude::*;
use diesel::sqlite::SqliteConnection;
use std::sync::Arc;

use crate::db::ports::DbExecutor;
use crate::db::schema::file_transfer_events;
use crate::file_transfer::persistence_cipher::{
    ResolvedTransferPersistenceProtection, TransferPersistenceProtection,
};
use crate::security::ContentProtection;
use uc_core::file_transfer::{FileTransferEvent, FileTransferEventStorePort};
use uc_core::ports::security::current_profile::CurrentProfilePort;
use uc_core::ports::space::DeriveSpaceSubkeyPort;

const MAX_COMMIT_ATTEMPTS: usize = 4;

#[allow(dead_code)]
#[derive(Debug, Clone, Queryable, Selectable)]
#[diesel(table_name = file_transfer_events)]
#[diesel(check_for_backend(diesel::sqlite::Sqlite))]
pub(crate) struct FileTransferEventRow {
    id: i32,
    transfer_id: String,
    sequence: i32,
    event_type: String,
    payload_ciphertext: Vec<u8>,
    occurred_at_ms: i64,
}

#[derive(Debug, Clone, Insertable)]
#[diesel(table_name = file_transfer_events)]
pub(crate) struct NewFileTransferEventRow {
    transfer_id: String,
    sequence: i32,
    event_type: String,
    payload_ciphertext: Vec<u8>,
    occurred_at_ms: i64,
}

/// SQLite-backed event store for file transfer lifecycle events.
pub struct SqliteFileTransferEventStore<E> {
    executor: E,
    protection: TransferPersistenceProtection,
}

impl<E> SqliteFileTransferEventStore<E> {
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
impl<E: DbExecutor> FileTransferEventStorePort for SqliteFileTransferEventStore<E> {
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
            let transfer_id = transfer_id_of(&event).to_owned();
            let sequence = self
                .executor
                .run(move |conn| read_next_sequence(conn, &transfer_id))?;
            let row = prepare_event_row(event.clone(), sequence, &protection).await?;
            let result = self.executor.run(move |conn| {
                conn.transaction::<_, anyhow::Error, _>(|conn| append_prepared_event(conn, &row))
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
            "file transfer event append exhausted commit retries"
        ))
    }
}

pub(crate) fn load_event_rows(
    conn: &mut SqliteConnection,
    transfer_id: &str,
) -> Result<Vec<FileTransferEventRow>> {
    file_transfer_events::table
        .filter(file_transfer_events::transfer_id.eq(transfer_id))
        .order(file_transfer_events::sequence.asc())
        .load::<FileTransferEventRow>(conn)
        .context("failed to load file transfer events")
}

pub(crate) async fn decode_event_rows(
    rows: Vec<FileTransferEventRow>,
    protection: &ResolvedTransferPersistenceProtection,
) -> Result<Vec<FileTransferEvent>> {
    let mut events = Vec::with_capacity(rows.len());
    for row in rows {
        let event = protection
            .open_event(
                &row.transfer_id,
                row.sequence,
                &row.event_type,
                &row.payload_ciphertext,
            )
            .await
            .with_context(|| {
                format!(
                    "failed to deserialize file transfer event `{}` at sequence {}",
                    row.event_type, row.sequence
                )
            })?;
        events.push(event);
    }
    Ok(events)
}

pub(crate) fn read_next_sequence(conn: &mut SqliteConnection, transfer_id: &str) -> Result<i32> {
    let current_max: Option<i32> = file_transfer_events::table
        .filter(file_transfer_events::transfer_id.eq(transfer_id))
        .select(diesel::dsl::max(file_transfer_events::sequence))
        .first(conn)
        .context("failed to read file transfer event sequence")?;

    current_max
        .unwrap_or(0)
        .checked_add(1)
        .context("file transfer event sequence overflow")
}

pub(crate) async fn prepare_event_row(
    event: FileTransferEvent,
    sequence: i32,
    protection: &ResolvedTransferPersistenceProtection,
) -> Result<NewFileTransferEventRow> {
    let transfer_id = transfer_id_of(&event).to_string();
    let event_type = event_type_of(&event).to_string();
    let payload_ciphertext = protection
        .seal_event(&transfer_id, sequence, &event_type, &event)
        .await
        .with_context(|| format!("failed to seal file transfer event `{event_type}`"))?;
    Ok(NewFileTransferEventRow {
        transfer_id: transfer_id.clone(),
        sequence,
        event_type,
        payload_ciphertext,
        occurred_at_ms: Utc::now().timestamp_millis(),
    })
}

pub(crate) fn append_prepared_event(
    conn: &mut SqliteConnection,
    row: &NewFileTransferEventRow,
) -> Result<()> {
    let expected_sequence = read_next_sequence(conn, &row.transfer_id)?;
    if expected_sequence != row.sequence {
        return Err(TransferCommitConflict.into());
    }

    diesel::insert_into(file_transfer_events::table)
        .values(row)
        .execute(conn)
        .with_context(|| {
            format!(
                "failed to append file transfer event at sequence {}",
                row.sequence
            )
        })?;

    Ok(())
}

#[derive(Debug, thiserror::Error)]
#[error("file transfer persistence changed concurrently")]
pub(crate) struct TransferCommitConflict;

pub(crate) fn transfer_id_of(event: &FileTransferEvent) -> &str {
    match event {
        FileTransferEvent::Started { transfer_id, .. }
        | FileTransferEvent::Progress { transfer_id, .. }
        | FileTransferEvent::Completed { transfer_id, .. }
        | FileTransferEvent::Failed { transfer_id, .. }
        | FileTransferEvent::Cancelled { transfer_id, .. } => transfer_id,
    }
}

fn event_type_of(event: &FileTransferEvent) -> &'static str {
    match event {
        FileTransferEvent::Started { .. } => "started",
        FileTransferEvent::Progress { .. } => "progress",
        FileTransferEvent::Completed { .. } => "completed",
        FileTransferEvent::Failed { .. } => "failed",
        FileTransferEvent::Cancelled { .. } => "cancelled",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::executor::DieselSqliteExecutor;
    use crate::db::pool::init_db_pool;
    use tempfile::{tempdir, TempDir};
    use uc_core::{FileTransferCancellationReason, FileTransferDirection, FileTransferProgress};

    fn make_store() -> (SqliteFileTransferEventStore<DieselSqliteExecutor>, TempDir) {
        let tempdir = tempdir().unwrap();
        let database_url = tempdir.path().join("file-transfer-events.sqlite");
        let pool = init_db_pool(database_url.to_str().unwrap()).unwrap();
        let (derive_subkey, current_profile) =
            crate::file_transfer::persistence_cipher::test_keys::ports();
        (
            SqliteFileTransferEventStore::new(
                DieselSqliteExecutor::new(pool),
                derive_subkey,
                current_profile,
            ),
            tempdir,
        )
    }

    #[tokio::test]
    async fn append_and_load_returns_events_in_sequence_order() {
        let (store, _tempdir) = make_store();
        let started = FileTransferEvent::started("transfer-1", "peer-1", "report.pdf", Some(128));
        let progress = FileTransferEvent::Progress {
            transfer_id: "transfer-1".into(),
            peer_id: "peer-1".into(),
            progress: FileTransferProgress {
                direction: FileTransferDirection::Receiving,
                bytes_transferred: 96,
                total_bytes: Some(128),
            },
        };

        store.append(started.clone()).await.unwrap();
        store.append(progress.clone()).await.unwrap();

        assert_eq!(
            store.load("transfer-1").await.unwrap(),
            vec![started, progress]
        );
    }

    #[tokio::test]
    async fn load_only_returns_events_for_requested_transfer() {
        let (store, _tempdir) = make_store();
        let first = FileTransferEvent::completed("transfer-1", "peer-1");
        let second = FileTransferEvent::cancelled(
            "transfer-2",
            "peer-2",
            FileTransferCancellationReason::RemotePeer,
        );

        store.append(first.clone()).await.unwrap();
        store.append(second).await.unwrap();

        assert_eq!(store.load("transfer-1").await.unwrap(), vec![first]);
        assert!(store.load("missing").await.unwrap().is_empty());
    }
}
