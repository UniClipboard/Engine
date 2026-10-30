use uc_core::file_transfer::ProgressIdentity;
use uc_core::ids::DeviceId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProgressProjectionKey {
    pub peer: DeviceId,
    pub source: ProgressIdentity,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressProjectionAdmission {
    Retained,
    Unchanged,
    Closed,
}
/// 有界变化提示；丢失时重读完整快照，不把提示当作业务结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressProjectionEvent {
    Changed(ProgressProjectionKey),
    Removed(ProgressProjectionKey),
}
