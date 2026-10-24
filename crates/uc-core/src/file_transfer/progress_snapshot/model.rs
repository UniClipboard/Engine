use super::super::{ProgressIdentity, ProgressItem, ProgressTerminal, RegistrationDescriptor};

pub const MAX_SNAPSHOT_PAGE_ITEMS: usize = 64;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressPhase {
    Waiting,
    Transferring,
    Processing,
    Retrying,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProgressActive {
    pub item: ProgressItem,
    pub round: u32,
    pub sequence: u64,
    pub bytes: u64,
    pub total: Option<u64>,
    pub phase: ProgressPhase,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressItemState {
    Active(ProgressActive),
    Terminal(ProgressTerminal),
}
impl ProgressItemState {
    pub fn item(self) -> ProgressItem {
        match self {
            Self::Active(value) => value.item,
            Self::Terminal(value) => value.item,
        }
    }
}
pub enum ProgressSnapshotInput {
    Page {
        descriptor: RegistrationDescriptor<ProgressIdentity>,
        revision: u64,
        offset: u16,
        total: u16,
        items: Vec<ProgressItem>,
    },
    End,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressSnapshotEffect {
    /// 先暂存本页再请求下一页；不得发布部分结果或更新完整快照。
    Accepted { next_offset: u16, revision: u64 },
    /// 暂存最后一页后才允许一次采用完整快照，完成不等于所有项业务成功。
    Complete { revision: u64 },
    /// 丢弃本次暂存结果，不修改已有投影；该覆盖会话永久封口。
    Refused(ProgressSnapshotRefusal),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ProgressSnapshotRefusal {
    #[error("snapshot original plan invalid")]
    InvalidPlan,
    #[error("snapshot descriptor changed")]
    DescriptorChanged,
    #[error("snapshot revision changed")]
    RevisionChanged,
    #[error("snapshot page offset invalid")]
    WrongOffset,
    #[error("snapshot total differs from original plan")]
    WrongTotal,
    #[error("snapshot page size invalid")]
    PageSize,
    #[error("snapshot item differs from original position")]
    WrongItem,
    #[error("snapshot page coverage incomplete")]
    Incomplete,
    #[error("snapshot coverage already sealed")]
    Sealed,
}
