use super::super::{ProgressSourceTask, ProgressTaskDescriptor};
use std::sync::Arc;
use uc_core::file_transfer::{ProgressIdentity, ProgressItem, ProgressItemState};

pub struct ProgressPageRequest {
    pub challenge: ProgressIdentity,
    pub source: ProgressIdentity,
    pub attempt: Option<ProgressIdentity>,
    pub offset: u16,
    pub revision: Option<u64>,
}
pub struct ProgressSnapshotPage {
    pub descriptor: ProgressTaskDescriptor,
    pub revision: u64,
    pub offset: u16,
    pub total: u16,
    pub items: Vec<ProgressItemState>,
}
pub enum ProgressPageReply {
    Current(ProgressSnapshotPage),
    Superseded(ProgressSnapshotPage),
    NotRetained,
    Unsupported,
    AccessDenied,
    Busy,
}
/// 只有完整分页核验通过才能从读取动作得到此快照；不代表每项实际下载成功。
#[derive(Clone)]
pub struct ProgressSnapshot {
    pub descriptor: ProgressTaskDescriptor,
    pub revision: u64,
    pub items: Vec<ProgressItemState>,
}
pub enum ProgressSnapshotRead {
    Complete(ProgressSnapshot),
    Superseded(ProgressTaskDescriptor),
    NotRetained,
    Unsupported,
    AccessDenied,
    Busy,
}

/// 入站查询只关联已认证设备与源条目，不接受任意本机条目枚举。
#[derive(Clone, Copy)]
pub struct ProgressSnapshotQuery {
    pub source: ProgressIdentity,
    pub attempt: Option<ProgressIdentity>,
    pub offset: u16,
    pub revision: Option<u64>,
}

/// 原意图完整计划约束查询范围；当前描述尚未确认时不能伪造零值描述。
#[derive(Clone)]
pub struct ProgressSnapshotTarget {
    pub source: ProgressIdentity,
    pub expected: Option<ProgressTaskDescriptor>,
    pub items: Arc<[ProgressItem]>,
}
impl From<ProgressSourceTask> for ProgressSnapshotTarget {
    fn from(task: ProgressSourceTask) -> Self {
        Self {
            source: task.descriptor.source,
            expected: Some(task.descriptor),
            items: task.items,
        }
    }
}
