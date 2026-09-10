//! 启动安全检查的窄读取：在数据库中筛出文件引用，只解密选中的 URI 清单。
use crate::db::ports::DbExecutor;
use anyhow::{Context, Result};
use async_trait::async_trait;
use diesel::{
    sql_types::{BigInt, Binary, Text},
    QueryableByName, RunQueryDsl,
};
use std::sync::Arc;
use uc_application::facade::clipboard_history::{HistoryFileReference, HistoryFileReferencePort};
use uc_core::{
    crypto::{
        aad,
        domain::{Aad, Ciphertext},
    },
    ids::{EntryId, EventId, RepresentationId},
    ports::security::BlobCipherPort,
};

pub struct DieselHistoryFileReferences<E: DbExecutor> {
    executor: E,
    cipher: Arc<dyn BlobCipherPort>,
}
impl<E: DbExecutor> DieselHistoryFileReferences<E> {
    pub fn new(executor: E, cipher: Arc<dyn BlobCipherPort>) -> Self {
        Self { executor, cipher }
    }
}

#[derive(QueryableByName)]
struct FileReferenceRow {
    #[diesel(sql_type = Text)]
    entry_id: String,
    #[diesel(sql_type = Text)]
    event_id: String,
    #[diesel(sql_type = Text)]
    representation_id: String,
    #[diesel(sql_type = Binary)]
    inline_data: Vec<u8>,
}

#[async_trait]
impl<E: DbExecutor> HistoryFileReferencePort for DieselHistoryFileReferences<E> {
    async fn list_file_references(
        &self,
        after: Option<&RepresentationId>,
        limit: usize,
    ) -> Result<Vec<HistoryFileReference>> {
        let after = after.map(|id| id.as_str()).unwrap_or("");
        let limit = i64::try_from(limit).context("history file reference page limit")?;
        let rows = self.executor.run(|conn| {
            diesel::sql_query("SELECT e.entry_id, r.event_id, r.id AS representation_id, r.inline_data FROM clipboard_snapshot_representation r JOIN clipboard_entry e ON e.event_id = r.event_id WHERE r.id > ? AND r.inline_data IS NOT NULL AND (lower(COALESCE(r.mime_type, '')) LIKE '%uri-list%' OR lower(r.format_id) = 'files') ORDER BY r.id ASC LIMIT ?")
                .bind::<Text, _>(after).bind::<BigInt, _>(limit)
                .load::<FileReferenceRow>(conn).context("query history file references")
        })?;
        let mut references = Vec::with_capacity(rows.len());
        for row in rows {
            let event_id = EventId::from(row.event_id);
            let representation_id = RepresentationId::from(row.representation_id);
            let plaintext = self
                .cipher
                .decrypt(
                    &Ciphertext::new(row.inline_data),
                    &Aad::from(aad::for_inline(&event_id, &representation_id).as_slice()),
                )
                .await
                .context("decrypt history file reference")?;
            references.push(HistoryFileReference {
                entry_id: EntryId::from(row.entry_id),
                representation_id,
                uri_list: plaintext.into_bytes(),
            });
        }
        Ok(references)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        security::{BlobCipherAdapter, MasterKey},
        space::InMemorySession,
    };
    use diesel::{Connection, SqliteConnection};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    };
    use uc_core::crypto::domain::Plaintext;
    use uc_core::ports::security::BlobCipherError;

    struct Sqlite(Mutex<SqliteConnection>);
    impl DbExecutor for Sqlite {
        fn run<T>(&self, f: impl FnOnce(&mut SqliteConnection) -> Result<T>) -> Result<T> {
            f(&mut self.0.lock().unwrap())
        }
    }
    struct CountingCipher {
        inner: BlobCipherAdapter,
        reads: AtomicUsize,
    }
    #[async_trait]
    impl BlobCipherPort for CountingCipher {
        async fn encrypt(&self, p: &Plaintext, a: &Aad) -> Result<Ciphertext, BlobCipherError> {
            self.inner.encrypt(p, a).await
        }
        async fn decrypt(&self, c: &Ciphertext, a: &Aad) -> Result<Plaintext, BlobCipherError> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            self.inner.decrypt(c, a).await
        }
    }

    #[tokio::test]
    async fn file_scan_is_paginated_authenticated_and_skips_unrelated_payloads() {
        let db = Arc::new(Sqlite(Mutex::new(
            SqliteConnection::establish(":memory:").unwrap(),
        )));
        db.run(|conn| {
            diesel::sql_query("CREATE TABLE clipboard_entry (entry_id TEXT PRIMARY KEY, event_id TEXT UNIQUE)").execute(conn)?;
            diesel::sql_query("CREATE TABLE clipboard_snapshot_representation (id TEXT PRIMARY KEY, event_id TEXT, mime_type TEXT, format_id TEXT, inline_data BLOB)").execute(conn)?;
            diesel::sql_query("INSERT INTO clipboard_entry VALUES ('entry', 'event')").execute(conn)?;
            Ok(())
        }).unwrap();
        let root = MasterKey::from_bytes(&[7; 32]).unwrap();
        let session = Arc::new(InMemorySession::new());
        session.set_master_key_for_space("space".into(), root.clone());
        let cipher = Arc::new(CountingCipher {
            inner: BlobCipherAdapter::new(session),
            reads: AtomicUsize::new(0),
        });
        for (id, mime, format) in [
            ("file-a", "TEXT/URI-LIST", "native"),
            ("file-b", "", "files"),
        ] {
            let aad = aad::for_inline(&EventId::from("event"), &RepresentationId::from(id));
            let encrypted = crate::security::v1_aead::encrypt_blob_xchacha(
                &root,
                b"file:///managed/missing",
                &aad,
            )
            .unwrap();
            let bytes = serde_json::to_vec(&encrypted).unwrap();
            db.run(|conn| {
                diesel::sql_query(
                    "INSERT INTO clipboard_snapshot_representation VALUES (?, 'event', ?, ?, ?)",
                )
                .bind::<Text, _>(id)
                .bind::<Text, _>(mime)
                .bind::<Text, _>(format)
                .bind::<Binary, _>(&bytes)
                .execute(conn)?;
                Ok(())
            })
            .unwrap();
        }
        db.run(|conn| {
            diesel::sql_query("WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x < 5000) INSERT INTO clipboard_snapshot_representation SELECT 'text-' || x, 'event', 'text/plain', 'text', X'00' FROM n").execute(conn)?;
            Ok(())
        }).unwrap();
        let reader = DieselHistoryFileReferences::new(db.clone(), cipher.clone());
        let first = reader.list_file_references(None, 1).await.unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].uri_list, b"file:///managed/missing");
        let second = reader
            .list_file_references(Some(&first[0].representation_id), 1)
            .await
            .unwrap();
        assert_eq!(second.len(), 1);
        assert!(reader
            .list_file_references(Some(&second[0].representation_id), 1)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(cipher.reads.load(Ordering::SeqCst), 2);
        db.run(|conn| { diesel::sql_query("UPDATE clipboard_snapshot_representation SET inline_data = X'00' WHERE id = 'file-a'").execute(conn)?; Ok(()) }).unwrap();
        assert!(reader.list_file_references(None, 10).await.is_err());
    }
}
