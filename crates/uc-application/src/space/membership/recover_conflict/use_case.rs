use std::sync::Arc;

use uc_core::membership::{
    HistoricalMembershipSignatureVerifier, MembershipBranchTransitionPhaseV1,
};
use uc_core::ports::ClockPort;

use crate::space::membership::{
    MembershipBranchRecoverySession, MembershipConflictStatus, MembershipLedger,
    MembershipLedgerError, MembershipMaintenanceStepOutcome, RecoverMembershipConflictsPort,
};

use super::{
    AdvanceMembershipBranchTransitionError, AdvanceMembershipBranchTransitionInput,
    AdvanceMembershipBranchTransitionPort, MembershipBranchRecoveryChannelError,
    MembershipBranchRecoveryChannelPort, MembershipBranchRecoveryCommit,
    MembershipBranchRecoveryRequest, PrepareMembershipBranchRecoveryRecipientError,
    PrepareMembershipBranchRecoveryRecipientPort, PrepareMembershipBranchTransitionError,
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
    recovery_channel: Arc<dyn MembershipBranchRecoveryChannelPort>,
    recipient_preparer: Arc<dyn PrepareMembershipBranchRecoveryRecipientPort>,
    transition: Arc<dyn PrepareMembershipBranchTransitionPort>,
    transition_executor: Arc<dyn AdvanceMembershipBranchTransitionPort>,
    verifier: Arc<dyn HistoricalMembershipSignatureVerifier>,
    clock: Arc<dyn ClockPort>,
    execution_lock: tokio::sync::Mutex<()>,
}

#[async_trait::async_trait]
impl RecoverMembershipConflictsPort for RecoverMembershipConflictUseCase {
    async fn recover_membership_conflicts(&self) -> MembershipMaintenanceStepOutcome {
        match self.execute().await {
            RecoverMembershipConflictOutcome::Completed => {
                MembershipMaintenanceStepOutcome::Completed
            }
            RecoverMembershipConflictOutcome::Deferred => {
                MembershipMaintenanceStepOutcome::Deferred
            }
            RecoverMembershipConflictOutcome::StableFailure => {
                MembershipMaintenanceStepOutcome::StableFailure
            }
            RecoverMembershipConflictOutcome::Corrupt => MembershipMaintenanceStepOutcome::Corrupt,
        }
    }
}

