use super::{
    LoadedMembershipLedger, MembershipEffectPhase, MembershipLedgerError, PeerReconciliationRecord,
    PendingMembershipEffect,
};
use std::collections::BTreeSet;
use uc_core::membership::{
    MemberInstanceId, MembershipHistoryRelationship, VersionedMembershipHistory,
};

fn current_event_ids(history: &VersionedMembershipHistory) -> BTreeSet<[u8; 32]> {
    let mut ids = BTreeSet::new();
    let mut next = history.current_head();
    while let Some(event_id) = next {
        if !ids.insert(*event_id.as_bytes()) {
            break;
        }
        next = history
            .event(event_id)
            .and_then(|event| event.parent_event_id);
    }
    ids
}

impl LoadedMembershipLedger {
    /// 历史效果保留为日志；只有当前选中分支中的事件具有执行资格。
    pub(crate) fn current_effects<'a>(
        &'a self,
        history: &VersionedMembershipHistory,
    ) -> Vec<(&'a [u8; 32], &'a PendingMembershipEffect)> {
        let ids = current_event_ids(history);
        self.effect_journal
            .iter()
            .filter(|(id, _)| ids.contains(*id))
            .collect()
    }

    /// 完整目标资料已准备时，形成新分支的唯一运行记录。保留审计和防重放事实，
    /// 旧传输、游标和执行资格不跨分支继承。
    pub fn recovered_branch(
        &self,
        history: &VersionedMembershipHistory,
        local_member: MemberInstanceId,
    ) -> Result<Self, MembershipLedgerError> {
        let local = history
            .admission_facts_for(local_member)
            .ok_or(MembershipLedgerError::Corrupt)?;
        if self.lineage_id.as_deref() != Some(history.lineage_id())
            || !history.active_members().contains(&local_member)
        {
            return Err(MembershipLedgerError::Corrupt);
        }
        let mut replacement = self.clone();
        replacement.membership_history = Some(
            history
                .encode_persisted_v2()
                .map_err(|_| MembershipLedgerError::Corrupt)?,
        );
        replacement.local_device_id = Some(local.device_id.clone());
        replacement.local_member_instance = Some(local_member);
        replacement.local_join_active = true;
        replacement.history_sync_cursor = None;
        replacement.inbound_transfers.clear();
        replacement.completed_inbound_transfers.clear();
        replacement.peer_reconciliation = history
            .active_members()
            .into_iter()
            .filter(|member| member != &local_member)
            .map(|member| {
                let facts = history
                    .admission_facts_for(member)
                    .ok_or(MembershipLedgerError::Corrupt)?;
                Ok((
                    facts.device_id.clone(),
                    PeerReconciliationRecord {
                        peer_device_id: facts.device_id.clone(),
                        relationship: MembershipHistoryRelationship::Consistent,
                        confirmed_position: None,
                        sync_state: Default::default(),
                        restricted_delivery: Vec::new(),
                        updated_at_ms: 0,
                    },
                ))
            })
            .collect::<Result<_, MembershipLedgerError>>()?;
        let installed = current_event_ids(history);
        for (id, effect) in &mut replacement.effect_journal {
            if installed.contains(id) {
                effect.phase = MembershipEffectPhase::Activated;
            }
        }
        Ok(replacement)
    }
}
