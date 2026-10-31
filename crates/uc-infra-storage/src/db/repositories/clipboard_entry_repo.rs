use super::entry_receive_attempt_repo::delete_receive_state;
use crate::db::models::ClipboardEntryRow;
use crate::db::models::NewClipboardEntryRow;
use crate::db::models::NewClipboardSelectionRow;
use crate::db::ports::DbExecutor;
use crate::db::ports::{InsertMapper, RowMapper};
use crate::db::schema::{
    clipboard_entry, clipboard_event, clipboard_representation_thumbnail, clipboard_selection,
    clipboard_snapshot_representation, directory_publish_log, entry_file_set,
    entry_receive_attempt, receive_artifact_log,
};
use anyhow::Result;
use diesel::query_dsl::methods::FilterDsl;
use diesel::query_dsl::methods::LimitDsl;
use diesel::query_dsl::methods::OffsetDsl;
use diesel::query_dsl::methods::OrderDsl;
use diesel::query_dsl::methods::SelectDsl;
use diesel::ExpressionMethods;
use diesel::OptionalExtension;
use diesel::RunQueryDsl;
use diesel::SqliteConnection;
use diesel::{sql_types::Text, QueryableByName};
use diesel::{BoolExpressionMethods, Connection};
use tracing::instrument;
use uc_application::facade::clipboard_history::HistoryDeletionTargetsPort;
use uc_core::clipboard::{
    check_receive_deletion, ClipboardEntry, ClipboardRepositoryError, ClipboardSelectionDecision,
};
use uc_core::ids::{BlobId, EntryId};
use uc_core::ports::clipboard::{
    ClipboardEntryRemoval, DeleteClipboardEntryPort, DeleteClipboardEntryWithReceiveStatePort,
    FindEntryIdBySnapshotHashPort, GetClipboardEntryPort, GetEntrySnapshotHashPort,
    ListClipboardEntriesPort, SaveClipboardEntryPort, SetClipboardEntryFavoritePort,
    TouchClipboardEntryPort,
};
use uc_core::ports::{
    AttemptState, ClipboardEntryStore, ReceiveArtifactPhase, ReceiveArtifactResolution,
};

pub struct DieselClipboardEntryRepository<E, ME, MS, RE> {
    executor: E,
    entry_mapper: ME,
    selection_mapper: MS,
    row_entry_mapper: RE,
}

#[derive(QueryableByName)]
struct HistoryDeletionTarget {
    #[diesel(sql_type = Text)]
    entry_id: String,
}

#[async_trait::async_trait]
impl<E: DbExecutor, ME: Send + Sync, MS: Send + Sync, RE: Send + Sync> HistoryDeletionTargetsPort
    for DieselClipboardEntryRepository<E, ME, MS, RE>
{
    async fn history_deletion_targets(&self) -> Result<Vec<EntryId>> {
        self.executor.run(|conn| {
            let rows = diesel::sql_query(
                "SELECT entry_id FROM clipboard_entry UNION SELECT entry_id FROM entry_receive_attempt UNION SELECT entry_id FROM file_transfer WHERE entry_id IS NOT NULL UNION SELECT entry_id FROM receive_artifact_log UNION SELECT entry_id FROM directory_publish_log",
            ).load::<HistoryDeletionTarget>(conn)?;
            Ok(rows.into_iter().map(|row| EntryId::from(row.entry_id)).collect())
        })
    }
}

impl<E, ME, MS, RE> DieselClipboardEntryRepository<E, ME, MS, RE> {
    pub fn new(executor: E, entry_mapper: ME, selection_mapper: MS, row_entry_mapper: RE) -> Self {
        Self {
            executor,
            entry_mapper,
            selection_mapper,
            row_entry_mapper,
        }
    }
}

pub(super) fn insert_entry_and_selection_rows(
    conn: &mut SqliteConnection,
    entry: &NewClipboardEntryRow,
    selection: &NewClipboardSelectionRow,
) -> Result<()> {
    diesel::insert_into(clipboard_entry::table)
        .values(entry)
        .execute(conn)?;
    diesel::insert_into(clipboard_selection::table)
        .values(selection)
        .execute(conn)?;
    Ok(())
}

