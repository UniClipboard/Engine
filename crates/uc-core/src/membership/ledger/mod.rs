//! 成员账本：一个 Space 全部成员事实的唯一状态机。
//!
//! # 状态
//!
//! - 已验证签名成员历史：成员资格的唯一来源。
//! - 本机设备与成员实例。本机状态由历史与未完成效果得出：在有效成员中为有效；否则有影响本机的
//!   未完成效果时为激活中；其余为已移除。
//! - 每台对端一个 [`PeerLink`]：`Member` 表示当前历史中的成员，携带历史关系、已确认位置、同步退避与
//!   待投递的本机决定；`Departing` 表示已被本机移除、只剩一次移除通知的设备。
//! - 未完成的成员效果：只保存尚未激活的效果，激活即删除。
//! - 历史同步游标与修订号。
//!
//! # 输入
//!
//! 本机移除已签名、本机决定已签名、邀请方正式提交准入、对端历史证据已核对、历史同步已选定对端、
//! 历史同步结束、投递结束、离开窗口到期、成员效果阶段完成、分支已恢复、同存的非聚合资料已改变。新建与加入用 [`MembershipLedger::start`]，
//! 已保存状态用 [`MembershipLedger::restore`]。
//!
//! # 效果
//!
//! 全部为 `AfterCommit`：发布设备信任变化、唤醒执行器。准入正式提交的 `BeforeCommit` 由准入聚合声明。
//! 持久待办不作为效果返回，而由 [`MembershipLedger::outstanding_work`] 从状态计算。
//!
//! # 不变量与终态
//!
//! - 每个当前有效对端恰有一个 `Member`；`Departing` 只属于已不在历史成员中的设备；其余对端记录不存在。
//! - 本机是某项移除的目标时，不保留该移除决定的投递。
//! - 未完成效果只属于当前历史路径上的事件。
//! - `Departing` 在通知送达或自移除提交起 5 分钟后删除；本机已移除时不产生同步或决定投递。

mod aggregate;
mod effect;
mod error;
mod input;
mod peer_link;
mod present;
mod snapshot;
mod work;

#[cfg(test)]
mod tests;

pub use aggregate::MembershipLedger;
pub use effect::{
    MemberEffectKind, MemberEffectMaterial, MemberEffectPhase, UnfinishedMemberEffect,
};
pub use error::{LedgerTransitionError, LedgerTransitionErrorCategory};
pub use input::{
    LedgerDeliveryKind, LedgerDeliveryResult, LedgerEffect, LedgerFollowUp, LedgerInput,
    LedgerOutcome, LedgerTransition, PeerEvidence, PeerSyncResult,
};
pub use peer_link::{
    DepartingLink, MemberLink, PeerLink, PeerRelation, PeerSyncBackoff, PeerSyncOutcome,
    DEPARTURE_WINDOW_MS,
};
pub use present::{
    LedgerDeviceView, LedgerMemberStatus, LedgerScope, LedgerUpdateProblem, LedgerUpdateView,
    LedgerView, PeerPauseReason, PeerRelationView, PeerSyncView, SecurityDeliveryStatus,
};
pub use snapshot::{
    DepartingLinkSnapshot, MemberLinkSnapshot, MembershipLedgerSnapshot, PeerLinkSnapshot,
    PeerSyncBackoffSnapshot, UnfinishedMemberEffectSnapshot,
};
pub use work::{LedgerWork, ScheduledLedgerWork};
