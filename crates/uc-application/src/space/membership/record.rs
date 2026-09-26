//! 成员记录：持久层与成员状态负责人交换的纯数据。
//!
//! 一条记录保存当前 Space 的成员账本快照，以及与账本同存、同一修订号提交的非聚合资料：历史分页暂存与
//! 完成回执（历史交换协议的工作存储），分叉记录与分支恢复会话（分叉恢复本次不重写）。字节布局、格式
//! 版本与旧格式迁移由 Infra 负责，本模块不含任何编码。

mod branch_recovery;
mod history_exchange;
mod presentation;

use std::collections::BTreeMap;

use uc_core::ids::DeviceId;
use uc_core::membership::{
    MembershipBranchTransitionV1, MembershipConflictId, MembershipHistoryAckV3,
    MembershipLedgerSnapshot,
};

pub use branch_recovery::{
    MembershipBranchRecoverySession, MembershipBranchRecoverySessionState,
    MembershipConflictRecord, MembershipConflictStatus,
};
pub use history_exchange::InboundMembershipTransfer;
pub use presentation::{MembershipConflictMember, MembershipConflictPresentation};

#[derive(Clone, PartialEq, Eq)]
pub enum MembershipRecord {
    /// 没有当前 Space。保留修订号，使下一个 Space 的设备信任修订继续递增。
    NoSpace {
        revision: u64,
    },
    Space(Box<SpaceMembershipRecord>),
}

impl MembershipRecord {
    pub fn revision(&self) -> u64 {
        match self {
            Self::NoSpace { revision } => *revision,
            Self::Space(space) => space.ledger.revision,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SpaceMembershipRecord {
    pub ledger: MembershipLedgerSnapshot,
    pub history_exchange: MembershipHistoryExchangeRecord,
    pub branch_recovery: MembershipBranchRecoveryRecord,
}

/// 历史交换协议的工作存储：未收齐的入站分页与已完成传输的回执。
#[derive(Clone, PartialEq, Eq, Default)]
pub struct MembershipHistoryExchangeRecord {
    pub inbound_transfers: BTreeMap<DeviceId, InboundMembershipTransfer>,
    pub completed_inbound_transfers: BTreeMap<(DeviceId, [u8; 32]), MembershipHistoryAckV3>,
}

/// 分叉恢复的持久资料，结构与现有分叉恢复流程一致。
#[derive(Clone, PartialEq, Eq, Default)]
pub struct MembershipBranchRecoveryRecord {
    pub conflicts: BTreeMap<MembershipConflictId, MembershipConflictRecord>,
    pub conflict_presentations: BTreeMap<MembershipConflictId, MembershipConflictPresentation>,
    pub branch_transitions: BTreeMap<[u8; 32], MembershipBranchTransitionV1>,
    pub consumed_recovery_nonces: BTreeMap<[u8; 32], MembershipConflictId>,
    pub recovery_sessions: BTreeMap<[u8; 32], MembershipBranchRecoverySession>,
}

impl std::fmt::Debug for MembershipRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSpace { revision } => formatter
                .debug_struct("MembershipRecord::NoSpace")
                .field("revision", revision)
                .finish(),
            Self::Space(space) => formatter
                .debug_struct("MembershipRecord::Space")
                .field("revision", &space.ledger.revision)
                .field("peer_count", &space.ledger.peers.len())
                .field("effect_count", &space.ledger.effects.len())
                .field("history_exchange", &space.history_exchange)
                .field("branch_recovery", &space.branch_recovery)
                .finish(),
        }
    }
}

impl std::fmt::Debug for SpaceMembershipRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SpaceMembershipRecord")
            .field("revision", &self.ledger.revision)
            .field("peer_count", &self.ledger.peers.len())
            .field("effect_count", &self.ledger.effects.len())
            .field("history_exchange", &self.history_exchange)
            .field("branch_recovery", &self.branch_recovery)
            .finish()
    }
}

impl std::fmt::Debug for MembershipHistoryExchangeRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MembershipHistoryExchangeRecord")
            .field("inbound_transfer_count", &self.inbound_transfers.len())
            .field(
                "completed_inbound_transfer_count",
                &self.completed_inbound_transfers.len(),
            )
            .finish()
    }
}

impl std::fmt::Debug for MembershipBranchRecoveryRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MembershipBranchRecoveryRecord")
            .field("conflict_count", &self.conflicts.len())
            .field("branch_transition_count", &self.branch_transitions.len())
            .field(
                "consumed_recovery_nonce_count",
                &self.consumed_recovery_nonces.len(),
            )
            .field("recovery_session_count", &self.recovery_sessions.len())
            .finish()
    }
}
