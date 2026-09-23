use std::collections::{BTreeMap, BTreeSet};

use crate::ids::DeviceId;
use crate::membership::{
    BaseMembershipHistoryPosition, MemberInstanceId, MembershipEventId, MembershipOperationV2,
    RemovalDecision, VersionedMembershipHistory,
};

use super::{
    DepartingLink, LedgerDeliveryKind, LedgerDeliveryResult, LedgerEffect, LedgerFollowUp,
    LedgerInput, LedgerMemberStatus, LedgerOutcome, LedgerTransition, LedgerTransitionError,
    MemberEffectKind, MemberEffectMaterial, MemberEffectPhase, MemberLink, PeerEvidence, PeerLink,
    PeerRelation, PeerSyncBackoff, PeerSyncOutcome, PeerSyncResult, UnfinishedMemberEffect,
};

/// 一个 Space 的成员状态。字段只能经 [`MembershipLedger::apply`] 改变。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MembershipLedger {
    pub(super) revision: u64,
    pub(super) history: VersionedMembershipHistory,
    pub(super) local_device_id: DeviceId,
    pub(super) local_member: MemberInstanceId,
    pub(super) peers: BTreeMap<DeviceId, PeerLink>,
    pub(super) effects: BTreeMap<MembershipEventId, UnfinishedMemberEffect>,
    pub(super) sync_cursor: Option<DeviceId>,
}