impl RecoverMembershipConflictUseCase {
    pub(crate) fn new(
        ledger: Arc<MembershipLedger>,
        recovery_channel: Arc<dyn MembershipBranchRecoveryChannelPort>,
        recipient_preparer: Arc<dyn PrepareMembershipBranchRecoveryRecipientPort>,
        transition: Arc<dyn PrepareMembershipBranchTransitionPort>,
        transition_executor: Arc<dyn AdvanceMembershipBranchTransitionPort>,
        verifier: Arc<dyn HistoricalMembershipSignatureVerifier>,
        clock: Arc<dyn ClockPort>,
    ) -> Self {
        Self {
            ledger,
            recovery_channel,
            recipient_preparer,
            transition,
            transition_executor,
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
        let conflict_id = conflict.conflict_id;
        if let Some(transition) = snapshot
            .record()
            .membership_branch_transitions
            .get(&transition_id)
            .cloned()
        {
            let Some(session) = snapshot
                .record()
                .membership_branch_recovery_sessions
                .get(&transition_id)
            else {
                return RecoverMembershipConflictOutcome::Corrupt;
            };
            let Some((recipient_staged_mls_state, recovery_package)) =
                session.recipient_completion()
            else {
                return RecoverMembershipConflictOutcome::Corrupt;
            };
            return self
                .advance_existing_transition(
                    conflict_id,
                    transition_id,
                    transition,
                    recipient_staged_mls_state.to_vec(),
                    recovery_package.clone(),
                )
                .await;
        }
        let Some(peer_device_id) = conflict.evidence_peer_device_ids.iter().next().cloned() else {
            return RecoverMembershipConflictOutcome::Corrupt;
        };
        let request = MembershipBranchRecoveryRequest {
            peer_device_id,
            conflict_id,
            target_branch_id,
            recipient_member,
        };
        let existing_session = snapshot
            .record()
            .membership_branch_recovery_sessions
            .get(&transition_id)
            .cloned();
        let package = if let Some(session) = existing_session.as_ref() {
            if let Some((_, package)) = session.recipient_completion() {
                package.clone()
            } else {
                let Some((external_commit, _)) = session.recipient_preparation() else {
                    return RecoverMembershipConflictOutcome::Corrupt;
                };
                match self
                    .submit_and_persist_package(
                        request.clone(),
                        transition_id,
                        external_commit.to_vec(),
                    )
                    .await
                {
                    Ok(package) => package,
                    Err(outcome) => return outcome,
                }
            }
        } else {
            let group_info = match self
                .recovery_channel
                .request_membership_branch_group_info(request.clone())
                .await
            {
                Ok(group_info) => group_info,
                Err(error) => return map_channel_error(error),
            };
            let prepared = match self
                .recipient_preparer
                .prepare_membership_branch_recovery_recipient(group_info)
                .await
            {
                Ok(prepared) => prepared,
                Err(error) => return map_recipient_error(error),
            };
            let Some(session) = MembershipBranchRecoverySession::new_recipient_prepared(
                transition_id,
                conflict_id,
                target_branch_id,
                recipient_member,
                prepared.external_commit.clone(),
                prepared.staged_mls_state,
            ) else {
                return RecoverMembershipConflictOutcome::StableFailure;
            };
            let persisted = self
                .ledger
                .compare_and_commit(move |record| {
                    let current = record
                        .membership_conflicts
                        .get(&conflict_id)
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
                    if record
                        .membership_branch_recovery_sessions
                        .insert(transition_id, session)
                        .is_some()
                    {
                        return Err(MembershipLedgerError::Conflict);
                    }
                    Ok(())
                })
                .await;
            if let Err(error) = persisted {
                return map_ledger_error(error);
            }
            match self
                .submit_and_persist_package(request, transition_id, prepared.external_commit)
                .await
            {
                Ok(package) => package,
                Err(outcome) => return outcome,
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

    async fn advance_existing_transition(
        &self,
        conflict_id: uc_core::membership::MembershipConflictId,
        transition_id: [u8; 32],
        mut transition: uc_core::membership::MembershipBranchTransitionV1,
        recipient_staged_mls_state: Vec<u8>,
        recovery_package: uc_core::membership::MembershipBranchRecoveryPackageV1,
    ) -> RecoverMembershipConflictOutcome {
        loop {
            let target_history =
                match uc_core::membership::VersionedMembershipHistory::decode_persisted_v2(
                    recovery_package.target_membership_history(),
                    self.verifier.as_ref(),
                ) {
                    Ok(history) => history,
                    Err(_) => return RecoverMembershipConflictOutcome::StableFailure,
                };
            let next = match self
                .transition_executor
                .advance_membership_branch_transition(AdvanceMembershipBranchTransitionInput {
                    transition: transition.clone(),
                    recipient_staged_mls_state: recipient_staged_mls_state.clone(),
                    recovery_package: recovery_package.clone(),
                    target_history,
                })
                .await
            {
                Ok(next) => next,
                Err(AdvanceMembershipBranchTransitionError::Unavailable { .. }) => {
                    return RecoverMembershipConflictOutcome::Deferred;
                }
                Err(AdvanceMembershipBranchTransitionError::Invalid { .. }) => {
                    return RecoverMembershipConflictOutcome::StableFailure;
                }
                Err(AdvanceMembershipBranchTransitionError::RecoveryRequired { .. }) => {
                    return RecoverMembershipConflictOutcome::Corrupt;
                }
            };
            if transition.advance(next.phase()).as_ref() != Some(&next) {
                return RecoverMembershipConflictOutcome::StableFailure;
            }
            if let Err(error) = self.ledger.load_verified().await {
                return map_ledger_error(error);
            }
            let completed = next.phase() == MembershipBranchTransitionPhaseV1::Completed;
            let previous_phase = transition.phase();
            let next_phase = next.phase();
            let persisted_transition = transition.clone();
            let persisted_next = next.clone();
            match self
                .ledger
                .compare_and_commit(move |record| {
                    let current = record
                        .membership_branch_transitions
                        .get_mut(&transition_id)
                        .ok_or(MembershipLedgerError::Conflict)?;
                    if current != &persisted_transition {
                        return Err(MembershipLedgerError::Conflict);
                    }
                    *current = persisted_next;
                    if completed {
                        let conflict = record
                            .membership_conflicts
                            .get_mut(&conflict_id)
                            .ok_or(MembershipLedgerError::Conflict)?;
                        if conflict.transition_id != Some(transition_id) {
                            return Err(MembershipLedgerError::Conflict);
                        }
                        conflict.status = MembershipConflictStatus::Completed;
                        record
                            .membership_branch_recovery_sessions
                            .remove(&transition_id)
                            .ok_or(MembershipLedgerError::Conflict)?;
                    }
                    Ok(())
                })
                .await
            {
                Ok(_) if completed => {
                    tracing::debug!(?previous_phase, ?next_phase, "成员分支转换阶段已持久化");
                    return RecoverMembershipConflictOutcome::Completed;
                }
                Ok(_) => {
                    tracing::debug!(?previous_phase, ?next_phase, "成员分支转换阶段已持久化");
                    transition = next;
                }
                Err(MembershipLedgerError::Conflict) => {
                    return RecoverMembershipConflictOutcome::StableFailure;
                }
                Err(error) => return map_ledger_error(error),
            }
        }
    }

    async fn submit_and_persist_package(
        &self,
        request: MembershipBranchRecoveryRequest,
        transition_id: [u8; 32],
        external_commit: Vec<u8>,
    ) -> Result<
        uc_core::membership::MembershipBranchRecoveryPackageV1,
        RecoverMembershipConflictOutcome,
    > {
        let package = self
            .recovery_channel
            .submit_membership_branch_external_commit(MembershipBranchRecoveryCommit {
                request: request.clone(),
                external_commit,
            })
            .await
            .map_err(map_channel_error)?;
        if package
            .validate(
                request.conflict_id,
                request.target_branch_id,
                request.recipient_member,
                self.clock.now_ms(),
                self.verifier.as_ref(),
            )
            .is_err()
        {
            return Err(RecoverMembershipConflictOutcome::StableFailure);
        }
        let persisted_package = package.clone();
        self.ledger
            .compare_and_commit(move |record| {
                let session = record
                    .membership_branch_recovery_sessions
                    .get_mut(&transition_id)
                    .ok_or(MembershipLedgerError::Conflict)?;
                session
                    .complete_recipient(persisted_package)
                    .then_some(())
                    .ok_or(MembershipLedgerError::Conflict)
            })
            .await
            .map_err(map_ledger_error)?;
        Ok(package)
    }
}

fn map_channel_error(
    error: MembershipBranchRecoveryChannelError,
) -> RecoverMembershipConflictOutcome {
    match error {
        MembershipBranchRecoveryChannelError::Unavailable { .. } => {
            RecoverMembershipConflictOutcome::Deferred
        }
        MembershipBranchRecoveryChannelError::Rejected { .. }
        | MembershipBranchRecoveryChannelError::Invalid { .. } => {
            RecoverMembershipConflictOutcome::StableFailure
        }
    }
}

fn map_recipient_error(
    error: PrepareMembershipBranchRecoveryRecipientError,
) -> RecoverMembershipConflictOutcome {
    match error {
        PrepareMembershipBranchRecoveryRecipientError::Unavailable { .. } => {
            RecoverMembershipConflictOutcome::Deferred
        }
        PrepareMembershipBranchRecoveryRecipientError::Invalid { .. } => {
            RecoverMembershipConflictOutcome::StableFailure
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
