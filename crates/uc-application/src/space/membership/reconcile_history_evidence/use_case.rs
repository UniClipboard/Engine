use std::sync::Arc;

use uc_core::membership::{
    MembershipConflictEvidenceV3, MembershipConflictPolicy, MembershipHistoryRelationship,
};

use super::MembershipEvidenceExchange;
use crate::space::membership::{
    MembershipBranchRecoverySession, MembershipConflictPresentation, MembershipConflictRecord,
    MembershipConflictStatus, MembershipLedger, MembershipLedgerError,
};

pub(crate) struct ReconcileMembershipEvidenceUseCase {
    ledger: Arc<MembershipLedger>,
}

impl ReconcileMembershipEvidenceUseCase {
    pub(crate) fn new(ledger: Arc<MembershipLedger>) -> Self {
        Self { ledger }
    }

    pub(crate) async fn execute(
        &self,
        source_device_id: &uc_core::ids::DeviceId,
        evidence: &MembershipConflictEvidenceV3,
    ) -> Result<Option<MembershipEvidenceExchange>, MembershipLedgerError> {
        let snapshot = self.ledger.load_verified().await?;
        let local = snapshot
            .history()
            .ok_or(MembershipLedgerError::RecoveryRequired)?;
        let Ok(remote) = self.ledger.verify_exchange_pages(&evidence.pages) else {
            return Ok(None);
        };
        let Ok(remote_position) = remote.current_position() else {
            return Ok(None);
        };
        if evidence.transfer_id != remote_position.history_digest
            || remote
                .effective_member_for_device(source_device_id)
                .and_then(|member| remote.admission_facts_for(member))
                .is_none_or(|facts| &facts.device_id != source_device_id)
        {
            return Ok(None);
        }
        let local_member = snapshot
            .record()
            .local_member_instance
            .ok_or(MembershipLedgerError::RecoveryRequired)?;
        let local_sender = local
            .admission_facts_for(local_member)
            .cloned()
            .ok_or(MembershipLedgerError::RecoveryRequired)?;
        let Ok(response_pages) = local.export_conflict_evidence_pages_v2(local_sender) else {
            return Err(MembershipLedgerError::Corrupt);
        };
        let response_position = local
            .current_position()
            .map_err(|_| MembershipLedgerError::Corrupt)?;
        let response = |relationship| {
            Some(MembershipEvidenceExchange {
                response: MembershipConflictEvidenceV3 {
                    transfer_id: response_position.history_digest,
                    pages: response_pages,
                },
                relationship,
            })
        };
        if MembershipConflictPolicy::branch_id(local).map_err(|_| MembershipLedgerError::Corrupt)?
            == MembershipConflictPolicy::branch_id(&remote)
                .map_err(|_| MembershipLedgerError::Corrupt)?
            && local.active_members() == remote.active_members()
        {
            if !snapshot
                .record()
                .peer_reconciliation
                .get(source_device_id)
                .is_some_and(|peer| peer.relationship == MembershipHistoryRelationship::Consistent)
            {
                self.ledger
                    .compare_and_commit(|record| {
                        if record.revision != snapshot.record().revision {
                            return Err(MembershipLedgerError::Conflict);
                        }
                        let peer = record
                            .peer_reconciliation
                            .get_mut(source_device_id)
                            .ok_or(MembershipLedgerError::RecoveryRequired)?;
                        peer.relationship = MembershipHistoryRelationship::Consistent;
                        // 只确认已应用分支一致，不能伪造完整已知历史的 ACK 水位。
                        peer.confirmed_position = None;
                        Ok(())
                    })
                    .await?;
            }
            return Ok(response(MembershipHistoryRelationship::Consistent));
        }
        let Ok(conflict) = MembershipConflictPolicy::describe(local, &remote, local_member) else {
            return Ok(None);
        };
        let local_choice = conflict
            .choice_for(conflict.local_branch_id)
            .ok_or(MembershipLedgerError::Corrupt)?;
        let remote_choice = conflict
            .choice_for(conflict.remote_branch_id)
            .ok_or(MembershipLedgerError::Corrupt)?;
        let presentation =
            MembershipConflictPresentation::from_verified_histories(local, &remote, local_member)?;
        let legacy =
            MembershipConflictPolicy::legacy_description(local, &remote, local_member).ok();
        let keep_local =
            MembershipConflictPolicy::local_choice_already_recorded(local, &remote, local_member)
                || legacy
                    .as_ref()
                    .and_then(|legacy| {
                        snapshot
                            .record()
                            .membership_conflicts
                            .get(&legacy.conflict_id)
                    })
                    .is_some_and(|record| {
                        record.status == MembershipConflictStatus::Completed
                            && record.selected_branch_id == Some(record.local_branch_id)
                    });
        let target_recovery_completed = snapshot
            .record()
            .membership_branch_recovery_sessions
            .values()
            .filter_map(MembershipBranchRecoverySession::target_completion)
            .any(|(_, package)| {
                (package.conflict_id() == conflict.conflict_id
                    && package.target_branch_id() == conflict.local_branch_id
                    || legacy.as_ref().is_some_and(|legacy| {
                        package.conflict_id() == legacy.conflict_id
                            && package.target_branch_id() == legacy.local_branch_id
                    }))
                    && local
                        .admission_facts_for(package.recipient_member())
                        .is_some_and(|facts| &facts.device_id == source_device_id)
            });
        if target_recovery_completed {
            if snapshot
                .record()
                .peer_reconciliation
                .get(source_device_id)
                .is_some_and(|peer| peer.relationship == MembershipHistoryRelationship::Consistent)
            {
                return Ok(response(MembershipHistoryRelationship::Consistent));
            }
            let source_device_id = source_device_id.clone();
            self.ledger
                .compare_and_commit(|record| {
                    if record.revision != snapshot.record().revision {
                        return Err(MembershipLedgerError::Conflict);
                    }
                    let peer = record
                        .peer_reconciliation
                        .get_mut(&source_device_id)
                        .ok_or(MembershipLedgerError::RecoveryRequired)?;
                    peer.relationship = MembershipHistoryRelationship::Consistent;
                    Ok(())
                })
                .await?;
            return Ok(response(MembershipHistoryRelationship::Consistent));
        }
        let evidence_already_recorded = snapshot
            .record()
            .membership_conflicts
            .get(&conflict.conflict_id)
            .is_some_and(|current| {
                current.evidence_peer_device_ids.contains(source_device_id)
                    && !(keep_local && current.selected_branch_id.is_none())
            })
            && snapshot
                .record()
                .membership_conflict_presentations
                .get(&conflict.conflict_id)
                == Some(&presentation)
            && snapshot
                .record()
                .peer_reconciliation
                .get(source_device_id)
                .is_some_and(|peer| {
                    peer.relationship == MembershipHistoryRelationship::Diverged
                        && peer.confirmed_position.is_none()
                });
        if evidence_already_recorded {
            return Ok(response(MembershipHistoryRelationship::Diverged));
        }
        let source_device_id = source_device_id.clone();
        let expected_revision = snapshot.record().revision;
        self.ledger
            .compare_and_commit(|record| {
                if record.revision != expected_revision {
                    return Err(MembershipLedgerError::Conflict);
                }
                let peer = record
                    .peer_reconciliation
                    .get_mut(&source_device_id)
                    .ok_or(MembershipLedgerError::RecoveryRequired)?;
                peer.relationship = MembershipHistoryRelationship::Diverged;
                peer.confirmed_position = None;
                record
                    .membership_conflict_presentations
                    .insert(conflict.conflict_id, presentation);
                record
                    .membership_conflicts
                    .entry(conflict.conflict_id)
                    .and_modify(|current| {
                        current
                            .evidence_peer_device_ids
                            .insert(source_device_id.clone());
                        if keep_local && current.selected_branch_id.is_none() {
                            current.status = MembershipConflictStatus::Completed;
                            current.selected_branch_id = Some(current.local_branch_id);
                        }
                    })
                    .or_insert_with(|| MembershipConflictRecord {
                        conflict_id: conflict.conflict_id,
                        local_branch_id: conflict.local_branch_id,
                        remote_branch_id: conflict.remote_branch_id,
                        local_choice,
                        remote_choice,
                        evidence_peer_device_ids: [source_device_id.clone()].into(),
                        detected_at_revision: record.revision,
                        status: if keep_local {
                            MembershipConflictStatus::Completed
                        } else {
                            MembershipConflictStatus::Unresolved
                        },
                        selected_branch_id: keep_local.then_some(conflict.local_branch_id),
                        transition_id: None,
                    });
                Ok(())
            })
            .await?;
        Ok(response(MembershipHistoryRelationship::Diverged))
    }
}
