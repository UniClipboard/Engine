use super::{
    BeginReceiveRetryInput, PreparedReceiveRetry, ReceiveRetryItem, ReceiveRetryItemState,
};
use crate::clipboard::sync::apply_inbound::ApplyInboundError;
use crate::transfer::receive::identity::receive_item_ids;
use uc_core::ports::{ReceiveItemFailureReason, ReceiveItemRole, TrackedFileTransferStatus};

impl PreparedReceiveRetry {
    pub(super) fn ledger_plan(
        &self,
        attempt: &str,
        now_ms: i64,
    ) -> Result<BeginReceiveRetryInput, ApplyInboundError> {
        let selection = self.selection.as_ref().ok_or_else(|| {
            ApplyInboundError::Internal(anyhow::anyhow!("receive retry selection is missing"))
        })?;
        let item_ids = receive_item_ids(&self.entry, attempt, &self.source.blob_refs);
        let mut items = Vec::with_capacity(self.source.blob_refs.len());
        for (index, (blob, item_id)) in self.source.blob_refs.iter().zip(item_ids).enumerate() {
            let role = if blob.representation_index.is_some() {
                ReceiveItemRole::Representation
            } else {
                ReceiveItemRole::File
            };
            let (state, bytes) = if let Some(saved) = selection.landed.get(&index) {
                (ReceiveRetryItemState::Completed, saved.bytes)
            } else if selection.selected.contains(&index) {
                (ReceiveRetryItemState::Pending, blob.size_bytes)
            } else {
                let previous = selection.unselected.get(&index);
                let state = if previous
                    .is_some_and(|item| item.status == TrackedFileTransferStatus::Cancelled)
                {
                    ReceiveRetryItemState::Cancelled {
                        timed_out: previous.and_then(|item| item.failure_reason)
                            == Some(ReceiveItemFailureReason::TimedOut),
                    }
                } else {
                    ReceiveRetryItemState::Failed(
                        previous
                            .and_then(|item| item.failure_reason)
                            .unwrap_or(ReceiveItemFailureReason::Interrupted),
                    )
                };
                (state, blob.size_bytes)
            };
            items.push(ReceiveRetryItem {
                item_id,
                role,
                filename: blob.filename.clone().unwrap_or_default(),
                bytes,
                state,
            });
        }
        Ok(BeginReceiveRetryInput {
            entry_id: self.entry.as_ref().to_owned(),
            expected_attempt_id: self.expected_attempt.clone(),
            attempt_id: attempt.to_owned(),
            source_device_id: self.source.from_device.as_str().to_owned(),
            directory: self.source.manifest.is_some(),
            now_ms,
            items,
        })
    }
}
