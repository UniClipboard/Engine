use std::sync::Arc;
use uc_core::error_class::ErrorClass;

use uc_core::membership::{
    HistoricalMembershipSignatureVerifier, LedgerInput, LedgerOutcome, MemberInstanceId,
    MembershipBranchTransitionPhaseV1, MembershipBranchTransitionV1, MembershipHistoryV2Error,
    VersionedMembershipHistory,
};
use uc_core::ports::ClockPort;

use crate::space::membership::{
    ledger_error, MembershipBranchRecoverySession, MembershipConflictStatus, MembershipLedgerError,
    MembershipMaintenanceStepOutcome, MembershipOwner, RecoverMembershipConflictsPort,
    StagedMembershipRecord,
};

use super::{
    AdvanceMembershipBranchTransitionError, AdvanceMembershipBranchTransitionInput,
    AdvanceMembershipBranchTransitionPort, MembershipBranchRecoveryChannelError,
    MembershipBranchRecoveryChannelPort, MembershipBranchRecoveryCommit,
    MembershipBranchRecoveryRequest, PrepareMembershipBranchRecoveryRecipientError,
    PrepareMembershipBranchRecoveryRecipientPort, PrepareMembershipBranchTransitionError,
    PrepareMembershipBranchTransitionInput, PrepareMembershipBranchTransitionPort,
};
use uc_observability_contract::{log_fields::log_vocab_debug, uc_debug, uc_warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecoverMembershipConflictOutcome {
    Completed,
    Deferred,
    StableFailure,
    Corrupt,
}

pub(crate) struct RecoverMembershipConflictUseCase {
    owner: Arc<MembershipOwner>,
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
        owner: Arc<MembershipOwner>,
        recovery_channel: Arc<dyn MembershipBranchRecoveryChannelPort>,
        recipient_preparer: Arc<dyn PrepareMembershipBranchRecoveryRecipientPort>,
        transition: Arc<dyn PrepareMembershipBranchTransitionPort>,
        transition_executor: Arc<dyn AdvanceMembershipBranchTransitionPort>,
        verifier: Arc<dyn HistoricalMembershipSignatureVerifier>,
        clock: Arc<dyn ClockPort>,
    ) -> Self {
        Self {
            owner,
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
        let view = match self.owner.load().await {
            Ok(view) => view,
            Err(error) => return map_ledger_error(error),
        };
        let Some(space) = view.space() else {
            return RecoverMembershipConflictOutcome::Completed;
        };
        let record = space.branch_recovery();
        let Some(conflict) = record.conflicts.values().find(|conflict| {
            conflict.status == MembershipConflictStatus::Selected
                || conflict.status == MembershipConflictStatus::Transitioning
        }) else {
            return RecoverMembershipConflictOutcome::Completed;
        };
        let recipient_member = space.local_member();
        let (Some(target_branch_id), Some(transition_id)) =
            (conflict.selected_branch_id, conflict.transition_id)
        else {
            return RecoverMembershipConflictOutcome::Corrupt;
        };
        let conflict_id = conflict.conflict_id;
        if let Some(transition) = record.branch_transitions.get(&transition_id).cloned() {
            let Some(session) = record.recovery_sessions.get(&transition_id) else {
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
        let existing_session = record.recovery_sessions.get(&transition_id).cloned();
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
                return stable_failure("recipient_session", "invalid_session");
            };
            let persisted = self
                .owner
                .commit(move |draft| {
                    let record = draft.branch_recovery_mut()?;
                    let current = record
                        .conflicts
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
                        .recovery_sessions
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
            return stable_failure("package_validation", "invalid");
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
                return stable_failure("prepare_transition", "invalid");
            }
        };
        if !prepared.validate()
            || prepared.phase() != MembershipBranchTransitionPhaseV1::Prepared
            || prepared.transition_id() != &transition_id
            || prepared.conflict_id() != conflict_id
            || prepared.target_branch_id() != target_branch_id
        {
            return stable_failure("prepare_transition", "mismatch");
        }

        match self
            .owner
            .commit(move |draft| {
                let record = draft.branch_recovery_mut()?;
                if let Some(consuming_conflict) = record.consumed_recovery_nonces.get(&nonce) {
                    if consuming_conflict != &conflict_id {
                        return Err(MembershipLedgerError::Conflict);
                    }
                }
                let current = record
                    .conflicts
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
                if let Some(existing) = record.branch_transitions.get(&transition_id) {
                    return (existing == &prepared)
                        .then_some(())
                        .ok_or(MembershipLedgerError::Conflict);
                }
                record.consumed_recovery_nonces.insert(nonce, conflict_id);
                record.branch_transitions.insert(transition_id, prepared);
                current.status = MembershipConflictStatus::Transitioning;
                Ok(())
            })
            .await
        {
            Ok(_) => RecoverMembershipConflictOutcome::Completed,
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
                    Err(error) => return decode_target_failed(&error),
                };
            let staged_membership =
                if transition.phase() == MembershipBranchTransitionPhaseV1::TargetVerified {
                    match self
                        .stage_target_membership(
                            transition_id,
                            &transition,
                            recovery_package.recipient_member(),
                            &target_history,
                        )
                        .await
                    {
                        Ok(staged) => Some(staged),
                        Err(error) => return map_ledger_error(error),
                    }
                } else {
                    None
                };
            let next = match self
                .transition_executor
                .advance_membership_branch_transition(AdvanceMembershipBranchTransitionInput {
                    transition: transition.clone(),
                    recipient_staged_mls_state: recipient_staged_mls_state.clone(),
                    recovery_package: recovery_package.clone(),
                    target_history,
                    staged_membership,
                })
                .await
            {
                Ok(next) => next,
                Err(AdvanceMembershipBranchTransitionError::Unavailable { .. }) => {
                    return RecoverMembershipConflictOutcome::Deferred;
                }
                Err(AdvanceMembershipBranchTransitionError::Invalid { .. }) => {
                    return stable_failure("advance_transition", "invalid");
                }
                Err(AdvanceMembershipBranchTransitionError::RecoveryRequired { .. }) => {
                    uc_warn!(
                        stage = "advance_transition",
                        error_kind = "recovery_required",
                        "membership conflict recovery needs manual recovery"
                    );
                    return RecoverMembershipConflictOutcome::Corrupt;
                }
            };
            if transition.advance(next.phase()).as_ref() != Some(&next) {
                return stable_failure("advance_transition", "phase_mismatch");
            }
            // 提升后当前控制世代已换成目标世代，Owner 按数据库代号的变化读取其中已暂存的成员记录。
            if let Err(error) = self.owner.load().await {
                return map_ledger_error(error);
            }
            let completed = next.phase() == MembershipBranchTransitionPhaseV1::Completed;
            let previous_phase = transition.phase();
            let next_phase = next.phase();
            let persisted_transition = transition.clone();
            let persisted_next = next.clone();
            match self
                .owner
                .commit(move |draft| {
                    let record = draft.branch_recovery_mut()?;
                    let current = record
                        .branch_transitions
                        .get_mut(&transition_id)
                        .ok_or(MembershipLedgerError::Conflict)?;
                    if current != &persisted_transition {
                        return Err(MembershipLedgerError::Conflict);
                    }
                    *current = persisted_next;
                    if completed {
                        let conflict = record
                            .conflicts
                            .get_mut(&conflict_id)
                            .ok_or(MembershipLedgerError::Conflict)?;
                        if conflict.transition_id != Some(transition_id) {
                            return Err(MembershipLedgerError::Conflict);
                        }
                        conflict.status = MembershipConflictStatus::Completed;
                        record
                            .recovery_sessions
                            .remove(&transition_id)
                            .ok_or(MembershipLedgerError::Conflict)?;
                    }
                    Ok(())
                })
                .await
            {
                Ok(_) if completed => {
                    uc_debug!(
                        previous_phase = log_vocab_debug(&previous_phase),
                        next_phase = log_vocab_debug(&next_phase),
                        "成员分支转换阶段已持久化"
                    );
                    return RecoverMembershipConflictOutcome::Completed;
                }
                Ok(_) => {
                    uc_debug!(
                        previous_phase = log_vocab_debug(&previous_phase),
                        next_phase = log_vocab_debug(&next_phase),
                        "成员分支转换阶段已持久化"
                    );
                    transition = next;
                }
                Err(error) => return map_ledger_error(error),
            }
        }
    }

    /// 目标控制世代的成员记录：在当前状态上采用目标分支历史，并把本次转换的检查点推进到
    /// `TargetStaged`，使目标世代提升后即可从该检查点继续。
    async fn stage_target_membership(
        &self,
        transition_id: [u8; 32],
        transition: &MembershipBranchTransitionV1,
        recipient_member: MemberInstanceId,
        target_history: &VersionedMembershipHistory,
    ) -> Result<StagedMembershipRecord, MembershipLedgerError> {
        let staged_transition = transition
            .advance(MembershipBranchTransitionPhaseV1::TargetStaged)
            .ok_or(MembershipLedgerError::Conflict)?;
        let history = target_history.clone();
        self.owner
            .stage(move |draft| {
                if draft.require_space()?.local_member() != recipient_member {
                    return Err(MembershipLedgerError::Conflict);
                }
                let checkpoint = draft
                    .branch_recovery_mut()?
                    .branch_transitions
                    .get_mut(&transition_id)
                    .ok_or(MembershipLedgerError::Conflict)?;
                if checkpoint != transition {
                    return Err(MembershipLedgerError::Conflict);
                }
                *checkpoint = staged_transition;
                // 旧分支的历史交换暂存不跨分支继承。
                *draft.history_exchange_mut()? = Default::default();
                match draft
                    .apply(LedgerInput::BranchRecovered { history })
                    .map_err(ledger_error)?
                {
                    LedgerOutcome::Applied => Ok(()),
                    LedgerOutcome::Unchanged | LedgerOutcome::Stale => {
                        Err(MembershipLedgerError::Conflict)
                    }
                }
            })
            .await
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
            return Err(stable_failure("package_validation", "invalid"));
        }
        let persisted_package = package.clone();
        self.owner
            .commit(move |draft| {
                let session = draft
                    .branch_recovery_mut()?
                    .recovery_sessions
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
        MembershipBranchRecoveryChannelError::Rejected { .. } => {
            stable_failure("channel", "rejected")
        }
        MembershipBranchRecoveryChannelError::Invalid { .. } => {
            stable_failure("channel", "invalid")
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
            stable_failure("recipient", "invalid")
        }
    }
}

fn map_ledger_error(error: MembershipLedgerError) -> RecoverMembershipConflictOutcome {
    match error {
        MembershipLedgerError::Locked | MembershipLedgerError::Unavailable { .. } => {
            RecoverMembershipConflictOutcome::Deferred
        }
        MembershipLedgerError::Conflict => stable_failure("ledger", "conflict"),
        MembershipLedgerError::Corrupt { .. } | MembershipLedgerError::RecoveryRequired => {
            RecoverMembershipConflictOutcome::Corrupt
        }
    }
}

/// 冲突恢复在此阶段得出不会自行重试的结论：只记录固定阶段与分类，不含冲突、转换或成员标识。
fn stable_failure(
    stage: &'static str,
    error_kind: &'static str,
) -> RecoverMembershipConflictOutcome {
    uc_warn!(
        stage = stage,
        error_kind = error_kind,
        "membership conflict recovery stopped with a stable failure"
    );
    RecoverMembershipConflictOutcome::StableFailure
}

fn decode_target_failed(error: &MembershipHistoryV2Error) -> RecoverMembershipConflictOutcome {
    uc_warn!(
        stage = "decode_target",
        error_kind = "decode",
        error_class = error.class(),
        "membership conflict recovery stopped with a stable failure"
    );
    RecoverMembershipConflictOutcome::StableFailure
}