#[async_trait::async_trait]
impl<E, ME, MS, RE> ClipboardEntryStore for DieselClipboardEntryRepository<E, ME, MS, RE>
where
    E: DbExecutor,
    ME: InsertMapper<ClipboardEntry, NewClipboardEntryRow>,
    MS: InsertMapper<ClipboardSelectionDecision, NewClipboardSelectionRow>,
    RE: RowMapper<ClipboardEntryRow, ClipboardEntry>,
{
    #[instrument(
        name = "infra.sqlite.insert_clipboard_entry",
        skip_all,
        fields(
            operation = "save_entry",
            table = "clipboard_entry",
            entry_id = %entry.entry_id,
        )
    )]
    async fn save_entry_and_selection(
        &self,
        entry: &ClipboardEntry,
        selection: &ClipboardSelectionDecision,
    ) -> Result<()> {
        self.executor.run(|conn| {
            let new_entry_row = self.entry_mapper.to_row(entry)?;
            let new_selection_row = self.selection_mapper.to_row(selection)?;

            conn.transaction(|conn| {
                insert_entry_and_selection_rows(conn, &new_entry_row, &new_selection_row)
            })
        })
    }

    #[instrument(
        name = "infra.sqlite.query_clipboard_entry",
        skip_all,
        fields(
            operation = "get_entry",
            table = "clipboard_entry",
            entry_id = %entry_id,
        )
    )]
    async fn get_entry(&self, entry_id: &EntryId) -> Result<Option<ClipboardEntry>> {
        let entry_id_str = entry_id.to_string();
        self.executor.run(|conn| {
            let entry_row = clipboard_entry::table
                .filter(clipboard_entry::entry_id.eq(&entry_id_str))
                .first::<ClipboardEntryRow>(conn)
                .optional()?;

            match entry_row {
                Some(row) => {
                    let entry = self.row_entry_mapper.to_domain(&row)?;
                    Ok(Some(entry))
                }
                None => Ok(None),
            }
        })
    }

    /// Lists clipboard entries ordered by active time (newest first) with pagination.
    ///
    /// # Parameters
    ///
    /// - `limit`: Maximum number of entries to return.
    /// - `offset`: Number of entries to skip before collecting results (zero-based).
    ///
    /// # Returns
    ///
    /// A `Vec<ClipboardEntry>` containing entries ordered by `active_time_ms` descending.
    ///
    /// # Examples
    ///
    /// ```
    /// # use uc_core::ports::ClipboardEntryStore;
    /// # async fn example(repo: &impl ClipboardEntryStore) -> anyhow::Result<()> {
    /// let entries = repo.list_entries(10, 0).await?;
    /// assert!(entries.len() <= 10);
    /// # Ok(())
    /// # }
    /// ```
    #[instrument(
        name = "infra.sqlite.query_clipboard_entries",
        skip_all,
        fields(
            operation = "list_entries",
            table = "clipboard_entry",
            limit = limit,
            offset = offset,
        )
    )]
    async fn list_entries(&self, limit: usize, offset: usize) -> Result<Vec<ClipboardEntry>> {
        self.executor.run(|conn| {
            let entry_rows = clipboard_entry::table
                .order(clipboard_entry::active_time_ms.desc())
                .limit(limit as i64)
                .offset(offset as i64)
                .load::<ClipboardEntryRow>(conn)?;

            entry_rows
                .into_iter()
                .map(|row| self.row_entry_mapper.to_domain(&row))
                .collect()
        })
    }

    #[instrument(
        name = "infra.sqlite.touch_clipboard_entry",
        skip_all,
        fields(
            operation = "touch_entry",
            table = "clipboard_entry",
            entry_id = %entry_id,
            active_time_ms = active_time_ms,
        )
    )]
    async fn touch_entry(&self, entry_id: &EntryId, active_time_ms: i64) -> Result<bool> {
        self.executor.run(|conn| {
            use crate::db::schema::clipboard_entry::dsl;

            let affected = diesel::update(dsl::clipboard_entry)
                .filter(dsl::entry_id.eq(entry_id.to_string()))
                .set(dsl::active_time_ms.eq(active_time_ms))
                .execute(conn)?;

            Ok(affected > 0)
        })
    }

    #[instrument(
        name = "infra.sqlite.set_clipboard_entry_favorite",
        skip_all,
        fields(
            operation = "set_favorite",
            table = "clipboard_entry",
            entry_id = %entry_id,
            is_favorited = is_favorited,
        )
    )]
    async fn set_favorite(&self, entry_id: &EntryId, is_favorited: bool) -> Result<bool> {
        self.executor.run(|conn| {
            use crate::db::schema::clipboard_entry::dsl;

            let affected = diesel::update(dsl::clipboard_entry)
                .filter(dsl::entry_id.eq(entry_id.to_string()))
                .set(dsl::is_favorited.eq(is_favorited))
                .execute(conn)?;

            Ok(affected > 0)
        })
    }

    /// Deletes the clipboard entry with the given `EntryId` from the database.
    ///
    /// Attempts to remove the entry row whose `entry_id` matches `entry_id`. The operation returns `Ok(())` on success; if no row matches the provided `entry_id` the call still succeeds and returns `Ok(())`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use uc_core::ids::EntryId;
    /// # use uc_core::ports::ClipboardEntryStore;
    /// // Remove an entry by id
    /// # async fn run(repo: &impl ClipboardEntryStore, id: EntryId) -> anyhow::Result<()> {
    /// repo.delete_entry(&id).await?;
    /// # Ok(())
    /// # }
    /// ```
    #[instrument(
        name = "infra.sqlite.delete_clipboard_entry",
        skip_all,
        fields(
            operation = "delete_entry",
            table = "clipboard_entry",
            entry_id = %entry_id,
        )
    )]
    async fn delete_entry(&self, entry_id: &EntryId) -> Result<()> {
        let entry_id_str = entry_id.to_string();
        self.executor.run(|conn| {
            diesel::delete(clipboard_entry::table)
                .filter(clipboard_entry::entry_id.eq(&entry_id_str))
                .execute(conn)?;
            Ok(())
        })
    }

    /// Two-step lookup: first locate the event row for the given
    /// `snapshot_hash`, then locate the entry row whose `event_id`
    /// matches. Avoids `inner_join` import gymnastics (multiple
    /// `filter` candidates clash with the existing per-method DSL
    /// imports) while keeping the same dedup semantics.
    ///
    /// Two prepared statements vs one JOIN: trivially different cost on
    /// SQLite for an index-hit lookup (`snapshot_hash` is the natural
    /// dedup key), and Phase 3 calls this exactly once per inbound
    /// frame, so optimization is not warranted.
    #[instrument(
        name = "infra.sqlite.find_entry_by_snapshot_hash",
        skip_all,
        fields(
            operation = "find_entry_by_snapshot_hash",
            table = "clipboard_event + clipboard_entry",
            snapshot_hash_len = snapshot_hash.len(),
        )
    )]
    async fn find_entry_id_by_snapshot_hash(&self, snapshot_hash: &str) -> Result<Option<EntryId>> {
        let hash = snapshot_hash.to_string();
        self.executor.run(move |conn| {
            let event_id_str: Option<String> = clipboard_event::table
                .filter(clipboard_event::snapshot_hash.eq(&hash))
                .select(clipboard_event::event_id)
                .limit(1)
                .first::<String>(conn)
                .optional()?;

            let event_id_str = match event_id_str {
                Some(id) => id,
                None => return Ok(None),
            };

            let entry_id_str: Option<String> = clipboard_entry::table
                .filter(clipboard_entry::event_id.eq(&event_id_str))
                .select(clipboard_entry::entry_id)
                .limit(1)
                .first::<String>(conn)
                .optional()?;

            Ok(entry_id_str.map(EntryId::from))
        })
    }

    /// Forward lookup mirroring [`Self::find_entry_id_by_snapshot_hash`]: resolve
    /// `entry_id` to its event's persisted `snapshot_hash`. Two prepared
    /// statements (entry → `event_id`, then `event_id` → `snapshot_hash`) for
    /// the same reason as the reverse lookup — index-hit reads where a JOIN buys
    /// nothing and would clash with the per-method DSL imports.
    #[instrument(
        name = "infra.sqlite.get_entry_snapshot_hash",
        skip_all,
        fields(
            operation = "get_entry_snapshot_hash",
            table = "clipboard_entry + clipboard_event",
            entry_id = %entry_id,
        )
    )]
    async fn get_entry_snapshot_hash(&self, entry_id: &EntryId) -> Result<Option<String>> {
        let entry_id_str = entry_id.to_string();
        self.executor.run(move |conn| {
            let event_id_str: Option<String> = clipboard_entry::table
                .filter(clipboard_entry::entry_id.eq(&entry_id_str))
                .select(clipboard_entry::event_id)
                .limit(1)
                .first::<String>(conn)
                .optional()?;

            let event_id_str = match event_id_str {
                Some(id) => id,
                None => return Ok(None),
            };

            let snapshot_hash: Option<String> = clipboard_event::table
                .filter(clipboard_event::event_id.eq(&event_id_str))
                .select(clipboard_event::snapshot_hash)
                .limit(1)
                .first::<String>(conn)
                .optional()?;

            Ok(snapshot_hash)
        })
    }
}

