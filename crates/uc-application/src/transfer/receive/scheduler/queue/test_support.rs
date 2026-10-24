//! 显式资源验收读取原调度状态；不输出peer材料，不提供业务或Engine查询接口。
use super::State;
use std::fs::{create_dir_all, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use uc_observability_contract::wire_enum;

static NEXT_EVIDENCE_ID: AtomicU64 = AtomicU64::new(1);

pub(super) struct ResourceEvidenceId(u64);

impl Default for ResourceEvidenceId {
    fn default() -> Self {
        Self(NEXT_EVIDENCE_ID.fetch_add(1, Ordering::Relaxed))
    }
}

wire_enum! {
    #[derive(Clone, Copy)]
    pub(super) enum SnapshotEvent: &'static str {
        Created => "created",
        Reserved => "reserved",
        Returned => "returned",
        Closed => "closed",
        Settled => "settled",
    }
}

pub(super) fn record_snapshot(state: &State, event: SnapshotEvent) {
    let Some(directory) = std::env::var_os("UC_RECEIVE_RESOURCE_EVIDENCE_DIR") else {
        return;
    };
    let directory = PathBuf::from(directory);
    create_dir_all(&directory).expect("创建测试额度证据目录");
    let path = directory.join(format!(
        "download-scheduler-{}-{}.jsonl",
        std::process::id(),
        state.resource_evidence.0
    ));
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("打开测试额度证据");
    let record = serde_json::json!({
        "event": event.wire(),
        "active": state.running.values().sum::<usize>(),
        "queued": state.pending.values().map(|queue| queue.len()).sum::<usize>(),
        "slots": state.slots.len(),
        "peer_active_max": state.running.values().copied().max().unwrap_or(0),
        "peer_queued_max": state.pending.values().map(|queue| queue.len()).max().unwrap_or(0),
        "closed": state.closed,
    });
    serde_json::to_writer(&mut file, &record).expect("编码测试额度计数");
    file.write_all(b"\n").expect("保存测试额度计数");
}
