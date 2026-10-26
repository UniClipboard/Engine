//! 同进程重试的原始资料与实际保存事实；不写入持久存储，也不输出敏感字段。
use super::ReceiveProgressSignal;
use super::{ReceiveAction, ReceiveBatch, ReceiveWorkRegistry};
use crate::clipboard::sync::payload_codec::V3BlobRef;
use crate::transfer::receive::batch::ReceiveFacts;
use crate::transfer::receive::identity::receive_item_ids;
use crate::transfer::receive::materialization::{
    receive_member_totals, InboundFileSetManifest, ReceiveSelection, ReceivedItem,
};
use anyhow::Error as SourceError;
use bytes::Bytes;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::atomic::Ordering;
use std::sync::{Arc, PoisonError, Weak};
use uc_core::ids::{DeviceId, EntryId, FormatId, RepresentationId};
use uc_core::ports::{EntryReceiveProgress, ReceiveItemKind, TrackedFileTransferStatus};
use uc_core::MimeType;

mod progress_snapshot;
mod projection;

/// 原包只在尚未保存条目时需要；保存后从既有密文快照重建正文。
#[derive(Clone)]
pub(crate) struct ReceiveSource {
    pub(crate) source_entry: Option<EntryId>,
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

#[derive(Clone)]
pub(crate) struct ReceiveRepresentation {
    pub(crate) id: RepresentationId,
    pub(crate) format_id: FormatId,
    pub(crate) mime: Option<MimeType>,
}

pub(crate) struct PlannedReceiveItem {
    pub(crate) id: String,
    pub(crate) kind: ReceiveItemKind,
    pub(crate) blob_index: usize,
    pub(super) members: usize,
    pub(in crate::transfer::receive) bytes: u64,
}

pub(super) struct ReceiveReceipt {
    attempt: String,
    source: Option<ReceiveSource>,
    planned: Arc<[PlannedReceiveItem]>,
    batch: Weak<ReceiveBatch>,
    final_facts: Option<ReceiveFacts>,
    landed: HashMap<usize, ReceivedItem>,
}

impl ReceiveSource {
    pub(crate) fn planned(&self, entry: &EntryId, attempt: &str) -> Vec<PlannedReceiveItem> {
        if self.manifest.is_some() {
            let (members, bytes) = receive_member_totals(&self.blob_refs, self.manifest.as_ref());
            return vec![PlannedReceiveItem {
                id: "directory_unit".into(),
                kind: ReceiveItemKind::DirectoryUnit,
                blob_index: 0,
                members,
                bytes,
            }];
        }
        self.blob_refs
            .iter()
            .zip(receive_item_ids(entry, attempt, &self.blob_refs))
            .enumerate()
            .map(|(blob_index, (blob, id))| {
                let kind = if blob.representation_index.is_some() {
                    ReceiveItemKind::Representation
                } else {
                    ReceiveItemKind::File
                };
                PlannedReceiveItem {
                    id,
                    kind,
                    blob_index,
                    members: 1,
                    bytes: blob.size_bytes,
                }
            })
            .collect()
    }
}

impl ReceiveAction {
    /// 在持久 attempt 可见前准备不可变计划；此时不替换旧尝试的来源或交付证明。
    pub(crate) fn prepare_plan(&self, source: &ReceiveSource) {
        let progress = self
            .progress
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(progress) = progress {
            *progress
                .prepared_plan
                .lock()
                .unwrap_or_else(PoisonError::into_inner) =
                Some(source.planned(&progress.entry, &progress.attempt).into());
        }
    }

    pub(crate) fn inherit_results(&self, selection: &ReceiveSelection) {
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
            receipt.landed = selection.landed.clone();
            for item in receipt.planned.iter() {
                if let Some(previous) = selection.unselected.get(&item.blob_index) {
                    self.batch.inherit_result(&item.id, previous);
                }
            }
        }
    }