// ---- Intent ports ------------------------------------------------------
//
// The single Diesel adapter is coerced into these narrow intent ports at the
// composition root. Each impl delegates to the aggregate trait above and
// translates the storage error into the typed domain error.

fn to_repo_err(e: anyhow::Error) -> ClipboardRepositoryError {
    ClipboardRepositoryError::Storage(e.context("clipboard repository operation").into())
}

#[async_trait::async_trait]
impl<E, ME, MS, RE> GetClipboardEntryPort for DieselClipboardEntryRepository<E, ME, MS, RE>
where
    E: DbExecutor,
    ME: InsertMapper<ClipboardEntry, NewClipboardEntryRow>,
    MS: InsertMapper<ClipboardSelectionDecision, NewClipboardSelectionRow>,
    RE: RowMapper<ClipboardEntryRow, ClipboardEntry>,
{
    async fn get_entry(
        &self,
        entry_id: &EntryId,
    ) -> Result<Option<ClipboardEntry>, ClipboardRepositoryError> {
        ClipboardEntryStore::get_entry(self, entry_id)
            .await
            .map_err(to_repo_err)
    }
}

#[async_trait::async_trait]
impl<E, ME, MS, RE> ListClipboardEntriesPort for DieselClipboardEntryRepository<E, ME, MS, RE>
where
    E: DbExecutor,
    ME: InsertMapper<ClipboardEntry, NewClipboardEntryRow>,
    MS: InsertMapper<ClipboardSelectionDecision, NewClipboardSelectionRow>,
    RE: RowMapper<ClipboardEntryRow, ClipboardEntry>,
{
    async fn list_entries(
        &self,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<ClipboardEntry>, ClipboardRepositoryError> {
        ClipboardEntryStore::list_entries(self, limit, offset)
            .await
            .map_err(to_repo_err)
    }
}

#[async_trait::async_trait]
impl<E, ME, MS, RE> SaveClipboardEntryPort for DieselClipboardEntryRepository<E, ME, MS, RE>
where
    E: DbExecutor,
    ME: InsertMapper<ClipboardEntry, NewClipboardEntryRow>,
    MS: InsertMapper<ClipboardSelectionDecision, NewClipboardSelectionRow>,
    RE: RowMapper<ClipboardEntryRow, ClipboardEntry>,
{
    async fn save_entry_and_selection(
        &self,
        entry: &ClipboardEntry,
        selection: &ClipboardSelectionDecision,
    ) -> Result<(), ClipboardRepositoryError> {
        ClipboardEntryStore::save_entry_and_selection(self, entry, selection)
            .await
            .map_err(to_repo_err)
    }
}

#[async_trait::async_trait]
impl<E, ME, MS, RE> TouchClipboardEntryPort for DieselClipboardEntryRepository<E, ME, MS, RE>
where
    E: DbExecutor,
    ME: InsertMapper<ClipboardEntry, NewClipboardEntryRow>,
    MS: InsertMapper<ClipboardSelectionDecision, NewClipboardSelectionRow>,
    RE: RowMapper<ClipboardEntryRow, ClipboardEntry>,
{
    async fn touch_entry(
        &self,
        entry_id: &EntryId,
        active_time_ms: i64,
    ) -> Result<bool, ClipboardRepositoryError> {
        ClipboardEntryStore::touch_entry(self, entry_id, active_time_ms)
            .await
            .map_err(to_repo_err)
    }
}

#[async_trait::async_trait]
impl<E, ME, MS, RE> SetClipboardEntryFavoritePort for DieselClipboardEntryRepository<E, ME, MS, RE>
where
    E: DbExecutor,
    ME: InsertMapper<ClipboardEntry, NewClipboardEntryRow>,
    MS: InsertMapper<ClipboardSelectionDecision, NewClipboardSelectionRow>,
    RE: RowMapper<ClipboardEntryRow, ClipboardEntry>,
{
    async fn set_favorite(
        &self,
        entry_id: &EntryId,
        is_favorited: bool,
    ) -> Result<bool, ClipboardRepositoryError> {
        ClipboardEntryStore::set_favorite(self, entry_id, is_favorited)
            .await
            .map_err(to_repo_err)
    }
}

#[async_trait::async_trait]
impl<E, ME, MS, RE> DeleteClipboardEntryPort for DieselClipboardEntryRepository<E, ME, MS, RE>
where
    E: DbExecutor,
    ME: InsertMapper<ClipboardEntry, NewClipboardEntryRow>,
    MS: InsertMapper<ClipboardSelectionDecision, NewClipboardSelectionRow>,
    RE: RowMapper<ClipboardEntryRow, ClipboardEntry>,
{
    async fn delete_entry(&self, entry_id: &EntryId) -> Result<(), ClipboardRepositoryError> {
        ClipboardEntryStore::delete_entry(self, entry_id)
            .await
            .map_err(to_repo_err)
    }
}

#[async_trait::async_trait]
impl<E, ME, MS, RE> DeleteClipboardEntryWithReceiveStatePort
    for DieselClipboardEntryRepository<E, ME, MS, RE>
where
    E: DbExecutor,
    ME: InsertMapper<ClipboardEntry, NewClipboardEntryRow>,
    MS: InsertMapper<ClipboardSelectionDecision, NewClipboardSelectionRow>,
    RE: RowMapper<ClipboardEntryRow, ClipboardEntry>,
{
    async fn delete_entry_with_receive_state(
        &self,
        entry_id: &EntryId,
    ) -> Result<ClipboardEntryRemoval, ClipboardRepositoryError> {
        let entry_id = entry_id.to_string();
        self.executor
            .run(move |conn| {
                // 先取得写权限再读取删除前提，避免读后升级在短写锁竞争下
                // 立即返回 SQLITE_BUSY，绕过连接既有的写锁等待预算。
                conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                    let attempt_state = entry_receive_attempt::table
                        .filter(entry_receive_attempt::entry_id.eq(&entry_id))
                        .select(entry_receive_attempt::attempt_state)
                        .first::<String>(conn)
                        .optional()?;
                    let attempt_state = attempt_state
                        .as_deref()
                        .map(str::parse::<AttemptState>)
                        .transpose()?;
                    let unsettled_artifacts = receive_artifact_log::table
                        .filter(receive_artifact_log::entry_id.eq(&entry_id))
                        .filter(
                            receive_artifact_log::phase
                                .ne(ReceiveArtifactPhase::Settled.as_str())
                                .or(receive_artifact_log::resolution.ne_all([
                                    ReceiveArtifactResolution::Landed.as_str(),
                                    ReceiveArtifactResolution::RolledBack.as_str(),
                                ])),
                        )
                        .select(receive_artifact_log::entry_id)
                        .first::<String>(conn)
                        .optional()?
                        .is_some();
                    let unpublished = directory_publish_log::table
                        .filter(directory_publish_log::entry_id.eq(&entry_id))
                        .filter(directory_publish_log::landed.eq(false))
                        .select(directory_publish_log::attempt_id)
                        .load::<String>(conn)?;
                    let rolled_back = receive_artifact_log::table
                        .filter(receive_artifact_log::entry_id.eq(&entry_id))
                        .filter(
                            receive_artifact_log::phase.eq(ReceiveArtifactPhase::Settled.as_str()),
                        )
                        .filter(
                            receive_artifact_log::resolution
                                .eq(ReceiveArtifactResolution::RolledBack.as_str()),
                        )
                        .select(receive_artifact_log::attempt_id)
                        .load::<String>(conn)?;
                    check_receive_deletion(
                        attempt_state,
                        unsettled_artifacts,
                        &unpublished,
                        &rolled_back,
                    )?;

                    let event_id = clipboard_entry::table
                        .filter(clipboard_entry::entry_id.eq(&entry_id))
                        .select(clipboard_entry::event_id)
                        .first::<String>(conn)
                        .optional()?;
                    let had_entry = event_id.is_some();
                    // 缩略图没有级联。先收集三类候选并删掉缩略图行，再删条目
                    // （文件清单随条目 CASCADE）和表示。回收发生在事务提交之后。
                    let candidate_blob_ids = event_id
                        .as_deref()
                        .map(|event_id| collect_entry_blob_ids(conn, &entry_id, event_id))
                        .transpose()?
                        .unwrap_or_default();
                    if let Some(event_id) = event_id {
                        diesel::delete(
                            clipboard_selection::table
                                .filter(clipboard_selection::entry_id.eq(&entry_id)),
                        )
                        .execute(conn)?;
                        diesel::delete(
                            clipboard_entry::table.filter(clipboard_entry::entry_id.eq(&entry_id)),
                        )
                        .execute(conn)?;
                        diesel::delete(
                            clipboard_snapshot_representation::table
                                .filter(clipboard_snapshot_representation::event_id.eq(&event_id)),
                        )
                        .execute(conn)?;
                        diesel::delete(
                            clipboard_event::table.filter(clipboard_event::event_id.eq(&event_id)),
                        )
                        .execute(conn)?;
                    }
                    let had_receive = delete_receive_state(conn, &entry_id)?;
                    Ok(ClipboardEntryRemoval {
                        removed: had_entry || had_receive,
                        candidate_blob_ids,
                    })
                })
            })
            .map_err(|error| {
                ClipboardRepositoryError::Storage(
                    error.context("delete entry with receive state").into(),
                )
            })
    }
}

