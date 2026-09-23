use std::collections::BTreeSet;

use crate::ids::DeviceId;
use crate::membership::MemberInstanceId;

use super::{HistorySyncOutcome, PeerLink, PeerRelation, SpaceMembership, SpaceMembershipError};

/// 一台设备在本机当前历史中的成员状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberStatus {
    Active,
    PendingActivation,
    Removed,
}

/// 公开的设备组关系。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationView {
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
pub enum PauseReason {
    LocalMemberInactive,
    PendingLocalDecision,
    Diverged,
    Invalid,
    UpgradeRequired,
    RelationshipUnconfirmed,
    EffectPending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncView {
    Usable,
    Paused(PauseReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceMembershipView {
    pub device_id: DeviceId,
    pub is_local: bool,
    pub member: Option<MemberInstanceId>,
    pub status: MemberStatus,
    pub relation: RelationView,
    pub sync: SyncView,
}

/// 普通消费者可用的对端范围；暂停的对端只能继续缩小，不能加回可用范围。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MembershipScope {
    pub local_member_active: bool,
    pub usable_peer_device_ids: Vec<DeviceId>,
    pub paused_peer_devices: Vec<(DeviceId, PauseReason)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceUpdateProblem {
    DeviceStateRejected,
    DeviceRelationshipConflict,
    DeviceSecurityUpdateRejected,
    DeviceUpgradeRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceUpdateView {
    Updating,
    Completed,
    RetryableFailure { next_retry_at_ms: i64 },
    NeedsAttention(DeviceUpdateProblem),
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
pub struct MembershipView {
    pub local_status: MemberStatus,
    pub scope: MembershipScope,
    pub devices: Vec<DeviceMembershipView>,
    pub device_update: DeviceUpdateView,
}

impl SpaceMembership {
    pub fn present(
        &self,
        security: SecurityDeliveryStatus,
    ) -> Result<MembershipView, SpaceMembershipError> {
        let local_status = self.local_status();
        let scope = self.scope()?;
        let devices = self.device_views(&scope)?;
        let device_update = self.device_update(local_status, &devices, security)?;
        Ok(MembershipView {
            local_status,
            scope,
            devices,
            device_update,
        })
    }

    /// 已激活对端的可用与暂停范围。
    pub fn scope(&self) -> Result<MembershipScope, SpaceMembershipError> {
        let local_member_active = self.local_status() == MemberStatus::Active;
        let mut usable_peer_device_ids = Vec::new();
        let mut paused_peer_devices = Vec::new();
        for device in self.active_peer_devices()? {
            let reason = if !local_member_active {
                Some(PauseReason::LocalMemberInactive)
            } else if self.effect_affects(&device) {
                Some(PauseReason::EffectPending)
            } else {
                match self.peers.get(&device) {
                    Some(PeerLink::Member(link)) => relation_pause(link.relation()),
                    Some(PeerLink::Departing(_)) | None => {
                        Some(PauseReason::RelationshipUnconfirmed)
                    }
                }
            };
            match reason {
                Some(reason) => paused_peer_devices.push((device, reason)),
                None => usable_peer_device_ids.push(device),
            }
        }
        Ok(MembershipScope {
            local_member_active,
            usable_peer_device_ids,
            paused_peer_devices,
        })
    }

    fn device_views(
        &self,
        scope: &MembershipScope,
    ) -> Result<Vec<DeviceMembershipView>, SpaceMembershipError> {
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
                MemberStatus::Active
            } else if self.effect_affects(&device_id) {
                MemberStatus::PendingActivation
            } else {
                MemberStatus::Removed
            };
            let relation = if is_local {
                RelationView::Local
            } else {
                match self.peers.get(&device_id) {
                    Some(PeerLink::Member(link))
                        if status == MemberStatus::Active && link.awaits_confirmation(&current) =>
                    {
                        RelationView::ConfirmationPending
                    }
                    Some(PeerLink::Member(link)) => relation_view(link.relation()),
                    Some(PeerLink::Departing(_)) => RelationView::AwaitingRemovalAcknowledgement,
                    None => RelationView::Unknown,
                }
            };
            let sync = if status == MemberStatus::Removed
                || relation == RelationView::AwaitingRemovalAcknowledgement
            {
                SyncView::Paused(PauseReason::LocalMemberInactive)
            } else if is_local || scope.usable_peer_device_ids.contains(&device_id) {
                SyncView::Usable
            } else if let Some((_, reason)) = scope
                .paused_peer_devices
                .iter()
                .find(|(paused, _)| *paused == device_id)
            {
                SyncView::Paused(*reason)
            } else if self.effect_affects(&device_id) {
                SyncView::Paused(PauseReason::EffectPending)
            } else {
                match self.peers.get(&device_id) {
                    Some(PeerLink::Member(link)) => SyncView::Paused(
                        relation_pause(link.relation())
                            .unwrap_or(PauseReason::RelationshipUnconfirmed),
                    ),
                    Some(PeerLink::Departing(_)) | None => {
                        SyncView::Paused(PauseReason::RelationshipUnconfirmed)
                    }
                }
            };
            views.push(DeviceMembershipView {
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
        local_status: MemberStatus,
        devices: &[DeviceMembershipView],
        security: SecurityDeliveryStatus,
    ) -> Result<DeviceUpdateView, SpaceMembershipError> {
        if security == SecurityDeliveryStatus::Rejected {
            return Ok(DeviceUpdateView::NeedsAttention(
                DeviceUpdateProblem::DeviceSecurityUpdateRejected,
            ));
        }
        // 本机已移除是终态：除仍在收尾的本地效果外，没有任何设备更新可做。
        if local_status == MemberStatus::Removed {
            return Ok(if self.effects.is_empty() {
                DeviceUpdateView::Completed
            } else {
                DeviceUpdateView::Updating
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
        if local_status == MemberStatus::Active {
            for device in self.active_peer_devices()? {
                let Some(PeerLink::Member(member)) = self.peers.get(&device) else {
                    history_update_pending = true;
                    continue;
                };
                let needs_sync = member.needs_history_sync(&current);
                match member.sync().last_outcome() {
                    HistorySyncOutcome::StableRejected => {
                        return Ok(DeviceUpdateView::NeedsAttention(
                            DeviceUpdateProblem::DeviceStateRejected,
                        ));
                    }
                    HistorySyncOutcome::Deferred if needs_sync => {
                        let candidate = member.sync().next_attempt_at_ms();
                        next_retry_at_ms =
                            Some(next_retry_at_ms.map_or(candidate, |known| known.min(candidate)));
                    }
                    HistorySyncOutcome::Never | HistorySyncOutcome::Acked if needs_sync => {
                        history_update_pending = true;
                    }
                    HistorySyncOutcome::Never
                    | HistorySyncOutcome::Deferred
                    | HistorySyncOutcome::Acked => {}
                }
            }
        }
        for device in devices
            .iter()
            .filter(|device| device.status != MemberStatus::Removed)
        {
            match device.relation {
                RelationView::PendingLocalDecision
                | RelationView::Diverged
                | RelationView::Invalid => {
                    return Ok(DeviceUpdateView::NeedsAttention(
                        DeviceUpdateProblem::DeviceRelationshipConflict,
                    ));
                }
                RelationView::UpgradeRequired => {
                    return Ok(DeviceUpdateView::NeedsAttention(
                        DeviceUpdateProblem::DeviceUpgradeRequired,
                    ));
                }
                RelationView::Local
                | RelationView::Consistent
                | RelationView::ConfirmationPending
                | RelationView::AwaitingRemovalAcknowledgement
                | RelationView::Unknown => {}
            }
        }
        if let Some(next_retry_at_ms) = next_retry_at_ms {
            return Ok(DeviceUpdateView::RetryableFailure { next_retry_at_ms });
        }
        if let SecurityDeliveryStatus::RetryableFailure { next_retry_at_ms } = security {
            return Ok(DeviceUpdateView::RetryableFailure { next_retry_at_ms });
        }
        let relationship_update_pending = devices.iter().any(|device| {
            device.status == MemberStatus::PendingActivation
                || (device.status != MemberStatus::Removed
                    && matches!(
                        device.relation,
                        RelationView::ConfirmationPending | RelationView::Unknown
                    ))
        });
        Ok(
            if !self.effects.is_empty()
                || history_update_pending
                || relationship_update_pending
                || security == SecurityDeliveryStatus::Updating
            {
                DeviceUpdateView::Updating
            } else {
                DeviceUpdateView::Completed
            },
        )
    }
}

fn relation_view(relation: PeerRelation) -> RelationView {
    match relation {
        PeerRelation::Unconfirmed => RelationView::Unknown,
        PeerRelation::Consistent => RelationView::Consistent,
        PeerRelation::UpgradeRequired => RelationView::UpgradeRequired,
        PeerRelation::AwaitingLocalDecision => RelationView::PendingLocalDecision,
        PeerRelation::Diverged => RelationView::Diverged,
        PeerRelation::Invalid => RelationView::Invalid,
    }
}

fn relation_pause(relation: PeerRelation) -> Option<PauseReason> {
    match relation {
        PeerRelation::Consistent => None,
        PeerRelation::Unconfirmed => Some(PauseReason::RelationshipUnconfirmed),
        PeerRelation::UpgradeRequired => Some(PauseReason::UpgradeRequired),
        PeerRelation::AwaitingLocalDecision => Some(PauseReason::PendingLocalDecision),
        PeerRelation::Diverged => Some(PauseReason::Diverged),
        PeerRelation::Invalid => Some(PauseReason::Invalid),
    }
}
