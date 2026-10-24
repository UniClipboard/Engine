//! 读取资源查询的完整资料，再使用同一事件身份认证内联内容。
use crate::db::{
    mappers::snapshot_representation_mapper::RepresentationRowMapper,
    models::snapshot_representation::SnapshotRepresentationRow,
    ports::{DbExecutor, RowMapper},
    schema::{clipboard_entry, clipboard_selection, clipboard_snapshot_representation},
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use diesel::{
    Connection, ExpressionMethods, OptionalExtension, QueryDsl, RunQueryDsl, SqliteConnection,
};
use std::sync::Arc;
use uc_application::deps::{
    EntryResourceReadError, EntryResourceSnapshot, ReadEntryResourceSnapshotPort,
};
use uc_core::{
    crypto::{
        aad,
        domain::{Aad, Ciphertext},
    },
    ids::{EntryId, EventId},
    ports::security::BlobCipherPort,
};

pub struct DieselEntryResourceReader<E> {
    executor: E,
    cipher: Arc<dyn BlobCipherPort>,
}

impl<E: DbExecutor> DieselEntryResourceReader<E> {
    pub fn new(executor: E, cipher: Arc<dyn BlobCipherPort>) -> Self {
        Self { executor, cipher }
    }
}

fn read_snapshot(
    conn: &mut SqliteConnection,
    entry: &EntryId,
) -> Result<Option<EntryResourceSnapshot>> {
    // 同一次交付替换会删除旧表示；所有关联读取必须固定在同一个只读快照。
    conn.transaction(|conn| {
        let event: Option<String> = clipboard_entry::table
            .filter(clipboard_entry::entry_id.eq(entry.as_str()))
            .select(clipboard_entry::event_id)
            .first(conn)
            .optional()?;
        let Some(event) = event else { return Ok(None) };
        let preview: Option<String> = clipboard_selection::table
            .filter(clipboard_selection::entry_id.eq(entry.as_str()))
            .select(clipboard_selection::preview_rep_id)
            .first(conn)
            .optional()?;
        let rows = clipboard_snapshot_representation::table
            .filter(clipboard_snapshot_representation::event_id.eq(&event))
            .load::<SnapshotRepresentationRow>(conn)?;
        Ok(Some(EntryResourceSnapshot {
            event_id: EventId::from(event),
            preview_representation: preview.map(Into::into),
            representations: rows
                .iter()
                .map(|row| RepresentationRowMapper.to_domain(row))
                .collect::<Result<_>>()?,
        }))
    })
}

#[async_trait]
impl<E: DbExecutor> ReadEntryResourceSnapshotPort for DieselEntryResourceReader<E> {
    async fn read_entry_resource_snapshot(
        &self,
        entry: &EntryId,
    ) -> Result<Option<EntryResourceSnapshot>, EntryResourceReadError> {
        let snapshot = self
            .executor
            .run(|conn| read_snapshot(conn, entry))
            .context("read entry resource snapshot")?;
        let Some(mut snapshot) = snapshot else {
            return Ok(None);
        };
        // 数据库连接在认证前归还；解密不再重新查询可能已经替换的行。
        for rep in &mut snapshot.representations {
            if let Some(bytes) = rep.inline_data.take() {
                let plaintext = self
                    .cipher
                    .decrypt(
                        &Ciphertext::new(bytes),
                        &Aad::from(aad::for_inline(&snapshot.event_id, &rep.id).as_slice()),
                    )
                    .await
                    .context("authenticate entry resource representation")?;
                rep.inline_data = Some(plaintext.into_bytes());
            }
        }
        Ok(Some(snapshot))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diesel::sql_types::Binary;
    use diesel::{
        connection::{InstrumentationEvent, SimpleConnection},
        Connection,
    };
    use std::sync::atomic::{AtomicBool, Ordering};
    use uc_core::crypto::domain::Plaintext;
    use uc_core::ids::RepresentationId;
    use uc_core::ports::security::BlobCipherError;
    use uc_infra_security::{BlobCipherAdapter, InMemorySession, MasterKey};

    fn seed(conn: &mut SqliteConnection) {
        conn.batch_execute(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE clipboard_entry (entry_id TEXT PRIMARY KEY, event_id TEXT);
             CREATE TABLE clipboard_selection (entry_id TEXT PRIMARY KEY, preview_rep_id TEXT);
             CREATE TABLE clipboard_snapshot_representation (
               id TEXT PRIMARY KEY, event_id TEXT, format_id TEXT, mime_type TEXT,
               size_bytes BIGINT, inline_data BLOB, blob_id TEXT, payload_state TEXT, last_error TEXT
             );
             INSERT INTO clipboard_entry VALUES ('entry', 'old-event');
             INSERT INTO clipboard_selection VALUES ('entry', 'old-preview');
             INSERT INTO clipboard_snapshot_representation VALUES
               ('old-preview', 'old-event', 'text', 'text/plain', 3, X'6f6c64', NULL, 'Inline', NULL);"
        ).unwrap();
    }

    #[test]
    fn snapshot_remains_coherent_when_content_is_replaced_after_entry_read() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("resource.sqlite");
        let mut reader = SqliteConnection::establish(path.to_str().unwrap()).unwrap();
        seed(&mut reader);
        let replaced = Arc::new(AtomicBool::new(false));
        let observed = replaced.clone();
        // 在第一次真实 SELECT 结束后，由另一个连接提交完整替换，无需随机调度或 sleep。
        reader.set_instrumentation(move |event: InstrumentationEvent<'_>| {
            if let InstrumentationEvent::FinishQuery { query, error: None, .. } = event {
                if query.to_string().starts_with("SELECT")
                    && query.to_string().contains("clipboard_entry")
                    && !observed.swap(true, Ordering::SeqCst)
                {
                    let mut writer = SqliteConnection::establish(path.to_str().unwrap()).unwrap();
                    writer.batch_execute(
                        "BEGIN IMMEDIATE;
                         UPDATE clipboard_entry SET event_id = 'new-event' WHERE entry_id = 'entry';
                         UPDATE clipboard_selection SET preview_rep_id = 'new-preview' WHERE entry_id = 'entry';
                         DELETE FROM clipboard_snapshot_representation;
                         INSERT INTO clipboard_snapshot_representation VALUES
                           ('new-preview', 'new-event', 'text', 'text/plain', 3, X'6e6577', NULL, 'Inline', NULL);
                         COMMIT;"
                    ).unwrap();
                }
            }
        });
        let snapshot = read_snapshot(&mut reader, &EntryId::from("entry"))
            .unwrap()
            .unwrap();
        assert!(replaced.load(Ordering::SeqCst), "必须实际提交并发替换");
        assert_eq!(snapshot.event_id.as_str(), "old-event");
        assert_eq!(
            snapshot.preview_representation.unwrap().as_str(),
            "old-preview"
        );
        assert_eq!(snapshot.representations.len(), 1);
        assert_eq!(
            snapshot.representations[0].inline_data.as_deref(),
            Some(b"old".as_slice())
        );
        let current = read_snapshot(&mut reader, &EntryId::from("entry"))
            .unwrap()
            .unwrap();
        assert_eq!(current.event_id.as_str(), "new-event");
        assert_eq!(
            current.representations[0].inline_data.as_deref(),
            Some(b"new".as_slice())
        );
    }

    struct Database(std::sync::Mutex<SqliteConnection>);
    impl DbExecutor for Database {
        fn run<T>(&self, f: impl FnOnce(&mut SqliteConnection) -> Result<T>) -> Result<T> {
            f(&mut self.0.lock().unwrap())
        }
    }

    #[tokio::test]
    async fn snapshot_authenticates_inline_data_against_its_original_event() {
        let mut conn = SqliteConnection::establish(":memory:").unwrap();
        seed(&mut conn);
        let database = Arc::new(Database(std::sync::Mutex::new(conn)));
        let session = Arc::new(InMemorySession::new());
        session.set_master_key_for_space("space".into(), MasterKey::from_bytes(&[7; 32]).unwrap());
        let cipher = Arc::new(BlobCipherAdapter::new(session));
        let encrypted = cipher
            .encrypt(
                &Plaintext::new(b"old".to_vec()),
                &Aad::from(
                    aad::for_inline(
                        &EventId::from("old-event"),
                        &RepresentationId::from("old-preview"),
                    )
                    .as_slice(),
                ),
            )
            .await
            .unwrap();
        database
            .run(|conn| {
                diesel::sql_query("UPDATE clipboard_snapshot_representation SET inline_data = ?")
                    .bind::<Binary, _>(encrypted.as_bytes())
                    .execute(conn)?;
                Ok(())
            })
            .unwrap();
        let reader = DieselEntryResourceReader::new(database.clone(), cipher);
        assert!(reader
            .read_entry_resource_snapshot(&EntryId::from("missing"))
            .await
            .unwrap()
            .is_none());
        let snapshot = reader
            .read_entry_resource_snapshot(&EntryId::from("entry"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            snapshot.representations[0].inline_data.as_deref(),
            Some(b"old".as_slice())
        );
        // 关联记录仍完整，但内联密文不属于新事件，必须保留认证失败来源而不是交付密文。
        database.run(|conn| {
            conn.batch_execute("UPDATE clipboard_entry SET event_id = 'new-event'; UPDATE clipboard_snapshot_representation SET event_id = 'new-event';")?;
            Ok(())
        }).unwrap();
        let error = reader
            .read_entry_resource_snapshot(&EntryId::from("entry"))
            .await
            .err()
            .unwrap();
        assert!(std::error::Error::source(&error).is_some());
        assert!(error.0.chain().any(|source| source.is::<BlobCipherError>()));
    }
}