/// 收集并拆掉本条目上的 blob 引用。缩略图行在这里删除；文件清单行留给
/// 随后的条目 CASCADE。表示行由调用方在同一事务里删除。
fn collect_entry_blob_ids(
    conn: &mut SqliteConnection,
    entry_id: &str,
    event_id: &str,
) -> anyhow::Result<Vec<BlobId>> {
    let representation_ids = clipboard_snapshot_representation::table
        .filter(clipboard_snapshot_representation::event_id.eq(event_id))
        .select(clipboard_snapshot_representation::id)
        .load::<String>(conn)?;
    let mut blob_ids = clipboard_snapshot_representation::table
        .filter(clipboard_snapshot_representation::event_id.eq(event_id))
        .select(clipboard_snapshot_representation::blob_id)
        .load::<Option<String>>(conn)?
        .into_iter()
        .flatten()
        .map(BlobId::from)
        .collect::<Vec<_>>();
    if !representation_ids.is_empty() {
        let thumbnail_blob_ids = clipboard_representation_thumbnail::table
            .filter(
                clipboard_representation_thumbnail::representation_id.eq_any(&representation_ids),
            )
            .select(clipboard_representation_thumbnail::thumbnail_blob_id)
            .load::<String>(conn)?;
        diesel::delete(clipboard_representation_thumbnail::table.filter(
            clipboard_representation_thumbnail::representation_id.eq_any(&representation_ids),
        ))
        .execute(conn)?;
        blob_ids.extend(thumbnail_blob_ids.into_iter().map(BlobId::from));
    }
    let file_blob_ids = entry_file_set::table
        .filter(entry_file_set::entry_id.eq(entry_id))
        .select(entry_file_set::blob_id)
        .load::<Option<String>>(conn)?;
    blob_ids.extend(file_blob_ids.into_iter().flatten().map(BlobId::from));
    Ok(blob_ids)
}

