//! 验收证据只保留稳定状态与计数，不导出条目、设备或尝试身份。
use serde_json::{json, Value};
use uc_engine::OutboundProgressSummary;

pub(super) fn present(progress: Option<OutboundProgressSummary>) -> Value {
    let Some(progress) = progress else {
        return json!({"ok": true, "kind": "outbound_progress", "present": false});
    };
    json!({
        "ok": true,
        "kind": "outbound_progress",
        "present": true,
        "has_attempt": progress.attempt_id.is_some(),
        "revision": progress.revision.to_string(),
        "state": progress.state,
        "freshness": progress.freshness,
        "freshness_available": progress.freshness_available,
        "items_total": progress.items_total,
        "items_waiting": progress.items_waiting,
        "items_running": progress.items_running,
        "items_completed": progress.items_completed,
        "items_failed": progress.items_failed,
        "items_cancelled": progress.items_cancelled,
        "items_unconfirmed": progress.items_unconfirmed,
    })
}