    /// 仅在持久attempt的CAS接受后调用；预备计划不产生当前进度权威。
    pub(crate) fn accept_source(&self, source: ReceiveSource) {
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
            || progress.completion.borrow().is_some()
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
        // 每个已接受attempt只交接一次；旧动作的重复交接不能按到达顺序重获权威。
        if progress.source_accepted.swap(true, Ordering::AcqRel) {
            return;
        }
        let planned = progress
            .prepared_plan
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert_with(|| source.planned(&progress.entry, &progress.attempt).into())
            .clone();
        if let Some(changed) = state.accept_progress(&progress.entry, &progress.attempt, &source) {
            self.batch
                .observe_progress(ReceiveProgressSignal::new(&self.registry, changed));
        }
        let expected_hash = source.expected_hash.clone();
        let previous = state.receipts.insert(
            progress.entry.clone(),
            ReceiveReceipt {
                attempt: progress.attempt.clone(),
                source: Some(source),
                planned,
                batch: Arc::downgrade(&self.batch),
                final_facts: None,
                landed: HashMap::new(),
            },
        );
        let mut saved = self
            .previous_receipt
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if saved.is_none() {
            *saved = previous.filter(|receipt| {
                receipt.attempt != progress.attempt
                    && !receipt.landed.is_empty()
                    && receipt.final_facts.is_some()
                    && receipt.source.as_ref().is_some_and(|original| {
                        original.manifest.is_none() && original.expected_hash == expected_hash
                    })
            });
        }
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
        let previous = self
            .previous_receipt
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(mut previous) = previous {
            if state.accepting
                && !self.generation.is_cancelled()
                && !state.cleaning.contains(progress.entry.as_ref())
                && state
                    .receipts
                    .get(&progress.entry)
                    .is_some_and(|receipt| receipt.attempt == progress.attempt)
            {
                if let Some(source) = &previous.source {
                    let planned: Arc<[_]> =
                        source.planned(&progress.entry, &progress.attempt).into();
                    if let Some(facts) = &mut previous.final_facts {
                        facts.rebase(&previous.planned, &planned);
                    }
                    previous.planned = planned;
                    previous.attempt = progress.attempt.clone();
                    previous.batch = Weak::new();
                    state.receipts.insert(progress.entry.clone(), previous);
                    return;
                }
            }
        }
        if let Some(receipt) = state
            .receipts
            .get_mut(&progress.entry)
            .filter(|receipt| receipt.attempt == progress.attempt)
        {
            if receipt
                .batch
                .upgrade()
                .is_some_and(|batch| Arc::ptr_eq(&batch, &self.batch))
            {
                receipt.final_facts = Some(self.batch.facts());
            }
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
        // 唯一capture提交已经成功，旧来源不再代表当前保存事实。
        self.previous_receipt
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let Some(receipt) = state
            .receipts
            .get_mut(&progress.entry)
            .filter(|receipt| receipt.attempt == progress.attempt)
        else {
            return;
        };
        if complete {
            progress.fully_persisted.store(true, Ordering::Release);
        }
        if complete && receipt.landed.is_empty() {
            state.receipts.remove(&progress.entry);
            drop(state);
            self.batch.progress_changed();
            return;
        }
        if complete {
            // 继承成功项没有本次下载行，保留纯结果投影，立即释放全部重试来源。
            receipt.source = None;
        }
        receipt.landed = completed
            .iter()
            .map(|item| (item.blob_index, item.clone()))
            .collect();
        if let Some(source) = &mut receipt.source {
            source.original_wire = None;
        }
        drop(state);
        self.batch.progress_changed();
    }
}

impl ReceiveWorkRegistry {
    pub(crate) fn retry_material(
        &self,
        entry: &EntryId,
        attempt: &str,
    ) -> Option<(ReceiveSource, HashMap<usize, ReceivedItem>)> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if !state.accepting || state.cleaning.contains(entry.as_ref()) {
            return None;
        }
        let receipt = state
            .receipts
            .get(entry)
            .filter(|row| row.attempt == attempt)?;
        Some((receipt.source.as_ref()?.clone(), receipt.landed.clone()))
    }

    pub(crate) fn merge_progress(
        &self,
        progress: &mut EntryReceiveProgress,
    ) -> Result<(), SourceError> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(receipt) = state
            .receipts
            .get(&EntryId::from(progress.entry_id.clone()))
            .filter(|receipt| receipt.attempt == progress.attempt_id)
        else {
            // 创建端口可能已经提交，但完整来源尚未交接；只使用同一原动作的已知计划和事实。
            let key = (
                EntryId::from(progress.entry_id.clone()),
                progress.attempt_id.clone(),
            );
            if let Some(active) = state.active.get(&key).and_then(Weak::upgrade) {
                let planned = active
                    .prepared_plan
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone();
                if let (Some(planned), Some(batch)) = (planned, active.batch.upgrade()) {
                    let committed = planned
                        .iter()
                        .filter(|item| {
                            progress.items.iter().any(|saved| {
                                saved.item_id == item.id
                                    && saved.status == TrackedFileTransferStatus::Completed
                            })
                        })
                        .map(|item| item.blob_index)
                        .collect();
                    progress.items = batch.items(&planned, &committed, progress.state);
                    return projection::project_totals(&planned, progress);
                }
            }
            return Ok(());
        };
        let committed = receipt.landed.keys().copied().collect::<HashSet<_>>();
        if let Some(batch) = receipt.batch.upgrade() {
            progress.items = batch.items(&receipt.planned, &committed, progress.state);
        } else if let Some(facts) = &receipt.final_facts {
            progress.items = facts.items(&receipt.planned, &committed, progress.state);
        }
        projection::project_totals(&receipt.planned, progress)
    }
}

#[cfg(test)]
mod tests;
