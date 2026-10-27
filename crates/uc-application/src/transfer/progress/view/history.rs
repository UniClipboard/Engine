//! 历史读取不登记投影，不生成attempt，不恢复任何通知补发。
use super::{mapping::present, owner::authorize, ProgressViewOwner, TransferProgressQueryError};
use crate::clipboard::outbound::{manifest_line_path, MAX_INLINE_OUTBOUND_REPRESENTATION_BYTES};
use crate::transfer::progress::{
    ProgressHistoryEvidence, ProgressProjectionKey, ProgressProjectionSnapshot,
};
use uc_core::clipboard::EntryFileSetLineKind;
use uc_core::file_transfer::{
    summarize_progress, valid_progress_plan, ProgressFreshnessView, ProgressItem, ProgressItemKind,
    TransferProgressView,
};
use uc_core::ids::EntryId;

impl ProgressViewOwner {
    pub(super) async fn historical(
        &self,
        key: ProgressProjectionKey,
        entry: &EntryId,
    ) -> Result<Option<TransferProgressView>, TransferProgressQueryError> {
        let fence = self.projections.begin_intent(entry.clone());
        let evidence = self
            .history
            .read_owned(self.local, key.peer, entry)
            .await
            .map_err(|source| TransferProgressQueryError::Unavailable(Box::new(source)))?;
        authorize(&self.current_scope().await?, key.peer)?;
        if !fence.permits_history(key.peer) {
            return Ok(None);
        }
        // 等待存储期间新登记或终态可能已到达；实时事实始终优先。
        if let Some(snapshot) = self.projections.snapshot(key) {
            return Ok(Some(present(key, snapshot)));
        }
        let Some(items) = evidence.and_then(original_items) else {
            return Ok(None);
        };
        let freshness = ProgressFreshnessView::Unconfirmed;
        let summary = summarize_progress(items.iter().map(|_| None), freshness);
        Ok(Some(present(
            key,
            ProgressProjectionSnapshot {
                freshness: Ok(freshness),
                summary,
                version: 0,
                bound: None,
                task: None,
                items: items.into_iter().map(|item| (item, None)).collect(),
            },
        )))
    }
}
fn original_items(evidence: ProgressHistoryEvidence) -> Option<Vec<ProgressItem>> {
    let oversized = evidence
        .representations
        .iter()
        .map(|representation| {
            let size = usize::try_from(representation.size_bytes).ok()?;
            Some(
                size > MAX_INLINE_OUTBOUND_REPRESENTATION_BYTES
                    && representation.mime_type.as_ref().is_some_and(|mime| {
                        mime.as_str().to_ascii_lowercase().starts_with("image/")
                    }),
            )
        })
        .collect::<Option<Vec<_>>>()?;
    let items = if let Some(files) = evidence.file_set {
        // 文件与大图片混合时，旧图片位置未持久化；不能只展示文件子集。
        if oversized.iter().any(|value| *value) {
            return None;
        }
        if files
            .lines
            .iter()
            .any(|line| matches!(line.kind, EntryFileSetLineKind::Excluded { .. }))
        {
            return None;
        }
        if files.has_directory_structure() {
            files.file_set_v1_component()?;
            vec![ProgressItem {
                kind: ProgressItemKind::DirectoryUnit,
                index: 0,
            }]
        } else {
            let mut paths = files
                .file_lines()
                .map(|line| manifest_line_path(&line.original_text))
                .collect::<Option<Vec<_>>>()?;
            paths.sort();
            paths.dedup();
            (0..paths.len())
                .map(|index| {
                    Some(ProgressItem {
                        kind: ProgressItemKind::File,
                        index: u16::try_from(index).ok()?,
                    })
                })
                .collect::<Option<Vec<_>>>()?
        }
    } else {
        if !evidence.sole_paste || oversized.as_slice() != [true] {
            return None;
        }
        vec![ProgressItem {
            kind: ProgressItemKind::Representation,
            index: 0,
        }]
    };
    valid_progress_plan(&items).then_some(items)
}

#[cfg(test)]
mod tests;
