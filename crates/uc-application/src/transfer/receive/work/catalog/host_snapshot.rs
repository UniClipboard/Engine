//! 宿主查询复用原目录快照，不从持久化传输编号反推内容位置。
use super::super::RegistryState;
use super::projection::project_totals;
use crate::transfer::receive::identity::{
    directory_member_transfer_id, directory_representation_transfer_id,
};
use anyhow::{Context, Error as SourceError};
use std::sync::{PoisonError, Weak};
use uc_core::file_transfer::{
    ProgressFailureReason, ProgressIdentity, ProgressItemKind, ProgressItemState, ProgressPhase,
    ProgressTerminalOutcome,
};
use uc_core::ids::EntryId;
use uc_core::ports::{
    EntryReceiveProgress, ReceiveContentLocator, ReceiveItemFailureReason, ReceiveItemTransfer,
    ReceiveSnapshotVersion, TrackedFileTransferStatus,
};
use uuid::Uuid;

impl RegistryState {
    pub(super) fn project_host_snapshot(
        &mut self,
        progress: &mut EntryReceiveProgress,
    ) -> Result<(), SourceError> {
        if !self.accepting || self.clearing || self.clear_failed {
            return Ok(());
        }
        let entry = EntryId::from(progress.entry_id.clone());
        let Ok(attempt) = Uuid::parse_str(&progress.attempt_id) else {
            return Ok(());
        };
        let Some((peer, current)) = self
            .progress_catalog
            .local_scope(&entry, ProgressIdentity::new(*attempt.as_bytes()))
        else {
            return Ok(());
        };
        let Some(items) = self.progress_states(&entry, &current).or_else(|| {
            self.progress_catalog
                .retained_states(&peer, current.descriptor.source)
        }) else {
            return Ok(());
        };
        let snapshot = self
            .progress_catalog
            .publish_snapshot(&peer, current.descriptor, items)
            .context("read receive host snapshot")?;
        for state in &snapshot.items {
            let logical = state.item();
            let (id, content) = match logical.kind {
                ProgressItemKind::File => (
                    directory_member_transfer_id(
                        &entry,
                        Some(&progress.attempt_id),
                        usize::from(logical.index),
                    ),
                    ReceiveContentLocator::File {
                        index: u32::from(logical.index),
                    },
                ),
                ProgressItemKind::Representation => (
                    directory_representation_transfer_id(
                        &entry,
                        Some(&progress.attempt_id),
                        usize::from(logical.index),
                    ),
                    ReceiveContentLocator::Representation {
                        index: u32::from(logical.index),
                    },
                ),
                ProgressItemKind::DirectoryUnit => (
                    "directory_unit".into(),
                    ReceiveContentLocator::DirectoryUnit,
                ),
            };
            let Some(item) = progress.items.iter_mut().find(|item| item.item_id == id) else {
                continue;
            };
            // 状态和字节采用同一次原子事实读取，避免保存围栏在两次读取之间推进。
            match state {
                ProgressItemState::Active(value) => {
                    // 原宿主结果保留已知失败／取消；反向协议仍可等待实际清理后再发终态。
                    if !matches!(
                        item.status,
                        TrackedFileTransferStatus::Failed | TrackedFileTransferStatus::Cancelled
                    ) {
                        item.status = if value.phase == ProgressPhase::Waiting {
                            TrackedFileTransferStatus::Pending
                        } else {
                            TrackedFileTransferStatus::Transferring
                        };
                    }
                }
                ProgressItemState::Terminal(value) => {
                    let (status, failure_reason) = match value.outcome {
                        ProgressTerminalOutcome::Completed => {
                            (TrackedFileTransferStatus::Completed, None)
                        }
                        ProgressTerminalOutcome::Cancelled(_) => {
                            (TrackedFileTransferStatus::Cancelled, None)
                        }
                        ProgressTerminalOutcome::Failed(reason) => (
                            TrackedFileTransferStatus::Failed,
                            Some(receive_failure(reason)),
                        ),
                    };
                    item.status = status;
                    item.failure_reason = failure_reason;
                }
            }
            let (received_bytes, total_bytes, round) = match state {
                ProgressItemState::Active(value) => (value.bytes, value.total, value.round),
                ProgressItemState::Terminal(value) => (value.bytes, value.total, value.round),
            };
            item.content = Some(content);
            item.transfer = Some(ReceiveItemTransfer {
                received_bytes,
                total_bytes,
                round,
            });
        }
        // URI列表按原始位置排列成功项，再排列缺失项；不使用名称或摘要配对。
        let mut files = progress
            .items
            .iter()
            .enumerate()
            .filter_map(|(position, item)| {
                if let Some(ReceiveContentLocator::File { index }) = item.content {
                    Some((
                        item.status != TrackedFileTransferStatus::Completed,
                        index,
                        position,
                    ))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        files.sort_unstable();
        if files.iter().any(|(missing, _, _)| !missing) {
            for (history_index, (_, _, position)) in files.into_iter().enumerate() {
                progress.items[position].history_file_index = Some(history_index as u32);
            }
        }
        if let Some(receipt) = self
            .receipts
            .get(&entry)
            .filter(|receipt| receipt.attempt == progress.attempt_id)
        {
            project_totals(&receipt.planned, progress)?;
        } else if let Some(active) = self
            .active
            .get(&(entry, progress.attempt_id.clone()))
            .and_then(Weak::upgrade)
        {
            if let Some(planned) = active
                .prepared_plan
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_ref()
            {
                project_totals(planned, progress)?;
            }
        }
        progress.snapshot_version = Some(ReceiveSnapshotVersion {
            producer_id: Uuid::from_bytes(snapshot.descriptor.producer.bytes()).to_string(),
            attempt_revision: snapshot.descriptor.revision,
            snapshot_revision: snapshot.revision,
        });
        Ok(())
    }
}

fn receive_failure(reason: ProgressFailureReason) -> ReceiveItemFailureReason {
    match reason {
        ProgressFailureReason::NetworkUnavailable => ReceiveItemFailureReason::NetworkUnavailable,
        ProgressFailureReason::TimedOut => ReceiveItemFailureReason::TimedOut,
        ProgressFailureReason::AccessDenied => ReceiveItemFailureReason::AccessDenied,
        ProgressFailureReason::StorageUnavailable => ReceiveItemFailureReason::StorageUnavailable,
        ProgressFailureReason::IntegrityCheckFailed => {
            ReceiveItemFailureReason::IntegrityCheckFailed
        }
        ProgressFailureReason::Interrupted => ReceiveItemFailureReason::Interrupted,
        ProgressFailureReason::Unknown => ReceiveItemFailureReason::Unknown,
    }
}
