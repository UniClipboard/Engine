use sha2::{Digest, Sha256};
use uc_core::ids::DeviceId;
use uc_core::membership::{
    MemberInstanceId, MembershipLedger, PeerPauseReason, VersionedMembershipHistory,
};

use crate::space::membership::{
    CurrentSpaceMemberScope, CurrentSpaceMemberScopeError, MembershipBranchRecoveryRecord,
    MembershipHistoryExchangeRecord, MembershipLedgerError, MembershipRecord, PausedSpaceMember,
    SpaceMemberPauseReason, SpaceMembershipRecord,
};

/// Owner 发布的只读成员状态。读取方只能观察，不能改写；改写只经 [`super::MembershipOwner`]。
#[derive(Clone)]
pub(crate) struct MembershipView {
    revision: u64,
    space: Option<SpaceMembershipView>,
}

/// 当前 Space 的成员账本与同存资料。
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct SpaceMembershipView {
    pub(super) ledger: MembershipLedger,
    pub(super) history_exchange: MembershipHistoryExchangeRecord,
    pub(super) branch_recovery: MembershipBranchRecoveryRecord,
}

impl MembershipView {
    pub(super) fn new(revision: u64, space: Option<SpaceMembershipView>) -> Self {
        Self { revision, space }
    }

    /// 从持久记录恢复并重新校验全部不变量；不一致的记录一律视为损坏，不做修复。
    pub(crate) fn from_record(record: MembershipRecord) -> Result<Self, MembershipLedgerError> {
        match record {
            MembershipRecord::NoSpace { revision } => Ok(Self::new(revision, None)),
            MembershipRecord::Space(space) => {
                let SpaceMembershipRecord {
                    ledger,
                    history_exchange,
                    branch_recovery,
                } = *space;
                let ledger = MembershipLedger::restore(ledger)
                    .map_err(MembershipLedgerError::corrupt_from)?;
                if branch_recovery
                    .conflict_presentations
                    .iter()
                    .any(|(id, presentation)| {
                        branch_recovery
                            .conflicts
                            .get(id)
                            .is_none_or(|conflict| !presentation.matches_record(conflict))
                    })
                    || branch_recovery
                        .recovery_sessions
                        .iter()
                        .any(|(key, session)| key != session.transition_id() || !session.validate())
                {
                    return Err(MembershipLedgerError::corrupt());
                }
                Ok(Self::new(
                    ledger.revision(),
                    Some(SpaceMembershipView {
                        ledger,
                        history_exchange,
                        branch_recovery,
                    }),
                ))
            }
        }
    }

    pub(super) fn to_record(&self) -> MembershipRecord {
        match &self.space {
            None => MembershipRecord::NoSpace {
                revision: self.revision,
            },
            Some(space) => MembershipRecord::Space(Box::new(SpaceMembershipRecord {
                ledger: space.ledger.snapshot(),
                history_exchange: space.history_exchange.clone(),
                branch_recovery: space.branch_recovery.clone(),
            })),
        }
    }

    /// 设备信任修订号：每次成员记录提交都单调递增。
    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn space(&self) -> Option<&SpaceMembershipView> {
        self.space.as_ref()
    }

    /// 需要当前 Space 的读取；没有当前 Space 时说明调用方所依赖的资料已不存在。
    pub(crate) fn require_space(&self) -> Result<&SpaceMembershipView, MembershipLedgerError> {
        self.space
            .as_ref()
            .ok_or(MembershipLedgerError::RecoveryRequired)
    }

    pub(crate) fn current_scope(
        &self,
    ) -> Result<CurrentSpaceMemberScope, CurrentSpaceMemberScopeError> {
        let space = self
            .space
            .as_ref()
            .ok_or(CurrentSpaceMemberScopeError::NoCurrentSpace)?;
        let scope = space
            .ledger
            .scope()
            .map_err(CurrentSpaceMemberScopeError::recovery_required_from)?;
        let mut usable_peer_device_ids = scope.usable_peer_device_ids;
        usable_peer_device_ids.sort();
        let mut paused_peer_devices: Vec<PausedSpaceMember> = scope
            .paused_peer_devices
            .into_iter()
            .map(|(device_id, reason)| PausedSpaceMember {
                device_id,
                reason: pause_reason(reason),
            })
            .collect();
        paused_peer_devices.sort_by_key(|member| member.device_id);
        Ok(CurrentSpaceMemberScope {
            revision: self.revision,
            local_member_active: scope.local_member_active,
            usable_peer_device_ids,
            paused_peer_devices,
        })
    }
}

impl SpaceMembershipView {
    pub(crate) fn ledger(&self) -> &MembershipLedger {
        &self.ledger
    }

    pub(crate) fn history(&self) -> &VersionedMembershipHistory {
        self.ledger.history()
    }

    pub(crate) fn local_device_id(&self) -> &DeviceId {
        self.ledger.local_device_id()
    }

    pub(crate) fn local_member(&self) -> MemberInstanceId {
        self.ledger.local_member()
    }

    pub(crate) fn history_exchange(&self) -> &MembershipHistoryExchangeRecord {
        &self.history_exchange
    }

    pub(crate) fn branch_recovery(&self) -> &MembershipBranchRecoveryRecord {
        &self.branch_recovery
    }

    /// 已提交历史归档的摘要，作为公开提交回执的一部分。
    pub(crate) fn history_digest(&self) -> Result<[u8; 32], MembershipLedgerError> {
        let bytes = self
            .history()
            .encode_persisted_v2()
            .map_err(MembershipLedgerError::corrupt_from)?;
        Ok(Sha256::digest(bytes).into())
    }
}

pub(crate) fn pause_reason(reason: PeerPauseReason) -> SpaceMemberPauseReason {
    match reason {
        PeerPauseReason::LocalMemberInactive => SpaceMemberPauseReason::LocalMemberInactive,
        PeerPauseReason::PendingLocalDecision => SpaceMemberPauseReason::PendingLocalDecision,
        PeerPauseReason::Diverged => SpaceMemberPauseReason::Diverged,
        PeerPauseReason::Invalid => SpaceMemberPauseReason::Invalid,
        PeerPauseReason::UpgradeRequired => SpaceMemberPauseReason::UpgradeRequired,
        PeerPauseReason::RelationshipUnconfirmed => SpaceMemberPauseReason::RelationshipUnconfirmed,
        PeerPauseReason::EffectPending => SpaceMemberPauseReason::EffectPending,
    }
}

impl std::fmt::Debug for MembershipView {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MembershipView")
            .field("revision", &self.revision)
            .field("has_current_space", &self.space.is_some())
            .finish()
    }
}