impl MembershipLedger {
    /// 新建 Space 或加入方激活后建立成员状态。历史中的其他成员已随本机获得的历史一起核对，
    /// 因此视为一致、尚未确认本机位置。`revision` 由调用方给出，保证同一 profile 内单调递增。
    pub fn start(
        history: VersionedMembershipHistory,
        local_device_id: DeviceId,
        local_member: MemberInstanceId,
        revision: u64,
    ) -> Result<Self, LedgerTransitionError> {
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
            return Err(LedgerTransitionError::InputMismatch);
        }
        for device_id in membership.effective_peer_devices()? {
            membership.peers.insert(
                device_id,
                PeerLink::Member(MemberLink::new(
                    PeerRelation::Consistent,
                    PeerSyncBackoff::fresh(None),
                )),
            );
        }
        membership.validate()?;
        Ok(membership)
    }

    pub fn apply(
        self,
        input: LedgerInput,
        now_ms: i64,
    ) -> Result<LedgerTransition, LedgerTransitionError> {
        let mut next = self.clone();
        let follow_ups: &[LedgerFollowUp] = match &input {
            LedgerInput::LocalRemovalSigned { .. }
            | LedgerInput::LocalDecisionSigned { .. }
            | LedgerInput::AdmissionCommitted { .. }
            | LedgerInput::PeerEvidenceReconciled { .. }
            | LedgerInput::BranchRecovered { .. } => &[
                LedgerFollowUp::PublishDeviceTrustChange,
                LedgerFollowUp::WakeWorker,
            ],
            LedgerInput::HistorySyncFinished { .. }
            | LedgerInput::DeliveryFinished { .. }
            | LedgerInput::DepartureWindowElapsed { .. }
            | LedgerInput::EffectStepFinished { .. } => &[LedgerFollowUp::PublishDeviceTrustChange],
            LedgerInput::HistorySyncSelected { .. } => &[],
        };
        let outcome = match input {
            LedgerInput::LocalRemovalSigned {
                history,
                retained_device_ids,
            } => next.on_local_removal(history, retained_device_ids, now_ms)?,
            LedgerInput::LocalDecisionSigned {
                history,
                removal_event_id,
            } => next.on_local_decision(history, removal_event_id)?,
            LedgerInput::AdmissionCommitted { history } => next.on_admission(history)?,
            LedgerInput::PeerEvidenceReconciled {
                source,
                history,
                evidence,
            } => next.on_peer_evidence(source, history, evidence)?,
            LedgerInput::HistorySyncSelected { peers } => next.on_sync_selected(peers),
            LedgerInput::HistorySyncFinished {
                peer,
                synced_position,
                result,
            } => next.on_sync_finished(&peer, &synced_position, result, now_ms)?,
            LedgerInput::DeliveryFinished {
                peer,
                delivery,
                result,
            } => next.on_delivery(&peer, delivery, result),
            LedgerInput::DepartureWindowElapsed { peer } => next.on_departure_window(&peer, now_ms),
            LedgerInput::EffectStepFinished { event_id, from } => {
                next.on_effect_step(event_id, from)
            }
            LedgerInput::BranchRecovered { history } => next.on_branch_recovered(history)?,
        };
        if outcome != LedgerOutcome::Applied {
            return Ok(LedgerTransition::new(self, outcome, Vec::new()));
        }
        next.normalize()?;
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or(LedgerTransitionError::RevisionOverflow)?;
        let effects = follow_ups
            .iter()
            .copied()
            .map(LedgerEffect::AfterCommit)
            .collect();
        Ok(LedgerTransition::new(next, outcome, effects))
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
    pub fn local_status(&self) -> LedgerMemberStatus {
        if self.history.active_members().contains(&self.local_member) {
            LedgerMemberStatus::Active
        } else if self.effect_affects(&self.local_device_id) {
            LedgerMemberStatus::PendingActivation
        } else {
            LedgerMemberStatus::Removed
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
    ) -> Result<LedgerOutcome, LedgerTransitionError> {
        self.ensure_lineage(&history)?;
        let head = history
            .current_head()
            .ok_or(LedgerTransitionError::InputMismatch)?;
        if self.history.event(head).is_some() {
            return Ok(LedgerOutcome::Unchanged);
        }
        let event = history
            .event(head)
            .ok_or(LedgerTransitionError::InputMismatch)?
            .clone();
        let MembershipOperationV2::RemoveDevice { member: target } = event.operation else {
            return Err(LedgerTransitionError::InputMismatch);
        };
        if event.author_member_instance_id != self.local_member
            || event.parent_event_id != self.history.current_head()
            || target == self.local_member
            || !self.history.effective_members().contains(&target)
        {
            return Err(LedgerTransitionError::InputMismatch);
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
        Ok(LedgerOutcome::Applied)
    }

    fn on_local_decision(
        &mut self,
        history: VersionedMembershipHistory,
        removal_event_id: MembershipEventId,
    ) -> Result<LedgerOutcome, LedgerTransitionError> {
        self.ensure_lineage(&history)?;
        if self
            .history
            .decision_for(removal_event_id, self.local_member)
            .is_some()
        {
            return Ok(LedgerOutcome::Unchanged);
        }
        let decision = history
            .decision_for(removal_event_id, self.local_member)
            .ok_or(LedgerTransitionError::InputMismatch)?
            .clone();
        let removal = history
            .event(removal_event_id)
            .ok_or(LedgerTransitionError::InputMismatch)?;
        let MembershipOperationV2::RemoveDevice { member: target } = removal.operation else {
            return Err(LedgerTransitionError::InputMismatch);
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
                        PeerSyncBackoff::fresh(Some(pending_revision)),
                    ));
                }
            })
            .or_insert_with(|| {
                PeerLink::Member(MemberLink::new(
                    relation,
                    PeerSyncBackoff::fresh(Some(pending_revision)),
                ))
            });
        if let PeerLink::Member(member) = link {
            let confirmed = member.confirmed_position().cloned();
            member.record_relation(relation, confirmed);
            // 本机是移除目标时，发起方已不认可本机身份，不排队投递决定。
            member.queue_decision((target != self.local_member).then_some(decision));
        }
        Ok(LedgerOutcome::Applied)
    }

    fn on_admission(
        &mut self,
        history: VersionedMembershipHistory,
    ) -> Result<LedgerOutcome, LedgerTransitionError> {
        self.ensure_lineage(&history)?;
        let head = history
            .current_head()
            .ok_or(LedgerTransitionError::InputMismatch)?;
        if self.history.event(head).is_some() {
            return Ok(LedgerOutcome::Unchanged);
        }
        let event = history
            .event(head)
            .ok_or(LedgerTransitionError::InputMismatch)?;
        let MembershipOperationV2::AddDevice { admission } = &event.operation else {
            return Err(LedgerTransitionError::InputMismatch);
        };
        if event.parent_event_id != self.history.current_head() {
            return Err(LedgerTransitionError::InputMismatch);
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
                PeerSyncBackoff::fresh(Some(pending_revision)),
            )),
        );
        Ok(LedgerOutcome::Applied)
    }

    fn on_peer_evidence(
        &mut self,
        source: DeviceId,
        history: Option<VersionedMembershipHistory>,
        evidence: PeerEvidence,
    ) -> Result<LedgerOutcome, LedgerTransitionError> {
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
            return Err(LedgerTransitionError::InputMismatch);
        };
        match evidence {
            PeerEvidence::Confirmed => {
                let relation = if awaiting_decision {
                    PeerRelation::AwaitingLocalDecision
                } else {
                    PeerRelation::Consistent
                };
                link.record_relation(relation, Some(current));
                link.sync_mut().settle(PeerSyncOutcome::Acked);
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
            LedgerOutcome::Unchanged
        } else {
            LedgerOutcome::Applied
        })
    }

    /// 采用对端历史后，为新加入或被移除的成员登记待执行效果。
    fn adopt_remote_history(
        &mut self,
        history: VersionedMembershipHistory,
    ) -> Result<(), LedgerTransitionError> {
        let before = self.history.effective_members();
        let after = history.effective_members();
        let changed: BTreeSet<MemberInstanceId> =
            before.symmetric_difference(&after).copied().collect();
        let mut recorded = BTreeSet::new();
        let mut cursor = history.current_head();
        while let Some(event_id) = cursor {
            let event = history
                .event(event_id)
                .ok_or(LedgerTransitionError::InputMismatch)?;
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
            return Err(LedgerTransitionError::InputMismatch);
        }
        self.history = history;
        Ok(())
    }

    fn on_sync_selected(&mut self, peers: Vec<DeviceId>) -> LedgerOutcome {
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
            LedgerOutcome::Applied
        } else {
            LedgerOutcome::Unchanged
        }
    }

    fn on_sync_finished(
        &mut self,
        peer: &DeviceId,
        synced_position: &BaseMembershipHistoryPosition,
        result: PeerSyncResult,
        now_ms: i64,
    ) -> Result<LedgerOutcome, LedgerTransitionError> {
        let current = self.history.current_position()?;
        let Some(PeerLink::Member(link)) = self.peers.get_mut(peer) else {
            return Ok(LedgerOutcome::Stale);
        };
        match result {
            PeerSyncResult::Confirmed | PeerSyncResult::Diverged | PeerSyncResult::Invalid
                if *synced_position != current =>
            {
                return Ok(LedgerOutcome::Stale);
            }
            PeerSyncResult::Confirmed => {
                link.record_relation(PeerRelation::Consistent, Some(current));
                link.sync_mut().settle(PeerSyncOutcome::Acked);
            }
            PeerSyncResult::Diverged => {
                link.record_relation(PeerRelation::Diverged, None);
                link.sync_mut().settle(PeerSyncOutcome::StableRejected);
            }
            PeerSyncResult::Invalid => {
                link.record_relation(PeerRelation::Invalid, None);
                link.sync_mut().settle(PeerSyncOutcome::StableRejected);
            }
            PeerSyncResult::Deferred => link.sync_mut().defer(now_ms)?,
            PeerSyncResult::Rejected => {
                link.sync_mut().settle(PeerSyncOutcome::StableRejected);
            }
        }
        Ok(LedgerOutcome::Applied)
    }

    fn on_delivery(
        &mut self,
        peer: &DeviceId,
        delivery: LedgerDeliveryKind,
        result: LedgerDeliveryResult,
    ) -> LedgerOutcome {
        match (delivery, self.peers.get_mut(peer)) {
            (LedgerDeliveryKind::RemovalNotice, Some(PeerLink::Departing(_))) => match result {
                LedgerDeliveryResult::Delivered => {
                    self.peers.remove(peer);
                    LedgerOutcome::Applied
                }
                // 通知责任由离开窗口兜底结束。
                LedgerDeliveryResult::Deferred | LedgerDeliveryResult::Rejected => {
                    LedgerOutcome::Unchanged
                }
            },
            (LedgerDeliveryKind::Decision, Some(PeerLink::Member(link)))
                if link.outgoing_decision().is_some() =>
            {
                match result {
                    // 对端明确拒绝时本机已无法再推进这项投递。
                    LedgerDeliveryResult::Delivered | LedgerDeliveryResult::Rejected => {
                        link.queue_decision(None);
                        LedgerOutcome::Applied
                    }
                    LedgerDeliveryResult::Deferred => LedgerOutcome::Unchanged,
                }
            }
            _ => LedgerOutcome::Stale,
        }
    }

    fn on_departure_window(&mut self, peer: &DeviceId, now_ms: i64) -> LedgerOutcome {
        match self.peers.get(peer) {
            Some(PeerLink::Departing(link)) if now_ms >= link.expires_at_ms() => {
                self.peers.remove(peer);
                LedgerOutcome::Applied
            }
            Some(PeerLink::Departing(_)) => LedgerOutcome::Unchanged,
            _ => LedgerOutcome::Stale,
        }
    }

    fn on_effect_step(
        &mut self,
        event_id: MembershipEventId,
        from: MemberEffectPhase,
    ) -> LedgerOutcome {
        let Some(effect) = self.effects.get_mut(&event_id) else {
            return LedgerOutcome::Stale;
        };
        if effect.phase() != from {
            return LedgerOutcome::Stale;
        }
        match from.next() {
            Some(next) => effect.advance(next),
            None => {
                self.effects.remove(&event_id);
            }
        }
        LedgerOutcome::Applied
    }

    fn on_branch_recovered(
        &mut self,
        history: VersionedMembershipHistory,
    ) -> Result<LedgerOutcome, LedgerTransitionError> {
        self.ensure_lineage(&history)?;
        if !history.active_members().contains(&self.local_member)
            || device_of(&history, self.local_member)? != self.local_device_id
        {
            return Err(LedgerTransitionError::InputMismatch);
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
                        PeerSyncBackoff::fresh(None),
                    )),
                )
            })
            .collect();
        Ok(LedgerOutcome::Applied)
    }

    fn ensure_lineage(
        &self,
        history: &VersionedMembershipHistory,
    ) -> Result<(), LedgerTransitionError> {
        if history.lineage_id() == self.history.lineage_id() {
            Ok(())
        } else {
            Err(LedgerTransitionError::LineageMismatch)
        }
    }

    fn pending_revision(&self) -> u64 {
        self.revision.saturating_add(1)
    }

    /// 当前历史中除本机外的成员设备。
    pub(super) fn effective_peer_devices(
        &self,
    ) -> Result<BTreeSet<DeviceId>, LedgerTransitionError> {
        self.history
            .effective_members()
            .into_iter()
            .filter(|member| *member != self.local_member)
            .map(|member| device_of(&self.history, member))
            .collect()
    }

    /// 当前历史中除本机外的已激活成员设备。
    pub(super) fn active_peer_devices(&self) -> Result<BTreeSet<DeviceId>, LedgerTransitionError> {
        self.history
            .active_members()
            .into_iter()
            .filter(|member| *member != self.local_member)
            .map(|member| device_of(&self.history, member))
            .collect()
    }

    /// 让对端记录与历史保持一致：每个有效对端恰有一个 `Member`，离开中的设备不在历史成员中，
    /// 其余记录删除；本机为目标的移除决定不投递；效果只保留当前历史路径上的事件。
    fn normalize(&mut self) -> Result<(), LedgerTransitionError> {
        let path = current_path(&self.history);
        self.effects.retain(|event_id, _| path.contains(event_id));
        let pending_revision = self.pending_revision();
        let mut previous = std::mem::take(&mut self.peers);
        for device in self.effective_peer_devices()? {
            let link = match previous.remove(&device) {
                Some(PeerLink::Member(link)) => link,
                Some(PeerLink::Departing(_)) | None => MemberLink::new(
                    PeerRelation::Unconfirmed,
                    PeerSyncBackoff::fresh(Some(pending_revision)),
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
    pub(super) fn validate(&self) -> Result<(), LedgerTransitionError> {
        if device_of(&self.history, self.local_member)
            .map_err(|_| LedgerTransitionError::InvalidSnapshot)?
            != self.local_device_id
            || self.peers.contains_key(&self.local_device_id)
        {
            return Err(LedgerTransitionError::InvalidSnapshot);
        }
        let effective = self
            .effective_peer_devices()
            .map_err(|_| LedgerTransitionError::InvalidSnapshot)?;
        for device in &effective {
            if !matches!(self.peers.get(device), Some(PeerLink::Member(_))) {
                return Err(LedgerTransitionError::InvalidSnapshot);
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
                return Err(LedgerTransitionError::InvalidSnapshot);
            }
        }
        let path = current_path(&self.history);
        if self
            .effects
            .iter()
            .any(|(event_id, effect)| !path.contains(event_id) || effect.event_id() != *event_id)
        {
            return Err(LedgerTransitionError::InvalidSnapshot);
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
) -> Result<DeviceId, LedgerTransitionError> {
    history
        .admission_facts_for(member)
        .map(|facts| facts.device_id)
        .ok_or(LedgerTransitionError::InputMismatch)
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
