use std::collections::BTreeSet;

use crate::ids::DeviceId;
use crate::membership::MemberInstanceId;

use super::{LedgerTransitionError, MembershipLedger, PeerLink, PeerRelation, PeerSyncOutcome};

/// 一台设备在本机当前历史中的成员状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedgerMemberStatus {
    Active,
    PendingActivation,
    Removed,
}

/// 公开的设备组关系。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerRelationView {
    Local,
    Consistent,
    ConfirmationPending,
    PendingLocalDecision,
    AwaitingRemovalAcknowledgement,
    UpgradeRequired,
    Diverged,
    Invalid,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerPauseReason {
    LocalMemberInactive,
    PendingLocalDecision,
    Diverged,
    Invalid,
    UpgradeRequired,
    RelationshipUnconfirmed,
    EffectPending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerSyncView {
    Usable,
    Paused(PeerPauseReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerDeviceView {
    pub device_id: DeviceId,
    pub is_local: bool,
    pub member: Option<MemberInstanceId>,
    pub status: LedgerMemberStatus,
    pub relation: PeerRelationView,
    pub sync: PeerSyncView,
}

/// 普通消费者可用的对端范围；暂停的对端只能继续缩小，不能加回可用范围。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerScope {
    pub local_member_active: bool,
    pub usable_peer_device_ids: Vec<DeviceId>,
    pub paused_peer_devices: Vec<(DeviceId, PeerPauseReason)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedgerUpdateProblem {
    DeviceStateRejected,
    DeviceRelationshipConflict,
    DeviceSecurityUpdateRejected,
    DeviceUpgradeRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedgerUpdateView {
    Updating,
    Completed,
    RetryableFailure { next_retry_at_ms: i64 },
    NeedsAttention(LedgerUpdateProblem),
}

/// 组密钥更新投递的观察结果，由独立存储给出。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityDeliveryStatus {
    Completed,
    Updating,
    RetryableFailure { next_retry_at_ms: i64 },
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerView {
    pub local_status: LedgerMemberStatus,
    pub scope: LedgerScope,
    pub devices: Vec<LedgerDeviceView>,
    pub device_update: LedgerUpdateView,
}

impl MembershipLedger {
    pub fn present(
        &self,
        security: SecurityDeliveryStatus,
    ) -> Result<LedgerView, LedgerTransitionError> {
        let local_status = self.local_status();
        let scope = self.scope()?;
        let devices = self.device_views(&scope)?;
        let device_update = self.device_update(local_status, &devices, security)?;
        Ok(LedgerView {
            local_status,
            scope,
            devices,
            device_update,
        })
    }

    /// 已激活对端的可用与暂停范围。
    pub fn scope(&self) -> Result<LedgerScope, LedgerTransitionError> {
        let local_member_active = self.local_status() == LedgerMemberStatus::Active;
        let mut usable_peer_device_ids = Vec::new();
        let mut paused_peer_devices = Vec::new();
        for device in self.active_peer_devices()? {
            let reason = if !local_member_active {
                Some(PeerPauseReason::LocalMemberInactive)
            } else if self.effect_affects(&device) {
                Some(PeerPauseReason::EffectPending)
            } else {
                match self.peers.get(&device) {
                    Some(PeerLink::Member(link)) => relation_pause(link.relation()),
                    Some(PeerLink::Departing(_)) | None => {
                        Some(PeerPauseReason::RelationshipUnconfirmed)
                    }
                }
            };
            match reason {
                Some(reason) => paused_peer_devices.push((device, reason)),
                None => usable_peer_device_ids.push(device),
            }
        }
        Ok(LedgerScope {
            local_member_active,
            usable_peer_device_ids,
            paused_peer_devices,
        })
    }

    fn device_views(
        &self,
        scope: &LedgerScope,
    ) -> Result<Vec<LedgerDeviceView>, LedgerTransitionError> {
        let current = self.history.current_position()?;
        let active = self.history.active_members();
        let mut device_ids: BTreeSet<DeviceId> = self.active_peer_devices()?;
        device_ids.insert(self.local_device_id);
        device_ids.extend(self.peers.keys().cloned());
        let candidates: Vec<DeviceId> = device_ids.iter().cloned().collect();
        let mut views = Vec::with_capacity(device_ids.len());
        for device_id in device_ids {
            let is_local = device_id == self.local_device_id;
            let member = if is_local {
                Some(self.local_member)
            } else {
                self.history.member_for_device(&device_id, &candidates)
            };
            let status = if is_local {
                self.local_status()
            } else if member.is_some_and(|member| active.contains(&member)) {
                LedgerMemberStatus::Active
            } else if self.effect_affects(&device_id) {
                LedgerMemberStatus::PendingActivation
            } else {
                LedgerMemberStatus::Removed
            };
            let relation = if is_local {
                PeerRelationView::Local
            } else {
                match self.peers.get(&device_id) {
                    Some(PeerLink::Member(link))
                        if status == LedgerMemberStatus::Active
                            && link.awaits_confirmation(&current) =>
                    {
                        PeerRelationView::ConfirmationPending
                    }
                    Some(PeerLink::Member(link)) => relation_view(link.relation()),
                    Some(PeerLink::Departing(_)) => {
                        PeerRelationView::AwaitingRemovalAcknowledgement
                    }
                    None => PeerRelationView::Unknown,
                }
            };
            let sync = if status == LedgerMemberStatus::Removed
                || relation == PeerRelationView::AwaitingRemovalAcknowledgement
            {
                PeerSyncView::Paused(PeerPauseReason::LocalMemberInactive)
            } else if is_local || scope.usable_peer_device_ids.contains(&device_id) {
                PeerSyncView::Usable
            } else if let Some((_, reason)) = scope
                .paused_peer_devices
                .iter()
                .find(|(paused, _)| *paused == device_id)
            {
                PeerSyncView::Paused(*reason)
            } else if self.effect_affects(&device_id) {
                PeerSyncView::Paused(PeerPauseReason::EffectPending)
            } else {
                match self.peers.get(&device_id) {
                    Some(PeerLink::Member(link)) => PeerSyncView::Paused(
                        relation_pause(link.relation())
                            .unwrap_or(PeerPauseReason::RelationshipUnconfirmed),
                    ),
                    Some(PeerLink::Departing(_)) | None => {
                        PeerSyncView::Paused(PeerPauseReason::RelationshipUnconfirmed)
                    }
                }
            };
            views.push(LedgerDeviceView {
                device_id,
                is_local,
                member,
                status,
                relation,
                sync,
            });
        }
        Ok(views)
    }

    fn device_update(
        &self,
        local_status: LedgerMemberStatus,
        devices: &[LedgerDeviceView],
        security: SecurityDeliveryStatus,
    ) -> Result<LedgerUpdateView, LedgerTransitionError> {
        if security == SecurityDeliveryStatus::Rejected {
            return Ok(LedgerUpdateView::NeedsAttention(
                LedgerUpdateProblem::DeviceSecurityUpdateRejected,
            ));
        }
        // 本机已移除是终态：除仍在收尾的本地效果外，没有任何设备更新可做。
        if local_status == LedgerMemberStatus::Removed {
            return Ok(if self.effects.is_empty() {
                LedgerUpdateView::Completed
            } else {
                LedgerUpdateView::Updating
            });
        }
        let current = self.history.current_position()?;
        let mut next_retry_at_ms: Option<i64> = None;
        let mut history_update_pending = false;
        for link in self.peers.values() {
            if let PeerLink::Member(member) = link {
                history_update_pending |= member.outgoing_decision().is_some();
            }
        }
        if local_status == LedgerMemberStatus::Active {
            for device in self.active_peer_devices()? {
                let Some(PeerLink::Member(member)) = self.peers.get(&device) else {
                    history_update_pending = true;
                    continue;
                };
                let needs_sync = member.needs_history_sync(&current);
                match member.sync().last_outcome() {
                    PeerSyncOutcome::StableRejected => {
                        return Ok(LedgerUpdateView::NeedsAttention(
                            LedgerUpdateProblem::DeviceStateRejected,
                        ));
                    }
                    PeerSyncOutcome::Deferred if needs_sync => {
                        let candidate = member.sync().next_attempt_at_ms();
                        next_retry_at_ms =
                            Some(next_retry_at_ms.map_or(candidate, |known| known.min(candidate)));
                    }
                    PeerSyncOutcome::Never | PeerSyncOutcome::Acked if needs_sync => {
                        history_update_pending = true;
                    }
                    PeerSyncOutcome::Never | PeerSyncOutcome::Deferred | PeerSyncOutcome::Acked => {
                    }
                }
            }
        }
        for device in devices
            .iter()
            .filter(|device| device.status != LedgerMemberStatus::Removed)
        {
            match device.relation {
                PeerRelationView::PendingLocalDecision
                | PeerRelationView::Diverged
                | PeerRelationView::Invalid => {
                    return Ok(LedgerUpdateView::NeedsAttention(
                        LedgerUpdateProblem::DeviceRelationshipConflict,
                    ));
                }
                PeerRelationView::UpgradeRequired => {
                    return Ok(LedgerUpdateView::NeedsAttention(
                        LedgerUpdateProblem::DeviceUpgradeRequired,
                    ));
                }
                PeerRelationView::Local
                | PeerRelationView::Consistent
                | PeerRelationView::ConfirmationPending
                | PeerRelationView::AwaitingRemovalAcknowledgement
                | PeerRelationView::Unknown => {}
            }
        }
        if let Some(next_retry_at_ms) = next_retry_at_ms {
            return Ok(LedgerUpdateView::RetryableFailure { next_retry_at_ms });
        }
        if let SecurityDeliveryStatus::RetryableFailure { next_retry_at_ms } = security {
            return Ok(LedgerUpdateView::RetryableFailure { next_retry_at_ms });
        }
        let relationship_update_pending = devices.iter().any(|device| {
            device.status == LedgerMemberStatus::PendingActivation
                || (device.status != LedgerMemberStatus::Removed
                    && matches!(
                        device.relation,
                        PeerRelationView::ConfirmationPending | PeerRelationView::Unknown
                    ))
        });
        Ok(
            if !self.effects.is_empty()
                || history_update_pending
                || relationship_update_pending
                || security == SecurityDeliveryStatus::Updating
            {
                LedgerUpdateView::Updating
            } else {
                LedgerUpdateView::Completed
            },
        )
    }
}

fn relation_view(relation: PeerRelation) -> PeerRelationView {
    match relation {
        PeerRelation::Unconfirmed => PeerRelationView::Unknown,
        PeerRelation::Consistent => PeerRelationView::Consistent,
        PeerRelation::UpgradeRequired => PeerRelationView::UpgradeRequired,
        PeerRelation::AwaitingLocalDecision => PeerRelationView::PendingLocalDecision,
        PeerRelation::AwaitingPeerDecision => PeerRelationView::ConfirmationPending,
        PeerRelation::Diverged => PeerRelationView::Diverged,
        PeerRelation::Invalid => PeerRelationView::Invalid,
    }
}

fn relation_pause(relation: PeerRelation) -> Option<PeerPauseReason> {
    match relation {
        PeerRelation::Consistent => None,
        PeerRelation::Unconfirmed => Some(PeerPauseReason::RelationshipUnconfirmed),
        PeerRelation::UpgradeRequired => Some(PeerPauseReason::UpgradeRequired),
        PeerRelation::AwaitingLocalDecision => Some(PeerPauseReason::PendingLocalDecision),
        // 对端尚未决定本机发起的移除，不越过该移除向其分享普通内容（ADR-020）。
        PeerRelation::AwaitingPeerDecision => Some(PeerPauseReason::RelationshipUnconfirmed),
        PeerRelation::Diverged => Some(PeerPauseReason::Diverged),
        PeerRelation::Invalid => Some(PeerPauseReason::Invalid),
    }
}
