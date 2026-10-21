//! 同进程重试的原始资料与实际保存事实；不写入持久存储，也不输出敏感字段。
use super::{ReceiveAction, ReceiveBatch, ReceiveWorkRegistry};
use crate::clipboard::sync::payload_codec::V3BlobRef;
use crate::transfer::receive::batch::ReceiveFacts;
use crate::transfer::receive::identity::{
    directory_member_transfer_id, directory_representation_transfer_id,
};
use crate::transfer::receive::materialization::{InboundFileSetManifest, ReceivedItem};
use bytes::Bytes;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::{Arc, PoisonError, Weak};
use uc_core::ids::{DeviceId, EntryId, FormatId, RepresentationId};
use uc_core::ports::{EntryReceiveProgress, ReceiveItemKind};
use uc_core::MimeType;

/// 原包只在尚未保存条目时需要；保存后从既有密文快照重建正文。
pub(crate) struct ReceiveSource {
    pub(crate) from_device: DeviceId,
    pub(crate) expected_hash: String,
    pub(crate) blob_refs: Vec<V3BlobRef>,
    pub(crate) manifest: Option<InboundFileSetManifest>,
    pub(crate) original_wire: Option<Bytes>,
    pub(crate) representations: Vec<ReceiveRepresentation>,
}
impl fmt::Debug for ReceiveSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ReceiveSource")
    }
}

pub(crate) struct ReceiveRepresentation {
    pub(crate) id: RepresentationId,
    pub(crate) format_id: FormatId,
    pub(crate) mime: Option<MimeType>,
}

pub(crate) struct PlannedReceiveItem {
    pub(crate) id: String,
    pub(crate) kind: ReceiveItemKind,
    pub(crate) blob_index: usize,
}

pub(super) struct ReceiveReceipt {
    attempt: String,
    source: ReceiveSource,
    planned: Vec<PlannedReceiveItem>,
    batch: Weak<ReceiveBatch>,
    final_facts: Option<ReceiveFacts>,
    landed: HashMap<usize, ReceivedItem>,
}

impl ReceiveSource {
    fn planned(&self, entry: &EntryId, attempt: &str) -> Vec<PlannedReceiveItem> {
        if self.manifest.is_some() {
            return vec![PlannedReceiveItem {
                id: "directory_unit".into(),
                kind: ReceiveItemKind::DirectoryUnit,
                blob_index: 0,
            }];
        }
        let mut file_index = 0;
        self.blob_refs
            .iter()
            .enumerate()
            .map(|(blob_index, blob)| {
                let (id, kind) = match blob.representation_index {
                    Some(index) => (
                        directory_representation_transfer_id(entry, Some(attempt), index as usize),
                        ReceiveItemKind::Representation,
                    ),
                    None => {
                        let id = directory_member_transfer_id(entry, Some(attempt), file_index);
                        file_index += 1;
                        (id, ReceiveItemKind::File)
                    }
                };
                PlannedReceiveItem {
                    id,
                    kind,
                    blob_index,
                }
            })
            .collect()
    }
}

impl ReceiveAction {
    pub(crate) fn retain_source(&self, source: ReceiveSource) {
        let progress = self
            .progress
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let Some(progress) = progress else {
            return;
        };
        let mut state = self
            .registry
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if !state.accepting
            || self.generation.is_cancelled()
            || state.cleaning.contains(progress.entry.as_ref())
            || state.retired.get(&progress.entry).is_some_and(|births| {
                births
                    .iter()
                    .filter_map(Weak::upgrade)
                    .any(|birth| Arc::ptr_eq(&birth, &self.birth))
            })
        {
            return;
        }
        let planned = source.planned(&progress.entry, &progress.attempt);
        state.receipts.insert(
            progress.entry.clone(),
            ReceiveReceipt {
                attempt: progress.attempt.clone(),
                source,
                planned,
                batch: Arc::downgrade(&self.batch),
                final_facts: None,
                landed: HashMap::new(),
            },
        );
    }

    pub(super) fn finish_receipt(&self) {
        let progress = self
            .progress
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let Some(progress) = progress else {
            return;
        };
        let mut state = self
            .registry
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(receipt) = state
            .receipts
            .get_mut(&progress.entry)
            .filter(|receipt| receipt.attempt == progress.attempt)
        {
            receipt.final_facts = Some(self.batch.facts());
        }
    }

    /// 只有持久保存成功才释放原包；全部成功后不再保留重试能力。
    pub(crate) fn persisted(&self, complete: bool, completed: &[ReceivedItem]) {
        let progress = self
            .progress
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let Some(progress) = progress else {
            return;
        };
        let mut state = self
            .registry
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let Some(receipt) = state
            .receipts
            .get_mut(&progress.entry)
            .filter(|receipt| receipt.attempt == progress.attempt)
        else {
            return;
        };
        if complete {
            state.receipts.remove(&progress.entry);
            return;
        }
        receipt.landed = completed
            .iter()
            .map(|item| (item.blob_index, item.clone()))
            .collect();
        receipt.source.original_wire = None;
    }
}

impl ReceiveWorkRegistry {
    pub(crate) fn merge_progress(&self, progress: &mut EntryReceiveProgress) {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(receipt) = state
            .receipts
            .get(&EntryId::from(progress.entry_id.clone()))
            .filter(|receipt| receipt.attempt == progress.attempt_id)
        else {
            return;
        };
        let committed = receipt.landed.keys().copied().collect::<HashSet<_>>();
        if let Some(batch) = receipt.batch.upgrade() {
            progress.items = batch.items(&receipt.planned, &committed, progress.state);
        } else if let Some(facts) = &receipt.final_facts {
            progress.items = facts.items(&receipt.planned, &committed, progress.state);
        }
    }
}

#[cfg(test)]
mod tests;
