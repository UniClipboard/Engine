//! 成员历史的完整网络交换，业务验证与恢复仍归原成员负责人。
mod budget;
mod exchange;
pub(crate) use budget::within_history_budget;
mod model;
mod ports;
mod receiver;
mod retention;

pub use exchange::{membership_history_purpose, MembershipHistoryTransport};
pub use model::{HistoryIncoming, HistoryReply, HistoryWork};
pub use ports::{
    HistoryConnectionRetentionPort, HistoryLocalWorkPort, HistoryPeerBindingPort,
    HistoryReceiverPort, HistoryWirePort,
};
pub use receiver::MembershipHistoryReceiver;
