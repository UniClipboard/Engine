//! 接收项身份只使用条目、尝试及原始位置，不以名称或内容摘要合并。
use crate::clipboard::sync::payload_codec::V3BlobRef;
use uc_core::ids::EntryId;

/// 与原始blob数组一一对应；文件序号独立于表示位置。
pub(crate) fn receive_item_ids(entry: &EntryId, attempt: &str, blobs: &[V3BlobRef]) -> Vec<String> {
    let mut file_index = 0;
    blobs
        .iter()
        .map(|blob| {
            if let Some(index) = blob.representation_index {
                directory_representation_transfer_id(entry, Some(attempt), index as usize)
            } else {
                let id = directory_member_transfer_id(entry, Some(attempt), file_index);
                file_index += 1;
                id
            }
        })
        .collect()
}

pub(crate) fn directory_member_transfer_id(
    receiver_entry_id: &EntryId,
    attempt_id: Option<&str>,
    member_index: usize,
) -> String {
    match attempt_id {
        Some(attempt_id) => format!(
            "{}:attempt:{attempt_id}:member:{member_index}",
            receiver_entry_id.as_ref()
        ),
        None => format!("{}:member:{member_index}", receiver_entry_id.as_ref()),
    }
}

pub(crate) fn directory_representation_transfer_id(
    entry_id: &EntryId,
    attempt_id: Option<&str>,
    representation_index: usize,
) -> String {
    match attempt_id {
        Some(attempt) => format!(
            "{}:attempt:{attempt}:representation:{representation_index}",
            entry_id.as_ref()
        ),
        None => format!(
            "{}:representation:{representation_index}",
            entry_id.as_ref()
        ),
    }
}
