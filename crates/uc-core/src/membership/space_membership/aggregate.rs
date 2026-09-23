use std::collections::{BTreeMap, BTreeSet};

use crate::ids::DeviceId;
use crate::membership::{
    BaseMembershipHistoryPosition, MemberInstanceId, MembershipEventId, MembershipOperationV2,
    RemovalDecision, VersionedMembershipHistory,
};

use super::{
    DeliveryKind, DeliveryResult, DepartingLink, HistorySyncOutcome, HistorySyncResult,
    MemberEffectKind, MemberEffectMaterial, MemberEffectPhase, MemberLink, MemberStatus,
    MembershipEffect, MembershipFollowUp, MembershipInput, MembershipOutcome, MembershipTransition,
    PeerEvidence, PeerLink, PeerRelation, SpaceMembershipError, SyncBackoff,
    UnfinishedMemberEffect,
};

/// 一个 Space 的成员状态。字段只能经 [`SpaceMembership::apply`] 改变。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceMembership {
    pub(super) revision: u64,
    pub(super) history: VersionedMembershipHistory,
    pub(super) local_device_id: DeviceId,
    pub(super) local_member: MemberInstanceId,
    pub(super) peers: BTreeMap<DeviceId, PeerLink>,
    pub(super) effects: BTreeMap<MembershipEventId, UnfinishedMemberEffect>,
    pub(super) sync_cursor: Option<DeviceId>,
}

impl SpaceMembership {
    /// 新建 Space 或加入方激活后建立成员状态。历史中的其他成员已随本机获得的历史一起核对，
    /// 因此视为一致、尚未确认本机位置。`revision` 由调用方给出，保证同一 profile 内单调递增。
    pub fn start(
        history: VersionedMembershipHistory,
        local_device_id: DeviceId,
        local_member: MemberInstanceId,
        revision: u64,
    ) -> Result<Self, SpaceMembershipError> {
        let mut membership = Self {
            revision,
            history,
            local_device_id,
            local_member,
            peers: BTreeMap::new(),
            effects: BTreeMap::new(),
            sync_cursor: None,
        };
        if !membership
            .history
            .effective_members()
            .contains(&membership.local_member)
        {
            return Err(SpaceMembershipError::InputMismatch);
        }
        for device_id in membership.effective_peer_devices()? {
            membership.peers.insert(
                device_id,
                PeerLink::Member(MemberLink::new(
                    PeerRelation::Consistent,
                    SyncBackoff::fresh(None),
                )),
            );
        }
        membership.validate()?;
        Ok(membership)
    }

