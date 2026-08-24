//! 处理本机从其他成员收到、已经验证但尚未接受的成员移除。
//!
//! 接受时，本机应用同一条 `RemoveDevice` 事件并与发送方恢复一致。
//! 拒绝时，本机保留当前成员分支，并将双方关系标记为分叉。
//! 如果移除目标是本机，接受前必须得到明确二次确认。
//! 该用例不负责发起成员移除，也不兼容旧成员历史。
mod deps;
mod error;
mod model;
mod use_case;

#[cfg(test)]
mod tests;

pub(crate) use deps::DecidePendingMembershipRemovalDeps;
pub use error::DecidePendingMembershipRemovalError;
pub use model::DecidePendingMembershipRemovalResult;
pub(crate) use use_case::DecidePendingMembershipRemovalUseCase;
