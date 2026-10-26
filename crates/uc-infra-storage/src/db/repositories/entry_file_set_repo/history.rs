//! 只读已有投递归属；所有路径和内容仍经既有认证仓储读取。
use super::DieselEntryFileSetRepository;
use crate::db::models::{
    clipboard_selection::ClipboardSelectionRow, entry_file_set::EntryFileSetRow,
};
use crate::db::schema::{clipboard_selection, clipboard_snapshot_representation, entry_file_set};
use crate::db::{
    ports::DbExecutor,
    schema::{clipboard_entry, clipboard_entry_delivery, clipboard_event},
};
use async_trait::async_trait;
use diesel::{
    Connection, ExpressionMethods, OptionalExtension, QueryDsl, RunQueryDsl, SqliteConnection,
};
use tokio::task::spawn_blocking;
use tracing::Span;
use uc_application::deps::{
    ProgressHistoryEvidence, ProgressHistoryReadError, ProgressHistoryRepresentation,
    ReadProgressHistoryPort,
};
use uc_core::clipboard::{EntryFileSet, MimeType};
use uc_core::file_transfer::{PROJECTION_GLOBAL_CAPACITY, PROJECTION_PEER_CAPACITY};
use uc_core::ids::{DeviceId, EntryId};

impl<E: DbExecutor + Clone + 'static> DieselEntryFileSetRepository<E> {
    async fn read_history<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut SqliteConnection) -> anyhow::Result<T> + Send + 'static,
    ) -> Result<T, ProgressHistoryReadError> {
        let executor = self.executor.clone();
        let span = Span::current();
        spawn_blocking(move || span.in_scope(|| executor.run(operation)))
            .await
            .map_err(|source| ProgressHistoryReadError(source.into()))?
            .map_err(ProgressHistoryReadError)
    }
}
#[async_trait]
impl<E: DbExecutor + Clone + 'static> ReadProgressHistoryPort for DieselEntryFileSetRepository<E> {
    async fn list_owned(
        &self,
        local: DeviceId,
        peers: &[DeviceId],
    ) -> Result<Vec<(DeviceId, EntryId)>, ProgressHistoryReadError> {
        let mut peers = peers.to_vec();
        peers.sort_unstable();
        peers.dedup();
        let mut result = Vec::new();
        for peer in peers {
            let local_text = local.to_string();
            let peer_text = peer.to_string();
            let remaining = PROJECTION_GLOBAL_CAPACITY.saturating_sub(result.len());
            if remaining == 0 {
                break;
            }
            let limit = remaining.min(PROJECTION_PEER_CAPACITY) as i64;
            let entries = self
                .read_history(move |conn| {
                    Ok(clipboard_entry_delivery::table
                        .inner_join(clipboard_entry::table.inner_join(clipboard_event::table))
                        .filter(clipboard_event::source_device.eq(local_text))
                        .filter(clipboard_entry_delivery::target_device_id.eq(peer_text))
                        .filter(clipboard_entry::deleted_at_ms.is_null())
                        .filter(clipboard_entry::delivery_tracked.eq(true))
                        .filter(clipboard_entry::content_category.eq_any(["file", "image"]))
                        .order((
                            clipboard_entry_delivery::updated_at_ms.desc(),
                            clipboard_entry::entry_id.asc(),
                        ))
                        .select(clipboard_entry::entry_id)
                        .limit(limit)
                        .load::<String>(conn)?)
                })
                .await?;
            result.extend(
                entries
                    .into_iter()
                    .map(|entry| (peer, EntryId::from(entry))),
            );
        }
        Ok(result)
    }
    async fn read_owned(
        &self,
        local: DeviceId,
        peer: DeviceId,
        entry: &EntryId,
    ) -> Result<Option<ProgressHistoryEvidence>, ProgressHistoryReadError> {
        let local_text = local.to_string();
        let peer_text = peer.to_string();
        let entry_text = entry.to_string();
        let raw = self
            .read_history(move |conn| {
                conn.transaction::<_, anyhow::Error, _>(|conn| {
                    let event = clipboard_entry_delivery::table
                        .inner_join(clipboard_entry::table.inner_join(clipboard_event::table))
                        .filter(clipboard_entry_delivery::entry_id.eq(&entry_text))
                        .filter(clipboard_entry_delivery::target_device_id.eq(peer_text))
                        .filter(clipboard_event::source_device.eq(local_text))
                        .filter(clipboard_entry::deleted_at_ms.is_null())
                        .filter(clipboard_entry::delivery_tracked.eq(true))
                        .filter(clipboard_entry::content_category.eq_any(["file", "image"]))
                        .select(clipboard_event::event_id)
                        .first::<String>(conn)
                        .optional()?;
                    let Some(event) = event else { return Ok(None) };
                    let rows = entry_file_set::table
                        .filter(entry_file_set::entry_id.eq(&entry_text))
                        .order(entry_file_set::line_index.asc())
                        .load::<EntryFileSetRow>(conn)?;
                    let representations = clipboard_snapshot_representation::table
                        .filter(clipboard_snapshot_representation::event_id.eq(event))
                        .select((
                            clipboard_snapshot_representation::id,
                            clipboard_snapshot_representation::mime_type,
                            clipboard_snapshot_representation::size_bytes,
                        ))
                        .load::<(String, Option<String>, i64)>(conn)?;
                    let sole_paste = if let ([representation], Some(selection)) = (
                        representations.as_slice(),
                        clipboard_selection::table
                            .filter(clipboard_selection::entry_id.eq(&entry_text))
                            .first::<ClipboardSelectionRow>(conn)
                            .optional()?,
                    ) {
                        selection.paste_rep_id == representation.0
                            && selection.primary_rep_id == representation.0
                            && selection.preview_rep_id == representation.0
                            && selection
                                .secondary_rep_ids
                                .split(',')
                                .all(|id| id.is_empty() || id == representation.0)
                    } else {
                        false
                    };
                    Ok(Some((rows, representations, sole_paste)))
                })
            })
            .await?;
        let Some((rows, representations, sole_paste)) = raw else {
            return Ok(None);
        };
        // 原始行在同一只读事务取得；归还连接后只认证这份快照，不重新查询。
        let file_set = if rows.is_empty() {
            None
        } else {
            Some(EntryFileSet {
                lines: self
                    .protection
                    .decode_rows(entry, rows)
                    .await
                    .map_err(|source| ProgressHistoryReadError(source.into()))?,
            })
        };
        Ok(Some(ProgressHistoryEvidence {
            file_set,
            representations: representations
                .into_iter()
                .map(|(_, mime_type, size_bytes)| ProgressHistoryRepresentation {
                    mime_type: mime_type.map(MimeType),
                    size_bytes,
                })
                .collect(),
            sole_paste,
        }))
    }
}
