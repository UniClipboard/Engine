//! 原接收负责人交接已接受的源身份与原始位置；不包含provider或文件路径。
use super::{ReceiveSource, ReceiveWorkRegistry, RegistryState};
use crate::transfer::progress::{ProgressSourceTask, ProgressTaskAuthorityPort};
use std::sync::PoisonError;
use uc_core::file_transfer::{
    ProgressIdentity, ProgressItem, ProgressItemKind, MAX_PROGRESS_ITEMS,
};
use uc_core::ids::{DeviceId, EntryId};
use uuid::Uuid;

impl RegistryState {
    pub(super) fn accept_progress(
        &mut self,
        entry: &EntryId,
        attempt: &str,
        source: &ReceiveSource,
    ) {
        let Some((source_id, attempt, items)) = source.progress_identity(attempt) else {
            // 新attempt无法可靠关联时退休旧映射；下载及本地结算继续由原负责人执行。
            self.progress_catalog.forget(entry);
            return;
        };
        self.progress_catalog
            .accept(entry, source.from_device, source_id, attempt, items);
    }
}
impl ReceiveSource {
    fn progress_identity(
        &self,
        attempt: &str,
    ) -> Option<(ProgressIdentity, ProgressIdentity, Vec<ProgressItem>)> {
        if self.blob_refs.len() > MAX_PROGRESS_ITEMS {
            return None;
        }
        let source = match &self.source_entry {
            Some(hint) => hint,
            None => {
                let first = &self.blob_refs.first()?.entry_id;
                if self.blob_refs.iter().any(|blob| &blob.entry_id != first) {
                    return None;
                }
                first
            }
        };
        // 无法表达的旧标识是关联拒绝，不是底层存储／网络错误。
        let Ok(source) = Uuid::parse_str(source.as_ref()) else {
            return None;
        };
        let Ok(attempt) = Uuid::parse_str(attempt) else {
            return None;
        };
        let items = if self.manifest.is_some() {
            vec![ProgressItem {
                kind: ProgressItemKind::DirectoryUnit,
                index: 0,
            }]
        } else {
            let mut file_index = 0_u16;
            let mut items = Vec::with_capacity(self.blob_refs.len());
            for blob in &self.blob_refs {
                let (kind, index) = match blob.representation_index {
                    Some(index) => {
                        let Ok(index) = u16::try_from(index) else {
                            return None;
                        };
                        (ProgressItemKind::Representation, index)
                    }
                    None => {
                        let index = file_index;
                        file_index += 1;
                        (ProgressItemKind::File, index)
                    }
                };
                items.push(ProgressItem { kind, index });
            }
            items
        };
        Some((
            ProgressIdentity::new(*source.as_bytes()),
            ProgressIdentity::new(*attempt.as_bytes()),
            items,
        ))
    }
}
impl ProgressTaskAuthorityPort for ReceiveWorkRegistry {
    fn current_task(
        &self,
        peer: &DeviceId,
        source: ProgressIdentity,
    ) -> Option<ProgressSourceTask> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if !state.accepting || state.clearing || state.clear_failed {
            return None;
        }
        state.progress_catalog.current(peer, source)
    }
}
