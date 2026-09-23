use crate::membership::{BaseMembershipHistoryPosition, MembershipDecisionV2, MembershipEventV2};

use super::LedgerTransitionError;

/// 已被本机移除的设备只在这段时间内接收移除通知；到期后本机结束通知责任。
pub const DEPARTURE_WINDOW_MS: i64 = 300_000;
const INITIAL_RETRY_DELAY_MS: i64 = 1_000;
const MAX_RETRY_DELAY_MS: i64 = 5 * 60 * 1_000;

/// 本机与一个当前成员之间的历史关系。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerRelation {
    /// 尚未取得对端确认的历史结论。
    Unconfirmed,
    /// 双方历史一致。
    Consistent,
    /// 对端版本不支持当前成员历史流程。
    UpgradeRequired,
    /// 对端送来的历史中有一项移除等待本机决定。
    AwaitingLocalDecision,
    /// 双方历史已分叉，停止普通交换。
    Diverged,
    /// 对端资料无法验证。
    Invalid,
}

/// 最近一次向该对端同步历史的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerSyncOutcome {
    Never,
    Deferred,
    Acked,
    StableRejected,
}

/// 向一个对端同步历史的退避状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerSyncBackoff {
    pending_since_revision: Option<u64>,
    retry_attempt: u32,
    next_attempt_at_ms: i64,
    last_outcome: PeerSyncOutcome,
}

impl PeerSyncBackoff {
    pub(super) fn fresh(pending_since_revision: Option<u64>) -> Self {
        Self {
            pending_since_revision,
            retry_attempt: 0,
            next_attempt_at_ms: 0,
            last_outcome: PeerSyncOutcome::Never,
        }
    }

    pub(super) fn from_parts(
        pending_since_revision: Option<u64>,
        retry_attempt: u32,
        next_attempt_at_ms: i64,
        last_outcome: PeerSyncOutcome,
    ) -> Self {
        Self {
            pending_since_revision,
            retry_attempt,
            next_attempt_at_ms,
            last_outcome,
        }
    }

    pub fn pending_since_revision(&self) -> Option<u64> {
        self.pending_since_revision
    }

    pub fn retry_attempt(&self) -> u32 {
        self.retry_attempt
    }

    pub fn next_attempt_at_ms(&self) -> i64 {
        self.next_attempt_at_ms
    }

    pub fn last_outcome(&self) -> PeerSyncOutcome {
        self.last_outcome
    }

    pub(super) fn mark_pending(&mut self, revision: u64) {
        self.pending_since_revision.get_or_insert(revision);
    }

    pub(super) fn settle(&mut self, outcome: PeerSyncOutcome) {
        self.pending_since_revision = None;
        self.retry_attempt = 0;
        self.next_attempt_at_ms = 0;
        self.last_outcome = outcome;
    }

    pub(super) fn defer(&mut self, now_ms: i64) -> Result<(), LedgerTransitionError> {
        self.retry_attempt = self
            .retry_attempt
            .checked_add(1)
            .ok_or(LedgerTransitionError::RetryOverflow)?;
        self.next_attempt_at_ms = now_ms.saturating_add(retry_delay_ms(self.retry_attempt));
        self.last_outcome = PeerSyncOutcome::Deferred;
        Ok(())
    }

    /// 最早可以再次同步的时间。持久截止时间与当前时间的距离超过本次退避窗口时说明系统时间倒退，
    /// 立即到期而不是等待 wall clock 追上旧值。
    pub(super) fn due_at_ms(&self, now_ms: i64) -> i64 {
        let clock_regressed = self.retry_attempt > 0
            && self.next_attempt_at_ms.saturating_sub(now_ms) > retry_delay_ms(self.retry_attempt);
        if self.next_attempt_at_ms <= now_ms || clock_regressed {
            now_ms
        } else {
            self.next_attempt_at_ms
        }
    }
}

fn retry_delay_ms(retry_attempt: u32) -> i64 {
    let shift = retry_attempt.saturating_sub(1).min(18);
    INITIAL_RETRY_DELAY_MS
        .checked_shl(shift)
        .unwrap_or(MAX_RETRY_DELAY_MS)
        .min(MAX_RETRY_DELAY_MS)
}

/// 当前历史中的一个对端成员。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberLink {
    relation: PeerRelation,
    confirmed_position: Option<BaseMembershipHistoryPosition>,
    sync: PeerSyncBackoff,
    outgoing_decision: Option<Box<MembershipDecisionV2>>,
}

impl MemberLink {
    pub(super) fn new(relation: PeerRelation, sync: PeerSyncBackoff) -> Self {
        Self {
            relation,
            confirmed_position: None,
            sync,
            outgoing_decision: None,
        }
    }

    pub(super) fn from_parts(
        relation: PeerRelation,
        confirmed_position: Option<BaseMembershipHistoryPosition>,
        sync: PeerSyncBackoff,
        outgoing_decision: Option<MembershipDecisionV2>,
    ) -> Self {
        Self {
            relation,
            confirmed_position,
            sync,
            outgoing_decision: outgoing_decision.map(Box::new),
        }
    }

    pub fn relation(&self) -> PeerRelation {
        self.relation
    }

    pub fn confirmed_position(&self) -> Option<&BaseMembershipHistoryPosition> {
        self.confirmed_position.as_ref()
    }

    pub fn sync(&self) -> &PeerSyncBackoff {
        &self.sync
    }

    pub fn outgoing_decision(&self) -> Option<&MembershipDecisionV2> {
        self.outgoing_decision.as_deref()
    }

    /// 一致但尚未确认本机当前位置。
    pub(super) fn awaits_confirmation(&self, current: &BaseMembershipHistoryPosition) -> bool {
        self.relation == PeerRelation::Consistent
            && self.confirmed_position.as_ref() != Some(current)
    }

    /// 仍需要把本机历史送给该对端核对。分叉的对端停止成员历史交换。
    pub(super) fn needs_history_sync(&self, current: &BaseMembershipHistoryPosition) -> bool {
        match self.relation {
            PeerRelation::Consistent => self.awaits_confirmation(current),
            PeerRelation::Unconfirmed
            | PeerRelation::UpgradeRequired
            | PeerRelation::AwaitingLocalDecision
            | PeerRelation::Invalid => true,
            PeerRelation::Diverged => false,
        }
    }

    pub(super) fn sync_mut(&mut self) -> &mut PeerSyncBackoff {
        &mut self.sync
    }

    pub(super) fn record_relation(
        &mut self,
        relation: PeerRelation,
        confirmed_position: Option<BaseMembershipHistoryPosition>,
    ) {
        self.relation = relation;
        self.confirmed_position = confirmed_position;
    }

    pub(super) fn forget_confirmation(&mut self) {
        self.confirmed_position = None;
    }

    pub(super) fn queue_decision(&mut self, decision: Option<MembershipDecisionV2>) {
        self.outgoing_decision = decision.map(Box::new);
    }
}

/// 已被本机移除、只剩一次移除通知的设备。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepartingLink {
    notice: Box<MembershipEventV2>,
    since_ms: i64,
}

impl DepartingLink {
    pub(super) fn new(notice: MembershipEventV2, since_ms: i64) -> Self {
        Self {
            notice: Box::new(notice),
            since_ms,
        }
    }

    pub fn notice(&self) -> &MembershipEventV2 {
        &self.notice
    }

    pub fn since_ms(&self) -> i64 {
        self.since_ms
    }

    pub fn expires_at_ms(&self) -> i64 {
        self.since_ms.saturating_add(DEPARTURE_WINDOW_MS)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerLink {
    Member(MemberLink),
    Departing(DepartingLink),
}
