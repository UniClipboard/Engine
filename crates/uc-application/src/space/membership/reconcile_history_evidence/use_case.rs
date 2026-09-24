use std::sync::Arc;

use uc_core::membership::{
    LedgerInput, MembershipConflictEvidenceV3, MembershipConflictPolicy,
    MembershipHistoryRelationship, PeerEvidence, PeerLink, PeerRelation,
    VersionedMembershipHistory,
};

use super::MembershipEvidenceExchange;
use crate::space::membership::{
    ledger_error, MembershipBranchRecoverySession, MembershipConflictPresentation,
    MembershipConflictRecord, MembershipConflictStatus, MembershipLedgerError, MembershipOwner,
};

pub(crate) struct ReconcileMembershipEvidenceUseCase {
    owner: Arc<MembershipOwner>,
}

impl ReconcileMembershipEvidenceUseCase {
    pub(crate) fn new(owner: Arc<MembershipOwner>) -> Self {
        Self { owner }
    }

    pub(crate) async fn execute(
        &self,
        source_device_id: &uc_core::ids::DeviceId,
        evidence: &MembershipConflictEvidenceV3,
    ) -> Result<Option<MembershipEvidenceExchange>, MembershipLedgerError> {
        let view = self.owner.load().await?;
        let space = view.require_space()?;
        let local = space.history();
        let Ok(remote) = VersionedMembershipHistory::import_exchange_pages_v2(
            &evidence.pages,
            self.owner.verifier(),
        ) else {
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
        let local_member = space.local_member();
        let relation = match space.ledger().peer(source_device_id) {
            Some(PeerLink::Member(link)) => Some(link.relation()),
            Some(PeerLink::Departing(_)) | None => None,
        };
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
            if relation != Some(PeerRelation::Consistent) {
                self.commit_consistent(view.revision(), *source_device_id)
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
                    .and_then(|legacy| space.branch_recovery().conflicts.get(&legacy.conflict_id))
                    .is_some_and(|record| {
                        record.status == MembershipConflictStatus::Completed
                            && record.selected_branch_id == Some(record.local_branch_id)
                    });
        let target_recovery_completed = space
            .branch_recovery()
            .recovery_sessions
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
            if relation != Some(PeerRelation::Consistent) {
                self.commit_consistent(view.revision(), *source_device_id)
                    .await?;
            }
            return Ok(response(MembershipHistoryRelationship::Consistent));
        }
        let branch_recovery = space.branch_recovery();
        let evidence_already_recorded = branch_recovery
            .conflicts
            .get(&conflict.conflict_id)
            .is_some_and(|current| {
                current.evidence_peer_device_ids.contains(source_device_id)
                    && !(keep_local && current.selected_branch_id.is_none())
            })
            && branch_recovery
                .conflict_presentations
                .get(&conflict.conflict_id)
                == Some(&presentation)
            && relation == Some(PeerRelation::Diverged);
        if evidence_already_recorded {
            return Ok(response(MembershipHistoryRelationship::Diverged));
        }
        let source_device_id = *source_device_id;
        let expected_revision = view.revision();
        self.owner
            .commit(move |draft| {
                if draft.require_space()?.ledger().revision() != expected_revision {
                    return Err(MembershipLedgerError::Conflict);
                }
                draft
                    .apply(LedgerInput::PeerEvidenceReconciled {
                        source: source_device_id,
                        history: None,
                        evidence: PeerEvidence::Diverged,
                    })
                    .map_err(ledger_error)?;
                let branch_recovery = draft.branch_recovery_mut()?;
                branch_recovery
                    .conflict_presentations
                    .insert(conflict.conflict_id, presentation);
                branch_recovery
                    .conflicts
                    .entry(conflict.conflict_id)
                    .and_modify(|current| {
                        current.evidence_peer_device_ids.insert(source_device_id);
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
                        evidence_peer_device_ids: [source_device_id].into(),
                        detected_at_revision: expected_revision,
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

impl ReconcileMembershipEvidenceUseCase {
    /// 双方分支一致但证据不能证明对端拥有本机当前位置：只修复关系，不伪造确认水位。
    async fn commit_consistent(
        &self,
        expected_revision: u64,
        source_device_id: uc_core::ids::DeviceId,
    ) -> Result<(), MembershipLedgerError> {
        self.owner
            .commit(move |draft| {
                if draft.require_space()?.ledger().revision() != expected_revision {
                    return Err(MembershipLedgerError::Conflict);
                }
                draft
                    .apply(LedgerInput::PeerEvidenceReconciled {
                        source: source_device_id,
                        history: None,
                        evidence: PeerEvidence::Consistent,
                    })
                    .map_err(ledger_error)?;
                Ok(())
            })
            .await?;
        Ok(())
    }
}