    pub fn apply(
        self,
        input: MembershipInput,
        now_ms: i64,
    ) -> Result<MembershipTransition, SpaceMembershipError> {
        let mut next = self.clone();
        let follow_ups: &[MembershipFollowUp] = match &input {
            MembershipInput::LocalRemovalSigned { .. }
            | MembershipInput::LocalDecisionSigned { .. }
            | MembershipInput::AdmissionCommitted { .. }
            | MembershipInput::PeerEvidenceReconciled { .. }
            | MembershipInput::BranchRecovered { .. } => &[
                MembershipFollowUp::PublishDeviceTrustChange,
                MembershipFollowUp::WakeWorker,
            ],
            MembershipInput::HistorySyncFinished { .. }
            | MembershipInput::DeliveryFinished { .. }
            | MembershipInput::DepartureWindowElapsed { .. }
            | MembershipInput::EffectStepFinished { .. } => {
                &[MembershipFollowUp::PublishDeviceTrustChange]
            }
            MembershipInput::HistorySyncSelected { .. } => &[],
        };
        let outcome = match input {
            MembershipInput::LocalRemovalSigned {
                history,
                retained_device_ids,
            } => next.on_local_removal(history, retained_device_ids, now_ms)?,
            MembershipInput::LocalDecisionSigned {
                history,
                removal_event_id,
            } => next.on_local_decision(history, removal_event_id)?,
            MembershipInput::AdmissionCommitted { history } => next.on_admission(history)?,
            MembershipInput::PeerEvidenceReconciled {
                source,
                history,
                evidence,
            } => next.on_peer_evidence(source, history, evidence)?,
            MembershipInput::HistorySyncSelected { peers } => next.on_sync_selected(peers),
            MembershipInput::HistorySyncFinished {
                peer,
                synced_position,
                result,
            } => next.on_sync_finished(&peer, &synced_position, result, now_ms)?,
            MembershipInput::DeliveryFinished {
                peer,
                delivery,
                result,
            } => next.on_delivery(&peer, delivery, result),
            MembershipInput::DepartureWindowElapsed { peer } => {
                next.on_departure_window(&peer, now_ms)
            }
            MembershipInput::EffectStepFinished { event_id, from } => {
                next.on_effect_step(event_id, from)
            }
            MembershipInput::BranchRecovered { history } => next.on_branch_recovered(history)?,
        };
        if outcome != MembershipOutcome::Applied {
            return Ok(MembershipTransition::new(self, outcome, Vec::new()));
        }
        next.normalize()?;
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or(SpaceMembershipError::RevisionOverflow)?;
        let effects = follow_ups
            .iter()
            .copied()
            .map(MembershipEffect::AfterCommit)
            .collect();
        Ok(MembershipTransition::new(next, outcome, effects))
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn history(&self) -> &VersionedMembershipHistory {
        &self.history
    }

    pub fn local_device_id(&self) -> &DeviceId {
        &self.local_device_id
    }

    pub fn local_member(&self) -> MemberInstanceId {
        self.local_member
    }

    pub fn peer(&self, device_id: &DeviceId) -> Option<&PeerLink> {
        self.peers.get(device_id)
    }

    pub fn peers(&self) -> impl Iterator<Item = (&DeviceId, &PeerLink)> {
        self.peers.iter()
    }

    pub fn unfinished_effects(&self) -> impl Iterator<Item = &UnfinishedMemberEffect> {
        self.effects.values()
    }

    /// 本机状态：在有效成员中为有效；否则有影响本机的未完成效果时为激活中；其余为已移除。
    pub fn local_status(&self) -> MemberStatus {
        if self.history.active_members().contains(&self.local_member) {
            MemberStatus::Active
        } else if self.effect_affects(&self.local_device_id) {
            MemberStatus::PendingActivation
        } else {
            MemberStatus::Removed
        }
    }

    pub(super) fn effect_affects(&self, device_id: &DeviceId) -> bool {
        self.effects
            .values()
            .any(|effect| effect.affects(device_id))
    }

    fn on_local_removal(
        &mut self,
        history: VersionedMembershipHistory,
        retained_device_ids: Vec<DeviceId>,
        now_ms: i64,
    ) -> Result<MembershipOutcome, SpaceMembershipError> {
        self.ensure_lineage(&history)?;
        let head = history
            .current_head()
            .ok_or(SpaceMembershipError::InputMismatch)?;
        if self.history.event(head).is_some() {
            return Ok(MembershipOutcome::Unchanged);
        }
        let event = history
            .event(head)
            .ok_or(SpaceMembershipError::InputMismatch)?
            .clone();
        let MembershipOperationV2::RemoveDevice { member: target } = event.operation else {
            return Err(SpaceMembershipError::InputMismatch);
        };
        if event.author_member_instance_id != self.local_member
            || event.parent_event_id != self.history.current_head()
            || target == self.local_member
            || !self.history.effective_members().contains(&target)
        {
            return Err(SpaceMembershipError::InputMismatch);
        }
        let target_device = device_of(&history, target)?;
        self.history = history;
        self.peers.insert(
            target_device,
            PeerLink::Departing(DepartingLink::new(event.clone(), now_ms)),
        );
        self.effects.insert(
            head,
            UnfinishedMemberEffect::prepared(
                head,
                MemberEffectKind::RemoveDevice,
                vec![target_device],
                MemberEffectMaterial::InitiatedRemoval {
                    event,
                    retained_device_ids,
                },
            ),
        );
        Ok(MembershipOutcome::Applied)
    }

    fn on_local_decision(
        &mut self,
        history: VersionedMembershipHistory,
        removal_event_id: MembershipEventId,
    ) -> Result<MembershipOutcome, SpaceMembershipError> {
        self.ensure_lineage(&history)?;
        if self
            .history
            .decision_for(removal_event_id, self.local_member)
            .is_some()
        {
            return Ok(MembershipOutcome::Unchanged);
        }
        let decision = history
            .decision_for(removal_event_id, self.local_member)
            .ok_or(SpaceMembershipError::InputMismatch)?
            .clone();
        let removal = history
            .event(removal_event_id)
            .ok_or(SpaceMembershipError::InputMismatch)?;
        let MembershipOperationV2::RemoveDevice { member: target } = removal.operation else {
            return Err(SpaceMembershipError::InputMismatch);
        };
        let proposer = device_of(&history, removal.author_member_instance_id)?;
        let target_device = device_of(&history, target)?;
        let relation = match decision.decision {
            RemovalDecision::Accept => PeerRelation::Consistent,
            RemovalDecision::Reject => PeerRelation::Diverged,
        };
        if decision.decision == RemovalDecision::Accept {
            self.effects.insert(
                removal_event_id,
                UnfinishedMemberEffect::prepared(
                    removal_event_id,
                    MemberEffectKind::RemoveDevice,
                    vec![target_device],
                    MemberEffectMaterial::Decision(decision.clone()),
                ),
            );
        }
        self.history = history;
        let pending_revision = self.pending_revision();
        let link = self
            .peers
            .entry(proposer)
            .and_modify(|link| {
                if let PeerLink::Departing(_) = link {
                    *link = PeerLink::Member(MemberLink::new(
                        relation,
                        SyncBackoff::fresh(Some(pending_revision)),
                    ));
                }
            })
            .or_insert_with(|| {
                PeerLink::Member(MemberLink::new(
                    relation,
                    SyncBackoff::fresh(Some(pending_revision)),
                ))
            });
        if let PeerLink::Member(member) = link {
            let confirmed = member.confirmed_position().cloned();
            member.record_relation(relation, confirmed);
            // 本机是移除目标时，发起方已不认可本机身份，不排队投递决定。
            member.queue_decision((target != self.local_member).then_some(decision));
        }
        Ok(MembershipOutcome::Applied)
    }

    fn on_admission(
        &mut self,
        history: VersionedMembershipHistory,
    ) -> Result<MembershipOutcome, SpaceMembershipError> {
        self.ensure_lineage(&history)?;
        let head = history
            .current_head()
            .ok_or(SpaceMembershipError::InputMismatch)?;
        if self.history.event(head).is_some() {
            return Ok(MembershipOutcome::Unchanged);
        }
        let event = history
            .event(head)
            .ok_or(SpaceMembershipError::InputMismatch)?;
        let MembershipOperationV2::AddDevice { admission } = &event.operation else {
            return Err(SpaceMembershipError::InputMismatch);
        };
        if event.parent_event_id != self.history.current_head() {
            return Err(SpaceMembershipError::InputMismatch);
        }
        let admitted = admission.facts.device_id;
        let pending_revision = self.pending_revision();
        self.history = history;
        // 新的正式头产生后，旧确认只证明旧位置，逐个对端重新取得确认。
        for link in self.peers.values_mut() {
            if let PeerLink::Member(member) = link {
                member.forget_confirmation();
            }
        }
        self.peers.insert(
            admitted,
            PeerLink::Member(MemberLink::new(
                PeerRelation::Consistent,
                SyncBackoff::fresh(Some(pending_revision)),
            )),
        );
        Ok(MembershipOutcome::Applied)
    }

    fn on_peer_evidence(
        &mut self,
        source: DeviceId,
        history: Option<VersionedMembershipHistory>,
        evidence: PeerEvidence,
    ) -> Result<MembershipOutcome, SpaceMembershipError> {
        let before = self.clone();
        if let Some(history) = history {
            self.ensure_lineage(&history)?;
            if history != self.history {
                self.adopt_remote_history(history)?;
                self.normalize()?;
            }
        }
        let current = self.history.current_position()?;
        let awaiting_decision = self
            .history
            .pending_removal_decision(self.local_member)
            .is_some();
        let Some(PeerLink::Member(link)) = self.peers.get_mut(&source) else {
            return Err(SpaceMembershipError::InputMismatch);
        };
        match evidence {
            PeerEvidence::Confirmed => {
                let relation = if awaiting_decision {
                    PeerRelation::AwaitingLocalDecision
                } else {
                    PeerRelation::Consistent
                };
                link.record_relation(relation, Some(current));
                link.sync_mut().settle(HistorySyncOutcome::Acked);
            }
            PeerEvidence::Diverged | PeerEvidence::Invalid => {
                let relation = if evidence == PeerEvidence::Diverged {
                    PeerRelation::Diverged
                } else {
                    PeerRelation::Invalid
                };
                let confirmed = link.confirmed_position().cloned();
                link.record_relation(relation, confirmed);
            }
            PeerEvidence::NeedsEvidence => {}
        }
        Ok(if *self == before {
            MembershipOutcome::Unchanged
        } else {
            MembershipOutcome::Applied
        })
    }

    /// 采用对端历史后，为新加入或被移除的成员登记待执行效果。
    fn adopt_remote_history(
        &mut self,
        history: VersionedMembershipHistory,
    ) -> Result<(), SpaceMembershipError> {
        let before = self.history.effective_members();
        let after = history.effective_members();
        let changed: BTreeSet<MemberInstanceId> =
            before.symmetric_difference(&after).copied().collect();
        let mut recorded = BTreeSet::new();
        let mut cursor = history.current_head();
        while let Some(event_id) = cursor {
            let event = history
                .event(event_id)
                .ok_or(SpaceMembershipError::InputMismatch)?;
            let (kind, member) = match &event.operation {
                MembershipOperationV2::AddDevice { admission } => {
                    (MemberEffectKind::AddDevice, admission.facts.member_instance)
                }
                MembershipOperationV2::RemoveDevice { member } => {
                    (MemberEffectKind::RemoveDevice, *member)
                }
            };
            if changed.contains(&member) && recorded.insert(member) {
                let device = device_of(&history, member)?;
                self.effects.entry(event_id).or_insert_with(|| {
                    UnfinishedMemberEffect::prepared(
                        event_id,
                        kind,
                        vec![device],
                        MemberEffectMaterial::Event(event.clone()),
                    )
                });
            }
            cursor = event.parent_event_id;
        }
        if recorded != changed {
            return Err(SpaceMembershipError::InputMismatch);
        }
        self.history = history;
        Ok(())
    }

    fn on_sync_selected(&mut self, peers: Vec<DeviceId>) -> MembershipOutcome {
        let pending_revision = self.pending_revision();
        let mut changed = false;
        for peer in &peers {
            if let Some(PeerLink::Member(link)) = self.peers.get_mut(peer) {
                if link.sync().pending_since_revision().is_none() {
                    link.sync_mut().mark_pending(pending_revision);
                    changed = true;
                }
            }
        }
        if let Some(last) = peers.last() {
            if self.sync_cursor.as_ref() != Some(last) {
                self.sync_cursor = Some(*last);
                changed = true;
            }
        }
        if changed {
            MembershipOutcome::Applied
        } else {
            MembershipOutcome::Unchanged
        }
    }

    fn on_sync_finished(
        &mut self,
        peer: &DeviceId,
        synced_position: &BaseMembershipHistoryPosition,
        result: HistorySyncResult,
        now_ms: i64,
    ) -> Result<MembershipOutcome, SpaceMembershipError> {
        let current = self.history.current_position()?;
        let Some(PeerLink::Member(link)) = self.peers.get_mut(peer) else {
            return Ok(MembershipOutcome::Stale);
        };
        match result {
            HistorySyncResult::Confirmed
            | HistorySyncResult::Diverged
            | HistorySyncResult::Invalid
                if *synced_position != current =>
            {
                return Ok(MembershipOutcome::Stale);
            }
            HistorySyncResult::Confirmed => {
                link.record_relation(PeerRelation::Consistent, Some(current));
                link.sync_mut().settle(HistorySyncOutcome::Acked);
            }
            HistorySyncResult::Diverged => {
                link.record_relation(PeerRelation::Diverged, None);
                link.sync_mut().settle(HistorySyncOutcome::StableRejected);
            }
            HistorySyncResult::Invalid => {
                link.record_relation(PeerRelation::Invalid, None);
                link.sync_mut().settle(HistorySyncOutcome::StableRejected);
            }
            HistorySyncResult::Deferred => link.sync_mut().defer(now_ms)?,
            HistorySyncResult::Rejected => {
                link.sync_mut().settle(HistorySyncOutcome::StableRejected);
            }
        }
        Ok(MembershipOutcome::Applied)
    }

    fn on_delivery(
        &mut self,
        peer: &DeviceId,
        delivery: DeliveryKind,
        result: DeliveryResult,
    ) -> MembershipOutcome {
        match (delivery, self.peers.get_mut(peer)) {
            (DeliveryKind::RemovalNotice, Some(PeerLink::Departing(_))) => match result {
                DeliveryResult::Delivered => {
                    self.peers.remove(peer);
                    MembershipOutcome::Applied
                }
                // 通知责任由离开窗口兜底结束。
                DeliveryResult::Deferred | DeliveryResult::Rejected => MembershipOutcome::Unchanged,
            },
            (DeliveryKind::Decision, Some(PeerLink::Member(link)))
                if link.outgoing_decision().is_some() =>
            {
                match result {
                    // 对端明确拒绝时本机已无法再推进这项投递。
                    DeliveryResult::Delivered | DeliveryResult::Rejected => {
                        link.queue_decision(None);
                        MembershipOutcome::Applied
                    }
                    DeliveryResult::Deferred => MembershipOutcome::Unchanged,
                }
            }
            _ => MembershipOutcome::Stale,
        }
    }

    fn on_departure_window(&mut self, peer: &DeviceId, now_ms: i64) -> MembershipOutcome {
        match self.peers.get(peer) {
            Some(PeerLink::Departing(link)) if now_ms >= link.expires_at_ms() => {
                self.peers.remove(peer);
                MembershipOutcome::Applied
            }
            Some(PeerLink::Departing(_)) => MembershipOutcome::Unchanged,
            _ => MembershipOutcome::Stale,
        }
    }

    fn on_effect_step(
        &mut self,
        event_id: MembershipEventId,
        from: MemberEffectPhase,
    ) -> MembershipOutcome {
        let Some(effect) = self.effects.get_mut(&event_id) else {
            return MembershipOutcome::Stale;
        };
        if effect.phase() != from {
            return MembershipOutcome::Stale;
        }
        match from.next() {
            Some(next) => effect.advance(next),
            None => {
                self.effects.remove(&event_id);
            }
        }
        MembershipOutcome::Applied
    }

    fn on_branch_recovered(
        &mut self,
        history: VersionedMembershipHistory,
    ) -> Result<MembershipOutcome, SpaceMembershipError> {
        self.ensure_lineage(&history)?;
        if !history.active_members().contains(&self.local_member)
            || device_of(&history, self.local_member)? != self.local_device_id
        {
            return Err(SpaceMembershipError::InputMismatch);
        }
        self.history = history;
        // 目标分支的效果已由分支恢复安装，旧分支的传输与调度状态不跨分支继承。
        self.effects.clear();
        self.sync_cursor = None;
        self.peers = self
            .effective_peer_devices()?
            .into_iter()
            .map(|device| {
                (
                    device,
                    PeerLink::Member(MemberLink::new(
                        PeerRelation::Consistent,
                        SyncBackoff::fresh(None),
                    )),
                )
            })
            .collect();
        Ok(MembershipOutcome::Applied)
    }

    fn ensure_lineage(
        &self,
        history: &VersionedMembershipHistory,
    ) -> Result<(), SpaceMembershipError> {
        if history.lineage_id() == self.history.lineage_id() {
            Ok(())
        } else {
            Err(SpaceMembershipError::LineageMismatch)
        }
    }

    fn pending_revision(&self) -> u64 {
        self.revision.saturating_add(1)
    }

    /// 当前历史中除本机外的成员设备。
    pub(super) fn effective_peer_devices(
        &self,
    ) -> Result<BTreeSet<DeviceId>, SpaceMembershipError> {
        self.history
            .effective_members()
            .into_iter()
            .filter(|member| *member != self.local_member)
            .map(|member| device_of(&self.history, member))
            .collect()
    }

    /// 当前历史中除本机外的已激活成员设备。
    pub(super) fn active_peer_devices(&self) -> Result<BTreeSet<DeviceId>, SpaceMembershipError> {
        self.history
            .active_members()
            .into_iter()
            .filter(|member| *member != self.local_member)
            .map(|member| device_of(&self.history, member))
            .collect()
    }

    /// 让对端记录与历史保持一致：每个有效对端恰有一个 `Member`，离开中的设备不在历史成员中，
    /// 其余记录删除；本机为目标的移除决定不投递；效果只保留当前历史路径上的事件。
    fn normalize(&mut self) -> Result<(), SpaceMembershipError> {
        let path = current_path(&self.history);
        self.effects.retain(|event_id, _| path.contains(event_id));
        let pending_revision = self.pending_revision();
        let mut previous = std::mem::take(&mut self.peers);
        for device in self.effective_peer_devices()? {
            let link = match previous.remove(&device) {
                Some(PeerLink::Member(link)) => link,
                Some(PeerLink::Departing(_)) | None => MemberLink::new(
                    PeerRelation::Unconfirmed,
                    SyncBackoff::fresh(Some(pending_revision)),
                ),
            };
            self.peers.insert(device, PeerLink::Member(link));
        }
        for (device, link) in previous {
            if let PeerLink::Departing(departing) = link {
                self.peers.insert(device, PeerLink::Departing(departing));
            }
        }
        let history = &self.history;
        let local_member = self.local_member;
        for link in self.peers.values_mut() {
            if let PeerLink::Member(member) = link {
                let targets_local = member.outgoing_decision().is_some_and(|decision| {
                    removal_targets(history, decision.removal_event_id, local_member)
                });
                if targets_local {
                    member.queue_decision(None);
                }
            }
        }
        Ok(())
    }

    /// 校验全部不变量；`restore` 与 `start` 使用，拒绝任何需要规范化才能成立的状态。
    pub(super) fn validate(&self) -> Result<(), SpaceMembershipError> {
        if device_of(&self.history, self.local_member)
            .map_err(|_| SpaceMembershipError::InvalidSnapshot)?
            != self.local_device_id
            || self.peers.contains_key(&self.local_device_id)
        {
            return Err(SpaceMembershipError::InvalidSnapshot);
        }
        let effective = self
            .effective_peer_devices()
            .map_err(|_| SpaceMembershipError::InvalidSnapshot)?;
        for device in &effective {
            if !matches!(self.peers.get(device), Some(PeerLink::Member(_))) {
                return Err(SpaceMembershipError::InvalidSnapshot);
            }
        }
        for (device, link) in &self.peers {
            let valid = match link {
                PeerLink::Member(member) => {
                    effective.contains(device)
                        && !member.outgoing_decision().is_some_and(|decision| {
                            removal_targets(
                                &self.history,
                                decision.removal_event_id,
                                self.local_member,
                            )
                        })
                }
                PeerLink::Departing(_) => !effective.contains(device),
            };
            if !valid {
                return Err(SpaceMembershipError::InvalidSnapshot);
            }
        }
        let path = current_path(&self.history);
        if self
            .effects
            .iter()
            .any(|(event_id, effect)| !path.contains(event_id) || effect.event_id() != *event_id)
        {
            return Err(SpaceMembershipError::InvalidSnapshot);
        }
        Ok(())
    }
}

/// 从当前头沿父事件回溯到创世的事件集合。
pub(super) fn current_path(history: &VersionedMembershipHistory) -> BTreeSet<MembershipEventId> {
    let mut path = BTreeSet::new();
    let mut cursor = history.current_head();
    while let Some(event_id) = cursor {
        if !path.insert(event_id) {
            break;
        }
        cursor = history
            .event(event_id)
            .and_then(|event| event.parent_event_id);
    }
    path
}

pub(super) fn device_of(
    history: &VersionedMembershipHistory,
    member: MemberInstanceId,
) -> Result<DeviceId, SpaceMembershipError> {
    history
        .admission_facts_for(member)
        .map(|facts| facts.device_id)
        .ok_or(SpaceMembershipError::InputMismatch)
}

fn removal_targets(
    history: &VersionedMembershipHistory,
    removal_event_id: MembershipEventId,
    member: MemberInstanceId,
) -> bool {
    history.event(removal_event_id).is_some_and(|event| {
        matches!(
            event.operation,
            MembershipOperationV2::RemoveDevice { member: target } if target == member
        )
    })
}
