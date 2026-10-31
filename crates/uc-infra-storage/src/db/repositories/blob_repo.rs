use crate::db::models::blob::NewBlobRow;
use crate::db::models::BlobRow;
use crate::db::ports::DbExecutor;
use crate::db::ports::{InsertMapper, RowMapper};
use crate::db::schema::blob;
use anyhow::{Context, Result};
use diesel::sql_types::{BigInt, Nullable, Text};
use diesel::{ExpressionMethods, OptionalExtension, QueryDsl, QueryableByName, RunQueryDsl};
use tracing::debug_span;
use uc_core::{BlobId, ContentHash};
use uc_infra_local::blob::{Blob, BlobRepositoryPort, UnreferencedBlobDeletion};

pub struct DieselBlobRepository<E, IM, RM>
where
    E: DbExecutor,
    IM: InsertMapper<Blob, NewBlobRow>,
    RM: RowMapper<BlobRow, Blob>,
{
    executor: E,
    insert_mapper: IM,
    row_mapper: RM,
}

impl<E, IM, RM> DieselBlobRepository<E, IM, RM>
where
    E: DbExecutor,
    IM: InsertMapper<Blob, NewBlobRow>,
    RM: RowMapper<BlobRow, Blob>,
{
    pub fn new(executor: E, insert_mapper: IM, row_mapper: RM) -> Self {
        Self {
            executor,
            insert_mapper,
            row_mapper,
        }
    }
}

#[async_trait::async_trait]
impl<E, IM, RM> BlobRepositoryPort for DieselBlobRepository<E, IM, RM>
where
    E: DbExecutor,
    IM: InsertMapper<Blob, NewBlobRow>,
    RM: RowMapper<BlobRow, Blob>,
{
    async fn insert_blob(&self, blob_row: &Blob) -> Result<()> {
        let span = debug_span!(
            "infra.sqlite.insert_blob",
            table = "blob",
            blob_id = %blob_row.blob_id,
            size_bytes = blob_row.size_bytes,
        );
        span.in_scope(|| {
            let new_blob_row = self.insert_mapper.to_row(blob_row)?;
            self.executor.run(|conn| {
                diesel::insert_into(blob::table)
                    .values(&new_blob_row)
                    .execute(conn)?;
                Ok(())
            })
        })
    }

    async fn find_by_hash(&self, content_hash: &ContentHash) -> Result<Option<Blob>> {
        let span = debug_span!(
            "infra.sqlite.query_blob_by_hash",
            table = "blob",
            content_hash = %content_hash,
        );
        span.in_scope(|| {
            let blob_row: Option<BlobRow> = self.executor.run(|conn| {
                let result: Result<Option<BlobRow>, diesel::result::Error> = blob::table
                    .filter(blob::content_hash.eq(content_hash.to_string()))
                    .first::<BlobRow>(conn)
                    .optional();
                result.context("Database error")
            })?;

            match blob_row {
                Some(row) => {
                    let blob = self.row_mapper.to_domain(&row)?;
                    Ok(Some(blob))
                }
                None => Ok(None),
            }
        })
    }

    async fn claim_existing(
        &self,
        content_hash: &ContentHash,
        claimed_at_ms: i64,
    ) -> Result<Option<Blob>> {
        let content_hash = content_hash.to_string();
        let row = self.executor.run(move |conn| {
            conn.immediate_transaction(|conn| {
                let existing = blob::table
                    .filter(blob::content_hash.eq(&content_hash))
                    .first::<BlobRow>(conn)
                    .optional()
                    .context("claim blob by hash")?;
                let Some(mut row) = existing else {
                    return Ok(None);
                };
                diesel::update(blob::table.filter(blob::blob_id.eq(&row.blob_id)))
                    .set(blob::last_claimed_at_ms.eq(claimed_at_ms))
                    .execute(conn)
                    .context("refresh blob last_claimed_at")?;
                row.last_claimed_at_ms = claimed_at_ms;
                Ok(Some(row))
            })
        })?;
        row.as_ref()
            .map(|row| self.row_mapper.to_domain(row))
            .transpose()
    }

    async fn delete_if_unreferenced(&self, blob_id: &BlobId) -> Result<UnreferencedBlobDeletion> {
        let blob_id = blob_id.to_string();
        self.executor.run(move |conn| {
            conn.immediate_transaction(|conn| {
                let deleted = diesel::sql_query(
                    "DELETE FROM blob WHERE blob_id = ? \
                     AND NOT EXISTS ( \
                         SELECT 1 FROM clipboard_snapshot_representation \
                         WHERE clipboard_snapshot_representation.blob_id = blob.blob_id \
                     ) \
                     AND NOT EXISTS ( \
                         SELECT 1 FROM clipboard_representation_thumbnail \
                         WHERE thumbnail_blob_id = blob.blob_id \
                     ) \
                     AND NOT EXISTS ( \
                         SELECT 1 FROM entry_file_set \
                         WHERE entry_file_set.blob_id = blob.blob_id \
                     ) \
                     RETURNING size_bytes, compressed_size",
                )
                .bind::<Text, _>(&blob_id)
                .get_result::<DeletedBlobBytes>(conn)
                .optional()
                .context("delete blob if unreferenced")?;
                Ok(match deleted {
                    Some(row) => UnreferencedBlobDeletion::Deleted {
                        bytes_on_disk: on_disk_bytes(row.size_bytes, row.compressed_size),
                    },
                    None => UnreferencedBlobDeletion::Retained,
                })
            })
        })
    }
}

