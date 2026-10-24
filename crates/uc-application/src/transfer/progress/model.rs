use std::sync::Arc;
use uc_core::file_transfer::{
    ProgressIdentity, ProgressItem, ProgressTerminal, RegistrationDescriptor,
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
/// 完整快照用于事件丢失后的重读；没有终态的项保持等待，不推断失败。
pub struct ProgressProjectionSnapshot {
    pub version: u64,
    pub bound: Option<ProgressBound>,
    pub items: Vec<(ProgressItem, Option<ProgressTerminal>)>,
}

/// 当前接收负责人的不可变计划快照；仅身份、位置和单调修订，不包含内容或路径。
#[derive(Clone)]
pub struct ProgressSourceTask {
    pub descriptor: ProgressTaskDescriptor,
    pub items: Arc<[ProgressItem]>,
}
