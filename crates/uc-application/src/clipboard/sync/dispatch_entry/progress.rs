//! 原发送动作在第一条网络消息之前登记完整意图；通知拒绝不改变原实际投递结果。
use super::DispatchClipboardEntryInput;
use crate::clipboard::sync::payload_codec::decode_v3_bytes_to_snapshot_blob_refs_and_file_set;
use crate::transfer::progress::{ProgressIntent, ProgressOriginalPlan, ProgressProjectionKey};
use uc_core::error_class::ErrorClass;
use uc_core::ids::DeviceId;
use uc_observability_contract::uc_debug;

pub(super) fn original_plan(input: &DispatchClipboardEntryInput) -> Option<ProgressOriginalPlan> {
    if input.payload_version != 3 {
        return None;
    }
    let (_, blobs, manifest) =
        match decode_v3_bytes_to_snapshot_blob_refs_and_file_set(&input.plaintext) {
            Ok(decoded) => decoded,
            Err(source) => {
                // 原raw-bytes入口允许未携带V3结构；不能让展示能力改变原发送行为。
                uc_debug!(
                    error = source.as_ref(),
                    "outbound original progress plan unavailable"
                );
                return None;
            }
        };
    // 有引用时严格取原包来源；纯空目录的本地意图仍可等待可靠关联。
    let hint = if blobs.is_empty() {
        input.entry_id.as_ref()
    } else {
        None
    };
    ProgressOriginalPlan::from_refs(hint, &blobs, manifest.is_some())
}

pub(super) fn accept_intent(intent: &ProgressIntent, peer: DeviceId, plan: &ProgressOriginalPlan) {
    if let Err(source) = intent.accept(
        ProgressProjectionKey {
            peer,
            source: plan.source,
        },
        plan.items.clone(),
    ) {
        uc_debug!(
            error_class = source.class(),
            error = &source as &dyn std::error::Error,
            "outbound original progress intent not retained"
        );
    }
}