#[async_trait::async_trait]
impl<E, ME, MS, RE> FindEntryIdBySnapshotHashPort for DieselClipboardEntryRepository<E, ME, MS, RE>
where
    E: DbExecutor,
    ME: InsertMapper<ClipboardEntry, NewClipboardEntryRow>,
    MS: InsertMapper<ClipboardSelectionDecision, NewClipboardSelectionRow>,
    RE: RowMapper<ClipboardEntryRow, ClipboardEntry>,
{
    async fn find_entry_id_by_snapshot_hash(
        &self,
        snapshot_hash: &str,
    ) -> Result<Option<EntryId>, ClipboardRepositoryError> {
        ClipboardEntryStore::find_entry_id_by_snapshot_hash(self, snapshot_hash)
            .await
            .map_err(to_repo_err)
    }
}

#[async_trait::async_trait]
impl<E, ME, MS, RE> GetEntrySnapshotHashPort for DieselClipboardEntryRepository<E, ME, MS, RE>
where
    E: DbExecutor,
    ME: InsertMapper<ClipboardEntry, NewClipboardEntryRow>,
    MS: InsertMapper<ClipboardSelectionDecision, NewClipboardSelectionRow>,
    RE: RowMapper<ClipboardEntryRow, ClipboardEntry>,
{
    async fn get_entry_snapshot_hash(
        &self,
        entry_id: &EntryId,
    ) -> Result<Option<String>, ClipboardRepositoryError> {
        ClipboardEntryStore::get_entry_snapshot_hash(self, entry_id)
            .await
            .map_err(to_repo_err)
    }
}

#[cfg(test)]
mod tests {
    mod write_contention;

    use super::*;
    use crate::db::executor::DieselSqliteExecutor;
    use crate::db::mappers::clipboard_entry_mapper::ClipboardEntryRowMapper;
    use crate::db::mappers::clipboard_selection_mapper::ClipboardSelectionRowMapper;
    use crate::db::models::{NewClipboardEntryRow, NewClipboardEventRow};
    use crate::db::pool::init_db_pool;
    use crate::db::ports::DbExecutor;
    use tempfile::{tempdir, TempDir};
    use uc_core::ids::EventId;

    type Repo = DieselClipboardEntryRepository<
        DieselSqliteExecutor,
        ClipboardEntryRowMapper,
        ClipboardSelectionRowMapper,
        ClipboardEntryRowMapper,
    >;

    /// Build a repo + a second executor (sharing the same pool so
    /// direct-insert fixtures land in the same in-memory DB as the repo
    /// reads). `DieselSqliteExecutor::new` takes a pool by value, so we
    /// build two executors from the same file-backed pool.
    fn make_repo() -> (Repo, DieselSqliteExecutor, TempDir) {
        let tempdir = tempdir().unwrap();
        let database_url = tempdir.path().join("entry-repo.sqlite");
        let path = database_url.to_str().unwrap();
        let pool_for_repo = init_db_pool(path).unwrap();
        let pool_for_seed = init_db_pool(path).unwrap();
        let repo = DieselClipboardEntryRepository::new(
            DieselSqliteExecutor::new(pool_for_repo),
            ClipboardEntryRowMapper,
            ClipboardSelectionRowMapper,
            ClipboardEntryRowMapper,
        );
        (repo, DieselSqliteExecutor::new(pool_for_seed), tempdir)
    }

    /// Seed one `clipboard_event` + `clipboard_entry` row pair carrying
    /// the given `snapshot_hash`. Bypasses the mapper pipeline — this
    /// test only exercises the read-side lookup contract, not the write
    /// path (which is covered by higher-level use case tests).
    fn seed_event_and_entry(executor: &DieselSqliteExecutor, snapshot_hash: &str) -> String {
        use crate::db::schema::{clipboard_entry, clipboard_event};

        let event_id = format!("ev-{}", uuid::Uuid::new_v4());
        let entry_id = format!("entry-{}", uuid::Uuid::new_v4());

        let event_row = NewClipboardEventRow {
            event_id: event_id.clone(),
            captured_at_ms: 1_700_000_000_000,
            source_device: "test-device".into(),
            snapshot_hash: snapshot_hash.into(),
        };
        let entry_row = NewClipboardEntryRow {
            entry_id: entry_id.clone(),
            event_id: event_id.clone(),
            created_at_ms: 1_700_000_000_000,
            active_time_ms: 1_700_000_000_000,
            total_size: 0,
            pinned: false,
            delivery_tracked: false,
            is_favorited: false,
            content_category: "text".into(),
        };

        executor
            .run(move |conn| {
                diesel::insert_into(clipboard_event::table)
                    .values(&event_row)
                    .execute(conn)?;
                diesel::insert_into(clipboard_entry::table)
                    .values(&entry_row)
                    .execute(conn)?;
                Ok(())
            })
            .unwrap();

        entry_id
    }

