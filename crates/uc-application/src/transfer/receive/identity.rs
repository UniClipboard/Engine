//! 接收项身份只使用条目、尝试及原始位置，不以名称或内容摘要合并。
use uc_core::ids::EntryId;

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
