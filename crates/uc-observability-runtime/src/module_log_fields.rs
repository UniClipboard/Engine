//! 模块日志的文本字段判定：只有登记在字段目录（`uc_observability_contract::log_fields`）里且类别为文本的字段名，
//! 其取值才原样落盘；其余文本字段一律记为 `<omitted>`。数字与布尔字段不受此限。
//!
//! 通过 `uc_*!` 宏写入的记录在编译期已被目录约束，这里保留的运行期判定只是最后一道防线，
//! 针对绕过宏的调用（观测 crate 自己的测试与第三方 crate 的事件）。

use uc_observability_contract::log_fields::text_field_in_catalog;

pub(crate) fn text_field_allowed(name: &str) -> bool {
    text_field_in_catalog(name)
}

#[cfg(test)]
mod tests {
    use super::text_field_allowed;

    /// 引入字段目录之前允许落盘的文本字段（去掉带点号的 `error.type`，它无法作为目录标识符）。
    const PRE_CATALOG_ALLOWED: &[&str] = &[
        "ack_kind",
        "alpn",
        "attempt_id",
        "blob_id",
        "category",
        "congestion_controller",
        "current",
        "current_version",
        "doc_table",
        "entry_id",
        "entry_id_str",
        "error_category",
        "error_code",
        "error_kind",
        "error_stage",
        "error_type",
        "evaluation",
        "event_id",
        "event_kind",
        "existing_entry_id",
        "expected",
        "failure",
        "failure_reason",
        "failure_stage",
        "filter_kind",
        "format_id",
        "format_ids",
        "intent",
        "io_error_kind",
        "key_class",
        "kind",
        "mime",
        "mimes",
        "mode",
        "next_phase",
        "op",
        "operation",
        "origin",
        "original_mime",
        "outcome",
        "packed_rep_ids",
        "paste_rep_id",
        "payload_state",
        "phase",
        "plain_rep_id",
        "posting_table",
        "preview_rep_id",
        "previous_phase",
        "reason",
        "rep_id",
        "reply_kind",
        "representation_id",
        "result",
        "rules",
        "scope",
        "source",
        "stage",
        "state",
        "step",
        "stored_version",
        "strategy",
        "table",
        "transfer_id",
        "uc_congestion_controller",
        "upgrade_action",
        "upgrade_phase",
        "variant",
        "version",
    ];

    /// 迁移及其后补日志时新增的可见文本字段：`Literal` 字段只接受 `&'static str`（编译期保证是固定字面量），
    /// `existing_status` 是文件传输状态的已审定词表。
    const NEWLY_VISIBLE: &[&str] = &[
        "cause",
        "context",
        "dependency",
        "emitter",
        "event",
        "existing_status",
        "file_paths_source",
        "issue",
        "msg_kind",
        "plan",
        "reject_reason",
        "rollback_target",
        "storage_generation",
        "task",
        "trigger",
        "worker",
    ];

    #[test]
    fn text_field_allowlist_changes_only_by_the_approved_migration_set() {
        let mut current: Vec<&str> = uc_observability_contract::log_fields::CATALOG
            .iter()
            .filter(|field| field.class.is_text())
            .map(|field| field.name)
            .collect();
        let mut expected: Vec<&str> = PRE_CATALOG_ALLOWED
            .iter()
            .chain(NEWLY_VISIBLE)
            .copied()
            .collect();
        current.sort_unstable();
        expected.sort_unstable();
        assert_eq!(current, expected);
        for name in PRE_CATALOG_ALLOWED {
            assert!(text_field_allowed(name), "{name} must stay allowed");
        }
    }

    #[test]
    fn fields_outside_the_catalog_stay_omitted() {
        for name in ["path", "peer", "device_name", "snapshot_hash", "error.type"] {
            assert!(!text_field_allowed(name), "{name} must stay omitted");
        }
    }
}