    fn seed_entry_with_receive_state(
        executor: &DieselSqliteExecutor,
        attempt_state_value: &str,
        artifact_resolution: &str,
    ) -> (EntryId, EventId) {
        use crate::db::schema::{
            clipboard_entry, clipboard_selection, clipboard_snapshot_representation,
            entry_receive_attempt, receive_artifact_log,
        };

        let entry_id = seed_event_and_entry(executor, &format!("blake3v1:{}", "12".repeat(32)));
        let event_id = executor
            .run({
                let entry_id = entry_id.clone();
                move |conn| {
                    clipboard_entry::table
                        .filter(clipboard_entry::entry_id.eq(entry_id))
                        .select(clipboard_entry::event_id)
                        .first::<String>(conn)
                        .map_err(Into::into)
                }
            })
            .unwrap();
        let attempt_state_value = attempt_state_value.to_owned();
        let artifact_resolution = artifact_resolution.to_owned();
        executor
            .run({
                let entry_id = entry_id.clone();
                let event_id = event_id.clone();
                move |conn| {
                    diesel::insert_into(clipboard_snapshot_representation::table)
                        .values((
                            clipboard_snapshot_representation::id.eq("rep"),
                            clipboard_snapshot_representation::event_id.eq(&event_id),
                            clipboard_snapshot_representation::format_id.eq("text"),
                            clipboard_snapshot_representation::mime_type.eq(Some("text/plain")),
                            clipboard_snapshot_representation::size_bytes.eq(5_i64),
                            clipboard_snapshot_representation::inline_data
                                .eq(Some(b"hello".to_vec())),
                            clipboard_snapshot_representation::blob_id.eq::<Option<&str>>(None),
                            clipboard_snapshot_representation::payload_state.eq("Inline"),
                            clipboard_snapshot_representation::last_error.eq::<Option<&str>>(None),
                        ))
                        .execute(conn)?;
                    diesel::insert_into(clipboard_selection::table)
                        .values((
                            clipboard_selection::entry_id.eq(&entry_id),
                            clipboard_selection::primary_rep_id.eq("rep"),
                            clipboard_selection::secondary_rep_ids.eq("[]"),
                            clipboard_selection::preview_rep_id.eq("rep"),
                            clipboard_selection::paste_rep_id.eq("rep"),
                            clipboard_selection::policy_version.eq("v1"),
                        ))
                        .execute(conn)?;
                    diesel::insert_into(entry_receive_attempt::table)
                        .values((
                            entry_receive_attempt::entry_id.eq(&entry_id),
                            entry_receive_attempt::current_attempt_id.eq("attempt"),
                            entry_receive_attempt::attempt_state.eq(attempt_state_value),
                            entry_receive_attempt::updated_at_ms.eq(1_i64),
                        ))
                        .execute(conn)?;
                    diesel::insert_into(receive_artifact_log::table)
                        .values((
                            receive_artifact_log::entry_id.eq(&entry_id),
                            receive_artifact_log::attempt_id.eq("attempt"),
                            receive_artifact_log::phase.eq("settled"),
                            receive_artifact_log::resolution.eq(artifact_resolution),
                            receive_artifact_log::artifact_ciphertext.eq(vec![1_u8, 2, 3]),
                            receive_artifact_log::updated_at_ms.eq(1_i64),
                        ))
                        .execute(conn)?;
                    Ok(())
                }
            })
            .unwrap();

        (EntryId::from(entry_id), EventId::from(event_id))
    }

    fn assert_receive_delete_fixture_counts(executor: &DieselSqliteExecutor, expected: i64) {
        use crate::db::schema::{
            clipboard_entry, clipboard_event, clipboard_selection,
            clipboard_snapshot_representation, entry_receive_attempt, receive_artifact_log,
        };

        executor
            .run(move |conn| {
                assert_eq!(
                    clipboard_entry::table
                        .select(diesel::dsl::count_star())
                        .first::<i64>(conn)?,
                    expected
                );
                assert_eq!(
                    clipboard_event::table
                        .select(diesel::dsl::count_star())
                        .first::<i64>(conn)?,
                    expected
                );
                assert_eq!(
                    clipboard_selection::table
                        .select(diesel::dsl::count_star())
                        .first::<i64>(conn)?,
                    expected
                );
                assert_eq!(
                    clipboard_snapshot_representation::table
                        .select(diesel::dsl::count_star())
                        .first::<i64>(conn)?,
                    expected
                );
                assert_eq!(
                    entry_receive_attempt::table
                        .select(diesel::dsl::count_star())
                        .first::<i64>(conn)?,
                    expected
                );
                assert_eq!(
                    receive_artifact_log::table
                        .select(diesel::dsl::count_star())
                        .first::<i64>(conn)?,
                    expected
                );
                Ok(())
            })
            .unwrap();
    }

    #[tokio::test]
    async fn find_entry_id_by_snapshot_hash_returns_existing_entry() {
        let (repo, executor, _tempdir) = make_repo();
        let hash = "blake3v1:deadbeef00000000000000000000000000000000000000000000000000000000";
        let expected_entry_id = seed_event_and_entry(&executor, hash);

        let actual = ClipboardEntryStore::find_entry_id_by_snapshot_hash(&repo, hash)
            .await
            .expect("query ok");
        assert_eq!(
            actual.map(|e| e.into_inner()),
            Some(expected_entry_id),
            "existing snapshot_hash must resolve to the seeded entry_id"
        );
    }

    #[tokio::test]
    async fn find_entry_id_by_snapshot_hash_returns_none_for_missing() {
        let (repo, _executor, _tempdir) = make_repo();
        let result = ClipboardEntryStore::find_entry_id_by_snapshot_hash(
            &repo,
            "blake3v1:ffffffff00000000000000000000000000000000000000000000000000000000",
        )
        .await
        .expect("query ok");
        assert!(result.is_none(), "unknown hash must return None");
    }

    #[tokio::test]
    async fn get_entry_snapshot_hash_returns_persisted_hash_for_existing_entry() {
        let (repo, executor, _tempdir) = make_repo();
        let hash = "blake3v1:520d15b800000000000000000000000000000000000000000000000000000000";
        let entry_id = seed_event_and_entry(&executor, hash);

        let actual = ClipboardEntryStore::get_entry_snapshot_hash(&repo, &EntryId::from(entry_id))
            .await
            .expect("query ok");
        assert_eq!(
            actual.as_deref(),
            Some(hash),
            "an existing entry must resolve to its event's persisted snapshot_hash \
             (the inverse of find_entry_id_by_snapshot_hash)"
        );
    }

    #[tokio::test]
    async fn get_entry_snapshot_hash_returns_none_for_missing_entry() {
        let (repo, _executor, _tempdir) = make_repo();
        let result =
            ClipboardEntryStore::get_entry_snapshot_hash(&repo, &EntryId::from("entry-nope"))
                .await
                .expect("query ok");
        assert!(result.is_none(), "unknown entry must return None");
    }