#[derive(QueryableByName)]
struct DeletedBlobBytes {
    #[diesel(sql_type = BigInt)]
    size_bytes: i64,
    #[diesel(sql_type = Nullable<BigInt>)]
    compressed_size: Option<i64>,
}

fn on_disk_bytes(size_bytes: i64, compressed_size: Option<i64>) -> u64 {
    let bytes = compressed_size.unwrap_or(size_bytes);
    u64::try_from(bytes).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::executor::DieselSqliteExecutor;
    use crate::db::mappers::blob_mapper::BlobRowMapper;
    use crate::db::pool::init_db_pool;
    use diesel::connection::SimpleConnection;
    use diesel::sql_query;
    use tempfile::tempdir;
    use uc_core::ContentHash;

    fn live_hash() -> ContentHash {
        ContentHash::from(&[9u8; 32])
    }

    fn repo() -> (
        DieselBlobRepository<DieselSqliteExecutor, BlobRowMapper, BlobRowMapper>,
        tempfile::TempDir,
    ) {
        let dir = tempdir().unwrap();
        let path = dir.path().join("blobs.sqlite");
        let pool = init_db_pool(path.to_str().unwrap()).unwrap();
        (
            DieselBlobRepository::new(
                DieselSqliteExecutor::new(pool),
                BlobRowMapper,
                BlobRowMapper,
            ),
            dir,
        )
    }

    fn insert_blob(executor: &DieselSqliteExecutor, blob_id: &str, hash: &str, size: i64) {
        executor
            .run({
                let blob_id = blob_id.to_owned();
                let hash = hash.to_owned();
                move |conn| {
                    sql_query(
                        "INSERT INTO blob (blob_id, storage_path, storage_backend, size_bytes, \
                         content_hash, created_at_ms, compressed_size, last_claimed_at_ms) \
                         VALUES (?, '/vault/' || ?, 'local_fs', ?, ?, 10, ?, 10)",
                    )
                    .bind::<Text, _>(&blob_id)
                    .bind::<Text, _>(&blob_id)
                    .bind::<BigInt, _>(size)
                    .bind::<Text, _>(&hash)
                    .bind::<Nullable<BigInt>, _>(Some(size))
                    .execute(conn)?;
                    Ok(())
                }
            })
            .unwrap();
    }

    fn blob_count(executor: &DieselSqliteExecutor, blob_id: &str) -> i64 {
        executor
            .run({
                let blob_id = blob_id.to_owned();
                move |conn| {
                    let count = sql_query("SELECT COUNT(*) AS n FROM blob WHERE blob_id = ?")
                        .bind::<Text, _>(&blob_id)
                        .get_result::<CountRow>(conn)?;
                    Ok(count.n)
                }
            })
            .unwrap()
    }

    #[derive(QueryableByName)]
    struct CountRow {
        #[diesel(sql_type = BigInt)]
        n: i64,
    }

    #[tokio::test]
    async fn unreferenced_blob_deletes_once_and_repeat_is_retained() {
        let (repository, dir) = repo();
        let pool = init_db_pool(dir.path().join("blobs.sqlite").to_str().unwrap()).unwrap();
        let executor = DieselSqliteExecutor::new(pool);
        insert_blob(&executor, "only", "hash-only", 40);
        let deleted = repository
            .delete_if_unreferenced(&BlobId::from("only"))
            .await
            .unwrap();
        assert_eq!(
            deleted,
            UnreferencedBlobDeletion::Deleted { bytes_on_disk: 40 }
        );
        assert_eq!(blob_count(&executor, "only"), 0);
        let again = repository
            .delete_if_unreferenced(&BlobId::from("only"))
            .await
            .unwrap();
        assert_eq!(again, UnreferencedBlobDeletion::Retained);
    }

    #[tokio::test]
    async fn any_live_referencer_keeps_the_row_and_does_not_set_null() {
        let (repository, dir) = repo();
        let pool = init_db_pool(dir.path().join("blobs.sqlite").to_str().unwrap()).unwrap();
        let executor = DieselSqliteExecutor::new(pool);
        insert_blob(&executor, "shared", "hash-shared", 8);
        executor
            .run(|conn| {
                sql_query(
                    "INSERT INTO clipboard_event (event_id, captured_at_ms, source_device, snapshot_hash) \
                     VALUES ('ev', 1, 'device', 'snap')",
                )
                .execute(conn)?;
                sql_query(
                    "INSERT INTO clipboard_entry (entry_id, event_id, created_at_ms, active_time_ms, \
                     total_size, pinned, delivery_tracked, is_favorited, content_category) \
                     VALUES ('entry', 'ev', 1, 1, 8, 0, 0, 0, 'image')",
                )
                .execute(conn)?;
                sql_query(
                    "INSERT INTO clipboard_snapshot_representation \
                     (id, event_id, format_id, mime_type, size_bytes, blob_id, payload_state) \
                     VALUES ('rep', 'ev', 'public.png', 'image/png', 8, 'shared', 'BlobReady')",
                )
                .execute(conn)?;
                sql_query(
                    "INSERT INTO clipboard_representation_thumbnail \
                     (representation_id, thumbnail_blob_id, thumbnail_mime_type, original_width, \
                      original_height, original_size_bytes) \
                     VALUES ('rep', 'shared', 'image/webp', 1, 1, 1)",
                )
                .execute(conn)?;
                sql_query(
                    "INSERT INTO entry_file_set (entry_id, line_index, kind, blob_id) \
                     VALUES ('entry', 0, 'file', 'shared')",
                )
                .execute(conn)?;
                Ok(())
            })
            .unwrap();

        for (label, sql) in [
            (
                "representation",
                "DELETE FROM clipboard_representation_thumbnail; DELETE FROM entry_file_set",
            ),
            (
                "thumbnail",
                "DELETE FROM clipboard_snapshot_representation; DELETE FROM entry_file_set; \
                 INSERT INTO clipboard_representation_thumbnail \
                 (representation_id, thumbnail_blob_id, thumbnail_mime_type, original_width, \
                  original_height, original_size_bytes) \
                 VALUES ('rep', 'shared', 'image/webp', 1, 1, 1)",
            ),
            (
                "file_set",
                "DELETE FROM clipboard_representation_thumbnail; \
                 DELETE FROM clipboard_snapshot_representation; \
                 INSERT INTO entry_file_set (entry_id, line_index, kind, blob_id) \
                 VALUES ('entry', 0, 'file', 'shared')",
            ),
        ] {
            executor
                .run({
                    let sql = sql.to_owned();
                    move |conn| {
                        conn.batch_execute(&sql)?;
                        Ok(())
                    }
                })
                .unwrap();
            let outcome = repository
                .delete_if_unreferenced(&BlobId::from("shared"))
                .await
                .unwrap();
            assert_eq!(outcome, UnreferencedBlobDeletion::Retained, "{label}");
            assert_eq!(blob_count(&executor, "shared"), 1, "{label}");
        }

        let still_referenced = executor
            .run(|conn| {
                let id = sql_query(
                    "SELECT blob_id AS id FROM clipboard_snapshot_representation WHERE id = 'rep'",
                )
                .get_result::<IdRow>(conn)
                .optional()?;
                Ok(id)
            })
            .unwrap();
        // 最后一轮删掉了表示，只留 file_set。表示不存在，所以没有被 SET NULL 的行。
        // 再插回表示，确认删除被拒绝后 blob_id 仍在。
        assert!(still_referenced.is_none());
        executor
            .run(|conn| {
                sql_query(
                    "INSERT INTO clipboard_snapshot_representation \
                     (id, event_id, format_id, mime_type, size_bytes, blob_id, payload_state) \
                     VALUES ('rep', 'ev', 'public.png', 'image/png', 8, 'shared', 'BlobReady')",
                )
                .execute(conn)?;
                Ok(())
            })
            .unwrap();
        assert_eq!(
            repository
                .delete_if_unreferenced(&BlobId::from("shared"))
                .await
                .unwrap(),
            UnreferencedBlobDeletion::Retained
        );
        let blob_id = executor
            .run(|conn| {
                let row = sql_query(
                    "SELECT blob_id AS id FROM clipboard_snapshot_representation WHERE id = 'rep'",
                )
                .get_result::<IdRow>(conn)?;
                Ok(row.id)
            })
            .unwrap();
        assert_eq!(blob_id, "shared");
    }

    #[derive(QueryableByName)]
    struct IdRow {
        #[diesel(sql_type = Text)]
        id: String,
    }

    #[tokio::test]
    async fn claim_updates_last_claimed_at_only_while_the_row_exists() {
        let (repository, dir) = repo();
        let pool = init_db_pool(dir.path().join("blobs.sqlite").to_str().unwrap()).unwrap();
        let executor = DieselSqliteExecutor::new(pool);
        let hash = live_hash();
        insert_blob(&executor, "live", &hash.to_string(), 3);
        let claimed = repository.claim_existing(&hash, 99).await.unwrap().unwrap();
        assert_eq!(claimed.last_claimed_at_ms, 99);
        assert_eq!(claimed.blob_id.as_ref(), "live");
        repository
            .delete_if_unreferenced(&BlobId::from("live"))
            .await
            .unwrap();
        assert!(repository
            .claim_existing(&hash, 100)
            .await
            .unwrap()
            .is_none());
    }
}
