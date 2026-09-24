use uc_core::ids::DeviceId;
use uc_core::membership::{
    LedgerEffect, LedgerFollowUp, LedgerInput, LedgerOutcome, LedgerTransitionError,
    MemberInstanceId, MembershipLedger, PeerEvidence, PeerSyncResult, VersionedMembershipHistory,
};

use crate::space::membership::{
    MembershipBranchRecoveryRecord, MembershipHistoryExchangeRecord, MembershipLedgerError,
};

use super::view::{MembershipView, SpaceMembershipView};

/// 一次提交中的待定成员状态。
///
/// 账本只能经 [`MembershipDraft::apply`] 由 Core 规则推进；同存资料可以直接修改，Owner 在提交时
/// 比较差异并推进修订号。草稿不保存任何东西，由 Owner 决定是否提交。
pub(crate) struct MembershipDraft {
    now_ms: i64,
    base: std::sync::Arc<MembershipView>,
    revision: u64,
    space: Option<SpaceMembershipView>,
    follow_ups: Vec<LedgerFollowUp>,
    /// 本次提交中新确认了本机当前位置的对端：与它的已认证成员历史交换刚刚成功。
    confirmed_peers: Vec<DeviceId>,
    revised: bool,
}

pub(super) struct FinishedDraft {
    pub(super) view: MembershipView,
    pub(super) follow_ups: Vec<LedgerFollowUp>,
    pub(super) confirmed_peers: Vec<DeviceId>,
}

impl MembershipDraft {
    pub(super) fn new(base: std::sync::Arc<MembershipView>, now_ms: i64) -> Self {
        Self {
            now_ms,
            revision: base.revision(),
            space: base.space().cloned(),
            base,
            follow_ups: Vec::new(),
            confirmed_peers: Vec::new(),
            revised: false,
        }
    }

    pub(crate) fn space(&self) -> Option<&SpaceMembershipView> {
        self.space.as_ref()
    }

    pub(crate) fn require_space(&self) -> Result<&SpaceMembershipView, MembershipLedgerError> {
        self.space
            .as_ref()
            .ok_or(MembershipLedgerError::RecoveryRequired)
    }

    /// 以 Core 规则推进账本。没有当前 Space 时输入无处可用，按输入不匹配拒绝。
    pub(crate) fn apply(
        &mut self,
        input: LedgerInput,
    ) -> Result<LedgerOutcome, LedgerTransitionError> {
        let space = self
            .space
            .as_mut()
            .ok_or(LedgerTransitionError::InputMismatch)?;
        let confirmed_peer = match &input {
            LedgerInput::HistorySyncFinished {
                peer,
                result: PeerSyncResult::Confirmed,
                ..
            } => Some(*peer),
            LedgerInput::PeerEvidenceReconciled {
                source,
                evidence: PeerEvidence::Confirmed,
                ..
            } => Some(*source),
            _ => None,
        };
        let (ledger, outcome, effects) =
            space.ledger.clone().apply(input, self.now_ms)?.into_parts();
        if outcome == LedgerOutcome::Applied {
            space.ledger = ledger;
            self.revision = space.ledger.revision();
            self.revised = true;
            for LedgerEffect::AfterCommit(follow_up) in effects {
                push_unique(&mut self.follow_ups, follow_up);
            }
            if let Some(peer) = confirmed_peer {
                if !self.confirmed_peers.contains(&peer) {
                    self.confirmed_peers.push(peer);
                }
            }
        }
        Ok(outcome)
    }

    pub(crate) fn history_exchange_mut(
        &mut self,
    ) -> Result<&mut MembershipHistoryExchangeRecord, MembershipLedgerError> {
        self.space
            .as_mut()
            .map(|space| &mut space.history_exchange)
            .ok_or(MembershipLedgerError::RecoveryRequired)
    }

    pub(crate) fn branch_recovery_mut(
        &mut self,
    ) -> Result<&mut MembershipBranchRecoveryRecord, MembershipLedgerError> {
        self.space
            .as_mut()
            .map(|space| &mut space.branch_recovery)
            .ok_or(MembershipLedgerError::RecoveryRequired)
    }

    /// 新建 Space 或以新的单成员起点重建：只能从没有当前 Space 的状态开始。
    pub(crate) fn start_space(
        &mut self,
        history: VersionedMembershipHistory,
        local_device_id: DeviceId,
        local_member: MemberInstanceId,
    ) -> Result<(), MembershipLedgerError> {
        if self.space.is_some() {
            return Err(MembershipLedgerError::Conflict);
        }
        let revision = self
            .revision
            .checked_add(1)
            .ok_or(MembershipLedgerError::Corrupt)?;
        let ledger = MembershipLedger::start(history, local_device_id, local_member, revision)
            .map_err(|_| MembershipLedgerError::Corrupt)?;
        self.space = Some(SpaceMembershipView {
            ledger,
            history_exchange: MembershipHistoryExchangeRecord::default(),
            branch_recovery: MembershipBranchRecoveryRecord::default(),
        });
        self.revision = revision;
        self.revised = true;
        push_unique(
            &mut self.follow_ups,
            LedgerFollowUp::PublishDeviceTrustChange,
        );
        push_unique(&mut self.follow_ups, LedgerFollowUp::WakeWorker);
        Ok(())
    }

    /// 结束当前 Space 的全部成员事实；修订号继续递增。
    pub(crate) fn clear_space(&mut self) -> Result<(), MembershipLedgerError> {
        if self.space.take().is_none() {
            return Ok(());
        }
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or(MembershipLedgerError::Corrupt)?;
        self.revised = true;
        push_unique(
            &mut self.follow_ups,
            LedgerFollowUp::PublishDeviceTrustChange,
        );
        Ok(())
    }

    /// 形成替换状态；没有任何变化时返回 `None`。
    pub(super) fn finish(mut self) -> Result<Option<FinishedDraft>, MembershipLedgerError> {
        let (exchange_changed, branch_changed) = match (self.base.space(), self.space.as_ref()) {
            (Some(before), Some(after)) => (
                before.history_exchange != after.history_exchange,
                before.branch_recovery != after.branch_recovery,
            ),
            _ => (false, false),
        };
        if (exchange_changed || branch_changed) && !self.revised {
            self.apply(LedgerInput::CompanionDataChanged {
                presentation_changed: branch_changed,
            })
            .map_err(|_| MembershipLedgerError::Corrupt)?;
        } else if branch_changed {
            push_unique(
                &mut self.follow_ups,
                LedgerFollowUp::PublishDeviceTrustChange,
            );
        }
        if !self.revised {
            return Ok(None);
        }
        Ok(Some(FinishedDraft {
            view: MembershipView::new(self.revision, self.space),
            follow_ups: self.follow_ups,
            confirmed_peers: self.confirmed_peers,
        }))
    }
}

fn push_unique(follow_ups: &mut Vec<LedgerFollowUp>, follow_up: LedgerFollowUp) {
    if !follow_ups.contains(&follow_up) {
        follow_ups.push(follow_up);
    }
}