    #[tokio::test]
    async fn set_favorite_persists_and_round_trips_through_get_entry() {
        let (repo, executor, _tempdir) = make_repo();
        let hash = "blake3v1:fa00000000000000000000000000000000000000000000000000000000000000";
        let entry_id = EntryId::from(seed_event_and_entry(&executor, hash));

        let before = ClipboardEntryStore::get_entry(&repo, &entry_id)
            .await
            .expect("query ok")
            .expect("entry exists");
        assert!(
            !before.is_favorited,
            "seeded entry defaults to not favorited"
        );

        let updated = ClipboardEntryStore::set_favorite(&repo, &entry_id, true)
            .await
            .expect("update ok");
        assert!(
            updated,
            "setting favorite on an existing entry reports a row update"
        );

        let after = ClipboardEntryStore::get_entry(&repo, &entry_id)
            .await
            .expect("query ok")
            .expect("entry exists");
        assert!(
            after.is_favorited,
            "favorite flag must persist and round-trip"
        );

        let cleared = ClipboardEntryStore::set_favorite(&repo, &entry_id, false)
            .await
            .expect("update ok");
        assert!(
            cleared,
            "clearing favorite on an existing entry still reports a row update"
        );

        let reloaded = ClipboardEntryStore::get_entry(&repo, &entry_id)
            .await
            .expect("query ok")
            .expect("entry exists");
        assert!(
            !reloaded.is_favorited,
            "cleared favorite flag must round-trip back to false"
        );
    }

    #[tokio::test]
    async fn set_favorite_returns_false_for_missing_entry() {
        let (repo, _executor, _tempdir) = make_repo();
        let updated = ClipboardEntryStore::set_favorite(&repo, &EntryId::from("entry-nope"), true)
            .await
            .expect("update ok");
        assert!(
            !updated,
            "favoriting an unknown entry must report no row update"
        );
    }

