use std::sync::Arc;

use uc_core::membership::{
    HistoricalMembershipSignatureVerifier, MembershipBranchTransitionPhaseV1,
};
use uc_core::ports::ClockPort;

use crate::space::membership::{MembershipConflictStatus, MembershipLedger, MembershipLedgerError};

use super::{
    FetchMembershipBranchRecoveryError, FetchMembershipBranchRecoveryInput,
    FetchMembershipBranchRecoveryPort, PrepareMembershipBranchTransitionError,
    PrepareMembershipBranchTransitionInput, PrepareMembershipBranchTransitionPort,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecoverMembershipConflictOutcome {
    Completed,
    Deferred,
    StableFailure,
    Corrupt,
}

pub(crate) struct RecoverMembershipConflictUseCase {
    ledger: Arc<MembershipLedger>,
    recovery: Arc<dyn FetchMembershipBranchRecoveryPort>,
    transition: Arc<dyn PrepareMembershipBranchTransitionPort>,
    verifier: Arc<dyn HistoricalMembershipSignatureVerifier>,
    clock: Arc<dyn ClockPort>,
    execution_lock: tokio::sync::Mutex<()>,
}

impl RecoverMembershipConflictUseCase {
    pub(crate) fn new(
        ledger: Arc<MembershipLedger>,
        recovery: Arc<dyn FetchMembershipBranchRecoveryPort>,
        transition: Arc<dyn PrepareMembershipBranchTransitionPort>,
        verifier: Arc<dyn HistoricalMembershipSignatureVerifier>,
        clock: Arc<dyn ClockPort>,
    ) -> Self {
        Self {
            ledger,
            recovery,
            transition,
            verifier,
            clock,
            execution_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub(crate) async fn execute(&self) -> RecoverMembershipConflictOutcome {
        let _guard = self.execution_lock.lock().await;
        let snapshot = match self.ledger.load_verified().await {
            Ok(snapshot) => snapshot,
            Err(error) => return map_ledger_error(error),
        };
        let Some(conflict) = snapshot
            .record()
            .membership_conflicts
            .values()
            .find(|conflict| {
                conflict.status == MembershipConflictStatus::Selected
                    || conflict.status == MembershipConflictStatus::Transitioning
            })
        else {
            return RecoverMembershipConflictOutcome::Completed;
        };
        let (Some(target_branch_id), Some(transition_id), Some(recipient_member)) = (
            conflict.selected_branch_id,
            conflict.transition_id,
            snapshot.record().local_member_instance,
        ) else {
            return RecoverMembershipConflictOutcome::Corrupt;
        };
        if snapshot
            .record()
            .membership_branch_transitions
            .contains_key(&transition_id)
        {
            return RecoverMembershipConflictOutcome::Completed;
        }
        let conflict_id = conflict.conflict_id;
        let package = match self
            .recovery
            .fetch_membership_branch_recovery(FetchMembershipBranchRecoveryInput {
                conflict_id,
                target_branch_id,
                recipient_member,
                evidence_peer_device_ids: conflict.evidence_peer_device_ids.clone(),
            })
            .await
        {
            Ok(package) => package,
            Err(FetchMembershipBranchRecoveryError::Unavailable { .. }) => {
                return RecoverMembershipConflictOutcome::Deferred;
            }
            Err(FetchMembershipBranchRecoveryError::Rejected { .. }) => {
                return RecoverMembershipConflictOutcome::StableFailure;
            }
        };
        if package
            .validate(
                conflict_id,
                target_branch_id,
                recipient_member,
                self.clock.now_ms(),
                self.verifier.as_ref(),
            )
            .is_err()
        {
            return RecoverMembershipConflictOutcome::StableFailure;
        }
        let nonce = *package.nonce();
        let prepared = match self
            .transition
            .prepare_membership_branch_transition(PrepareMembershipBranchTransitionInput {
                transition_id,
                conflict_id,
                target_branch_id,
                package,
            })
            .await
        {
            Ok(prepared) => prepared,
            Err(PrepareMembershipBranchTransitionError::Unavailable { .. }) => {
                return RecoverMembershipConflictOutcome::Deferred;
            }
            Err(PrepareMembershipBranchTransitionError::Invalid { .. }) => {
                return RecoverMembershipConflictOutcome::StableFailure;
            }
        };
        if !prepared.validate()
            || prepared.phase() != MembershipBranchTransitionPhaseV1::Prepared
            || prepared.transition_id() != &transition_id
            || prepared.conflict_id() != conflict_id
            || prepared.target_branch_id() != target_branch_id
        {
            return RecoverMembershipConflictOutcome::StableFailure;
        }

        match self
            .ledger
            .compare_and_commit(move |record| {
                if let Some(consuming_conflict) =
                    record.consumed_membership_recovery_nonces.get(&nonce)
                {
                    if consuming_conflict != &conflict_id {
                        return Err(MembershipLedgerError::Conflict);
                    }
                }
                let current = record
                    .membership_conflicts
                    .get_mut(&conflict_id)
                    .ok_or(MembershipLedgerError::Conflict)?;
                if current.selected_branch_id != Some(target_branch_id)
                    || current.transition_id != Some(transition_id)
                    || !matches!(
                        current.status,
                        MembershipConflictStatus::Selected
                            | MembershipConflictStatus::Transitioning
                    )
                {
                    return Err(MembershipLedgerError::Conflict);
                }
                if let Some(existing) = record.membership_branch_transitions.get(&transition_id) {
                    return (existing == &prepared)
                        .then_some(())
                        .ok_or(MembershipLedgerError::Conflict);
                }
                record
                    .consumed_membership_recovery_nonces
                    .insert(nonce, conflict_id);
                record
                    .membership_branch_transitions
                    .insert(transition_id, prepared);
                current.status = MembershipConflictStatus::Transitioning;
                Ok(())
            })
            .await
        {
            Ok(_) => RecoverMembershipConflictOutcome::Completed,
            Err(MembershipLedgerError::Conflict) => RecoverMembershipConflictOutcome::StableFailure,
            Err(error) => map_ledger_error(error),
        }
    }
}

fn map_ledger_error(error: MembershipLedgerError) -> RecoverMembershipConflictOutcome {
    match error {
        MembershipLedgerError::Locked | MembershipLedgerError::Unavailable => {
            RecoverMembershipConflictOutcome::Deferred
        }
        MembershipLedgerError::Conflict => RecoverMembershipConflictOutcome::StableFailure,
        MembershipLedgerError::Corrupt | MembershipLedgerError::RecoveryRequired => {
            RecoverMembershipConflictOutcome::Corrupt
        }
    }
}
