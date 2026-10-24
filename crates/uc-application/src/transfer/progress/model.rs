use std::collections::HashSet;
use std::sync::Arc;
use uc_core::file_transfer::{
    ProgressActive, ProgressIdentity, ProgressItem, ProgressItemState, ProgressTerminal,
    RegistrationDescriptor,
};

pub type ProgressTaskDescriptor = RegistrationDescriptor<ProgressIdentity>;
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ProgressBound {
    pub challenge: ProgressIdentity,
    pub binding: ProgressIdentity,
    pub descriptor: ProgressTaskDescriptor,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProgressTerminalNotice {
    pub binding: ProgressIdentity,
    pub source: ProgressIdentity,
    pub attempt: ProgressIdentity,
    pub revision: u64,
    pub result: ProgressTerminal,
}
/// 完整快照用于事件丢失后的重读；缺失项保持未知，不推断失败。
pub struct ProgressProjectionSnapshot {
    pub version: u64,
    pub bound: Option<ProgressBound>,
    pub items: Vec<(ProgressItem, Option<ProgressItemState>)>,
}

/// 当前接收负责人的不可变计划快照；仅身份、位置和单调修订，不包含内容或路径。
#[derive(Clone)]
pub struct ProgressSourceTask {
    pub descriptor: ProgressTaskDescriptor,
    pub items: Arc<[ProgressItem]>,
    /// 本运行期已领取终态通知的项；用于拒绝迟到活动值，不代表结果已送达。
    pub notification_closed: Arc<HashSet<ProgressItem>>,
}

/// 一次性通知资格的业务拒绝，不推断实际下载失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressClaimRefusal {
    NotRetained,
    Superseded,
    InvalidItem,
    AlreadyObserved,
    StaleUpdate,
    InvalidUpdate,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressSubmission {
    Unchanged,
    Retained,
    NotRetained,
    Refused(ProgressClaimRefusal),
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProgressUpdateNotice {
    pub binding: ProgressIdentity,
    pub source: ProgressIdentity,
    pub attempt: ProgressIdentity,
    pub revision: u64,
    pub update: ProgressActive,
}