    #[tokio::test]
    async fn history_clear_targets_include_unsaved_and_orphan_state_without_duplicate_entries() {
        let (repo, executor, _tempdir) = make_repo();
        let (entry, _) = seed_entry_with_receive_state(&executor, "completed", "landed");
        executor.run(|conn| {
            diesel::sql_query("INSERT INTO entry_receive_attempt (entry_id, current_attempt_id, attempt_state, updated_at_ms) VALUES ('unsaved', 'attempt', 'failed', 0)").execute(conn)?;
            diesel::sql_query("INSERT INTO file_transfer (transfer_id, entry_id, binding_state, receive_item_id, status, source_device, metadata_ciphertext, created_at_ms, updated_at_ms) VALUES ('orphan-transfer', 'orphan', 'legacy', 'item', 'failed', 'peer', X'00', 0, 0)").execute(conn)?;
            diesel::sql_query("INSERT INTO receive_artifact_log (entry_id, attempt_id, phase, resolution, artifact_ciphertext, updated_at_ms) VALUES ('artifact-only', 'old', 'settled', 'rolled_back', X'00', 0)").execute(conn)?;
            diesel::sql_query("INSERT INTO directory_publish_log (entry_id, attempt_id, phase, root_map_ciphertext, partial_publication, partial_root_count, landed, updated_at_ms) VALUES ('directory-only', 'old', 'landed', X'00', 0, 0, 1, 0)").execute(conn)?;
            Ok(())
        }).unwrap();
        let targets = repo.history_deletion_targets().await.unwrap();
        assert_eq!(targets.len(), 5);
        for id in [
            entry,
            EntryId::from("unsaved"),
            EntryId::from("orphan"),
            EntryId::from("artifact-only"),
            EntryId::from("directory-only"),
        ] {
            assert!(targets.contains(&id));
            assert!(
                repo.delete_entry_with_receive_state(&id)
                    .await
                    .unwrap()
                    .removed
            );
        }
        assert!(repo.history_deletion_targets().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn combined_delete_removes_terminal_entry_and_receive_state() {
        let (repo, executor, _tempdir) = make_repo();
        let (entry_id, _event_id) = seed_entry_with_receive_state(&executor, "completed", "landed");

        repo.delete_entry_with_receive_state(&entry_id)
            .await
            .unwrap();

        assert_receive_delete_fixture_counts(&executor, 0);
    }

    #[tokio::test]
    async fn combined_delete_cleans_unsaved_receive_atomically_and_reports_actual_absence() {
        for refusal in [
            None,
            Some("running"),
            Some("pending"),
            Some("unsettled_phase"),
            Some("write_failure"),
            Some("orphan"),
            Some("publication"),
            Some("rolled_back_publication"),
            Some("wrong_attempt_publication"),
        ] {
            let (repo, executor, _tempdir) = make_repo();
            let (entry_id, event_id) = seed_entry_with_receive_state(
                &executor,
                if refusal == Some("running") {
                    "receiving"
                } else {
                    "failed"
                },
                if refusal == Some("pending") {
                    "pending"
                } else {
                    "rolled_back"
                },
            );
            let raw_id = entry_id.to_string();
            executor.run(move |conn| {
                diesel::delete(clipboard_selection::table).execute(conn)?;
                diesel::delete(clipboard_entry::table).execute(conn)?;
                diesel::delete(clipboard_snapshot_representation::table).execute(conn)?;
                diesel::delete(clipboard_event::table.filter(clipboard_event::event_id.eq(event_id.to_string()))).execute(conn)?;
                diesel::sql_query("INSERT INTO file_transfer (transfer_id, entry_id, binding_state, receive_item_id, status, source_device, metadata_ciphertext, created_at_ms, updated_at_ms) VALUES ('unsaved-transfer', ?, 'attempt_bound', 'item', 'failed', 'peer', X'00', 0, 0)")
                    .bind::<diesel::sql_types::Text, _>(raw_id).execute(conn)?;
                diesel::sql_query("INSERT INTO file_transfer_events (transfer_id, sequence, event_type, payload_ciphertext, occurred_at_ms) VALUES ('unsaved-transfer', 0, 'started', X'00', 0)").execute(conn)?;
                if matches!(refusal, Some("publication" | "rolled_back_publication" | "wrong_attempt_publication")) {
                    if refusal == Some("publication") {
                        diesel::delete(receive_artifact_log::table).execute(conn)?;
                    } else {
                        diesel::sql_query("UPDATE receive_artifact_log SET phase = 'settled'").execute(conn)?;
                    }
                    diesel::sql_query("INSERT INTO directory_publish_log (entry_id, attempt_id, phase, root_map_ciphertext, partial_publication, partial_root_count, landed, updated_at_ms) SELECT entry_id, current_attempt_id, 'publishing', X'00', 1, 1, 0, 0 FROM entry_receive_attempt").execute(conn)?;
                }
                if refusal == Some("wrong_attempt_publication") {
                    diesel::sql_query("UPDATE directory_publish_log SET attempt_id = 'other-attempt'").execute(conn)?;
                }
                if refusal == Some("unsettled_phase") {
                    diesel::sql_query("UPDATE receive_artifact_log SET phase = 'rolling_back'").execute(conn)?;
                }
                if refusal == Some("orphan") {
                    diesel::delete(entry_receive_attempt::table).execute(conn)?;
                    diesel::delete(receive_artifact_log::table).execute(conn)?;
                }
                if refusal == Some("write_failure") {
                    diesel::sql_query("CREATE TRIGGER fail_receive_delete BEFORE DELETE ON entry_receive_attempt BEGIN SELECT RAISE(ABORT, 'injected receive deletion failure'); END").execute(conn)?;
                }
                Ok(())
            }).unwrap();
            let result = repo.delete_entry_with_receive_state(&entry_id).await;
            if matches!(refusal, None | Some("orphan" | "rolled_back_publication")) {
                assert!(result.unwrap().removed);
                assert!(
                    !repo
                        .delete_entry_with_receive_state(&entry_id)
                        .await
                        .unwrap()
                        .removed
                );
            } else {
                let error = result.unwrap_err();
                assert!(std::error::Error::source(&error).is_some());
                if refusal == Some("write_failure") {
                    let mut cause: &(dyn std::error::Error + 'static) = &error;
                    while !matches!(
                        cause.downcast_ref::<diesel::result::Error>(),
                        Some(diesel::result::Error::DatabaseError(_, _))
                    ) {
                        cause = cause.source().expect("必须保留实际SQLite删除失败来源");
                    }
                }
            }
            executor
                .run(move |conn| {
                    use crate::db::schema::{
                        directory_publish_log, file_transfer, file_transfer_events,
                    };
                    let expected = i64::from(!matches!(
                        refusal,
                        None | Some("orphan" | "rolled_back_publication")
                    ));
                    assert_eq!(
                        clipboard_entry::table
                            .select(diesel::dsl::count_star())
                            .get_result::<i64>(conn)?,
                        0
                    );
                    assert_eq!(
                        entry_receive_attempt::table
                            .select(diesel::dsl::count_star())
                            .get_result::<i64>(conn)?,
                        expected
                    );
                    assert_eq!(
                        receive_artifact_log::table
                            .select(diesel::dsl::count_star())
                            .get_result::<i64>(conn)?,
                        if refusal == Some("publication") {
                            0
                        } else {
                            expected
                        }
                    );
                    assert_eq!(
                        file_transfer::table
                            .select(diesel::dsl::count_star())
                            .get_result::<i64>(conn)?,
                        expected
                    );
                    assert_eq!(
                        file_transfer_events::table
                            .select(diesel::dsl::count_star())
                            .get_result::<i64>(conn)?,
                        expected
                    );
                    assert_eq!(
                        directory_publish_log::table
                            .select(diesel::dsl::count_star())
                            .get_result::<i64>(conn)?,
                        i64::from(matches!(
                            refusal,
                            Some("publication" | "wrong_attempt_publication")
                        ))
                    );
                    Ok(())
                })
                .unwrap();
        }
    }

    #[tokio::test]
    async fn combined_delete_rejects_nonterminal_receive_without_changes() {
        let (repo, executor, _tempdir) = make_repo();
        let (entry_id, _event_id) = seed_entry_with_receive_state(&executor, "receiving", "landed");

        assert!(repo
            .delete_entry_with_receive_state(&entry_id)
            .await
            .is_err());

        assert_receive_delete_fixture_counts(&executor, 1);
    }

    #[tokio::test]
    async fn combined_delete_loses_to_an_attempt_that_is_committing() {
        let (repo, executor, _tempdir) = make_repo();
        let (entry_id, _event_id) =
            seed_entry_with_receive_state(&executor, "committing", "landed");

        assert!(repo
            .delete_entry_with_receive_state(&entry_id)
            .await
            .is_err());

        assert_receive_delete_fixture_counts(&executor, 1);
    }

    #[tokio::test]
    async fn combined_delete_rejects_pending_artifacts_without_changes() {
        let (repo, executor, _tempdir) = make_repo();
        let (entry_id, _event_id) =
            seed_entry_with_receive_state(&executor, "completed", "pending");

        assert!(repo
            .delete_entry_with_receive_state(&entry_id)
            .await
            .is_err());

        assert_receive_delete_fixture_counts(&executor, 1);
    }

    #[tokio::test]
    async fn combined_delete_rolls_back_every_table_when_a_delete_fails() {
        let (repo, executor, _tempdir) = make_repo();
        let (entry_id, _event_id) = seed_entry_with_receive_state(&executor, "completed", "landed");
        executor
            .run(|conn| {
                diesel::sql_query(
                    "CREATE TRIGGER fail_event_delete BEFORE DELETE ON clipboard_event \
                     BEGIN SELECT RAISE(ABORT, 'injected delete failure'); END",
                )
                .execute(conn)?;
                Ok(())
            })
            .unwrap();

        assert!(repo
            .delete_entry_with_receive_state(&entry_id)
            .await
            .is_err());

        assert_receive_delete_fixture_counts(&executor, 1);
    }
}
