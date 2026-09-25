use std::sync::Arc;

use uc_core::membership::{
    LedgerInput, MembershipConflictPolicy, MembershipDecisionStoreOutcome, MembershipOperationV2,
    RemovalDecision,
};

use crate::space::membership::{
    ledger_error, CurrentMemberSignatureError, CurrentMemberSignaturePort,
    MembershipConflictStatus, MembershipLedgerError, MembershipOwner, QueryDeviceTrustUseCase,
    RecoverMembershipEffectsPort,
};

use super::{
    DecideDeviceTrustChange, DecideDeviceTrustChangeError, DecideDeviceTrustChangeResult,
    DeviceTrustChangeChoice,
};

pub(crate) struct DecideDeviceTrustChangeUseCase {
    owner: Arc<MembershipOwner>,
    signer: Arc<dyn CurrentMemberSignaturePort>,
    query: Arc<QueryDeviceTrustUseCase>,
    effects: Arc<dyn RecoverMembershipEffectsPort>,
    execution_lock: tokio::sync::Mutex<()>,
}

impl DecideDeviceTrustChangeUseCase {
    pub(crate) fn new(
        owner: Arc<MembershipOwner>,
        signer: Arc<dyn CurrentMemberSignaturePort>,
        query: Arc<QueryDeviceTrustUseCase>,
        effects: Arc<dyn RecoverMembershipEffectsPort>,
    ) -> Self {
        Self {
            owner,
            signer,
            query,
            effects,
            execution_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub(crate) async fn execute(
        &self,
        input: DecideDeviceTrustChange,
    ) -> Result<DecideDeviceTrustChangeResult, DecideDeviceTrustChangeError> {
        let _guard = self.execution_lock.lock().await;
        match self.execute_once(input).await {
            Err(DecideDeviceTrustChangeError::StateChanged) => {
                match self.execute_once(input).await {
                    Err(DecideDeviceTrustChangeError::StateChanged) => {
                        let status = self
                            .query
                            .execute()
                            .await
                            .map_err(DecideDeviceTrustChangeError::unavailable_from)?;
                        Ok(DecideDeviceTrustChangeResult::StateChanged {
                            current_change_id: status
                                .current_change
                                .as_ref()
                                .map(|change| change.change_id),
                            status,
                        })
                    }
                    result => result,
                }
            }
            result => result,
        }
    }

    async fn execute_once(
        &self,
        input: DecideDeviceTrustChange,
    ) -> Result<DecideDeviceTrustChangeResult, DecideDeviceTrustChangeError> {
        let view = self.owner.load().await.map_err(map_ledger_error)?;
        let space = view.require_space().map_err(map_ledger_error)?;
        let history = space.history();
        let local_member = space.local_member();
        if let Some(completed) = history.decision_for(input.change_id, local_member) {
            let choice = match completed.decision {
                RemovalDecision::Accept => DeviceTrustChangeChoice::ApplyChange,
                RemovalDecision::Reject => DeviceTrustChangeChoice::KeepCurrentDeviceGroup,
            };
            let _ = self.effects.recover_membership_effects().await;
            let status = self
                .query
                .execute()
                .await
                .map_err(DecideDeviceTrustChangeError::committed_but_pending_from)?;
            return Ok(DecideDeviceTrustChangeResult::AlreadyCompleted {
                change_id: input.change_id,
                choice,
                status,
            });
        }
        if history.pending_removal_decision(local_member) != Some(input.change_id) {
            let status = self
                .query
                .execute()
                .await
                .map_err(DecideDeviceTrustChangeError::unavailable_from)?;
            return Ok(DecideDeviceTrustChangeResult::StateChanged {
                current_change_id: status
                    .current_change
                    .as_ref()
                    .map(|change| change.change_id),
                status,
            });
        }
        let event = history
            .event(input.change_id)
            .ok_or_else(DecideDeviceTrustChangeError::recovery_required)?;
        let removes_local = matches!(
            &event.operation,
            MembershipOperationV2::RemoveDevice { member } if *member == local_member
        );
        if input.choice == DeviceTrustChangeChoice::ApplyChange
            && removes_local
            && !input.confirm_local_removal
        {
            let status = self
                .query
                .execute()
                .await
                .map_err(DecideDeviceTrustChangeError::unavailable_from)?;
            return Ok(DecideDeviceTrustChangeResult::LocalConfirmationRequired {
                change_id: input.change_id,
                status,
            });
        }
        let local_device_id = *space.local_device_id();
        let credential = self
            .signer
            .current_membership_credential(&local_device_id)
            .await
            .map_err(map_signature_error)?;
        if credential.member_instance_id(&local_device_id) != local_member {
            return Err(DecideDeviceTrustChangeError::recovery_required());
        }
        let decision_choice = match input.choice {
            DeviceTrustChangeChoice::ApplyChange => RemovalDecision::Accept,
            DeviceTrustChangeChoice::KeepCurrentDeviceGroup => RemovalDecision::Reject,
        };
        let mut decision = history
            .create_unsigned_local_removal_decision(
                input.change_id,
                local_member,
                &credential,
                decision_choice,
                uuid::Uuid::new_v4().into_bytes(),
            )
            .map_err(DecideDeviceTrustChangeError::recovery_required_from)?;
        decision.signature = self
            .signer
            .sign_current_member_payload(&decision.signing_payload())
            .await
            .map_err(map_signature_error)?;
        let change_id = input.change_id;
        let verifier = self.owner.verifier_handle();
        self.owner
            .commit(move |draft| {
                let mut history = draft.require_space()?.history().clone();
                if history.pending_removal_decision(local_member) != Some(change_id) {
                    return Err(MembershipLedgerError::Conflict);
                }
                if history
                    .apply_signed_local_removal_decision(decision, local_member, verifier.as_ref())
                    .map_err(MembershipLedgerError::corrupt_from)?
                    != MembershipDecisionStoreOutcome::Stored
                {
                    return Err(MembershipLedgerError::corrupt());
                }
                // 拒绝后本机已记录的选择就是保留当前分支；对应分叉不再等待用户选择。
                let kept_conflicts = if decision_choice == RemovalDecision::Reject {
                    draft
                        .require_space()?
                        .branch_recovery()
                        .conflicts
                        .values()
                        .filter(|conflict| {
                            conflict.selected_branch_id.is_none()
                                && MembershipConflictPolicy::has_recorded_local_choice(
                                    &history,
                                    local_member,
                                    conflict.remote_branch_id,
                                )
                        })
                        .map(|conflict| conflict.conflict_id)
                        .collect()
                } else {
                    Vec::new()
                };
                draft
                    .apply(LedgerInput::LocalDecisionSigned {
                        history,
                        removal_event_id: change_id,
                    })
                    .map_err(ledger_error)?;
                let branch_recovery = draft.branch_recovery_mut()?;
                for conflict_id in kept_conflicts {
                    if let Some(conflict) = branch_recovery.conflicts.get_mut(&conflict_id) {
                        conflict.status = MembershipConflictStatus::Completed;
                        conflict.selected_branch_id = Some(conflict.local_branch_id);
                    }
                }
                Ok(())
            })
            .await
            .map_err(map_ledger_error)?;
        let _ = self.effects.recover_membership_effects().await;
        let status = self
            .query
            .execute()
            .await
            .map_err(DecideDeviceTrustChangeError::committed_but_pending_from)?;
        Ok(match input.choice {
            DeviceTrustChangeChoice::ApplyChange => DecideDeviceTrustChangeResult::Applied {
                change_id: input.change_id,
                status,
            },
            DeviceTrustChangeChoice::KeepCurrentDeviceGroup => {
                DecideDeviceTrustChangeResult::KeptCurrentDeviceGroup {
                    change_id: input.change_id,
                    status,
                }
            }
        })
    }
}

fn map_signature_error(error: CurrentMemberSignatureError) -> DecideDeviceTrustChangeError {
    match error {
        CurrentMemberSignatureError::InvalidState { .. } => {
            DecideDeviceTrustChangeError::recovery_required()
        }
        CurrentMemberSignatureError::Unavailable { .. }
        | CurrentMemberSignatureError::Repository(_) => DecideDeviceTrustChangeError::unavailable(),
    }
}

fn map_ledger_error(error: MembershipLedgerError) -> DecideDeviceTrustChangeError {
    match error {
        MembershipLedgerError::Locked => DecideDeviceTrustChangeError::Locked,
        MembershipLedgerError::Corrupt { .. } | MembershipLedgerError::RecoveryRequired => {
            DecideDeviceTrustChangeError::recovery_required()
        }
        MembershipLedgerError::Conflict => DecideDeviceTrustChangeError::StateChanged,
        MembershipLedgerError::Unavailable { .. } => DecideDeviceTrustChangeError::unavailable(),
    }
}
