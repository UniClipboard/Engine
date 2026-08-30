use std::sync::Arc;

use uc_core::membership::{
    MembershipDecisionStoreOutcome, MembershipHistoryRelationship, MembershipOperationV2,
    RemovalDecision,
};

use crate::space::membership::QueryDeviceTrustUseCase;
use crate::space::membership::RecoverMembershipEffectsPort;
use crate::space::membership::WakeSpaceMembershipMaintenancePort;
use crate::space::membership::{CurrentMemberSignatureError, CurrentMemberSignaturePort};
use crate::space::membership::{
    MembershipEffectKind, MembershipEffectPhase, MembershipLedger, MembershipLedgerError,
    PendingMembershipEffect, RestrictedMembershipDelivery,
};

use super::{
    DecideDeviceTrustChange, DecideDeviceTrustChangeError, DecideDeviceTrustChangeResult,
    DeviceTrustChangeChoice,
};

pub(crate) struct DecideDeviceTrustChangeUseCase {
    ledger: Arc<MembershipLedger>,
    signer: Arc<dyn CurrentMemberSignaturePort>,
    query: Arc<QueryDeviceTrustUseCase>,
    effects: Arc<dyn RecoverMembershipEffectsPort>,
    maintenance: Arc<dyn WakeSpaceMembershipMaintenancePort>,
    execution_lock: tokio::sync::Mutex<()>,
}

impl DecideDeviceTrustChangeUseCase {
    pub(crate) fn new(
        ledger: Arc<MembershipLedger>,
        signer: Arc<dyn CurrentMemberSignaturePort>,
        query: Arc<QueryDeviceTrustUseCase>,
        effects: Arc<dyn RecoverMembershipEffectsPort>,
        maintenance: Arc<dyn WakeSpaceMembershipMaintenancePort>,
    ) -> Self {
        Self {
            ledger,
            signer,
            query,
            effects,
            maintenance,
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
                            .map_err(|_| DecideDeviceTrustChangeError::Unavailable)?;
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
        let snapshot = self
            .ledger
            .load_verified()
            .await
            .map_err(map_ledger_error)?;
        let history = snapshot
            .history()
            .ok_or(DecideDeviceTrustChangeError::RecoveryRequired)?;
        let local_member = snapshot
            .record()
            .local_member_instance
            .ok_or(DecideDeviceTrustChangeError::RecoveryRequired)?;
        if let Some(completed) = history.decision_for(input.change_id, local_member) {
            let choice = match completed.decision {
                RemovalDecision::Accept => DeviceTrustChangeChoice::ApplyChange,
                RemovalDecision::Reject => DeviceTrustChangeChoice::KeepCurrentDeviceGroup,
            };
            let _ = self.effects.recover_membership_effects().await;
            self.maintenance.wake();
            let status = self
                .query
                .execute()
                .await
                .map_err(|_| DecideDeviceTrustChangeError::CommittedButPending)?;
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
                .map_err(|_| DecideDeviceTrustChangeError::Unavailable)?;
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
            .ok_or(DecideDeviceTrustChangeError::RecoveryRequired)?;
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
                .map_err(|_| DecideDeviceTrustChangeError::Unavailable)?;
            return Ok(DecideDeviceTrustChangeResult::LocalConfirmationRequired {
                change_id: input.change_id,
                status,
            });
        }
        let local_device_id = snapshot
            .record()
            .local_device_id
            .as_ref()
            .ok_or(DecideDeviceTrustChangeError::RecoveryRequired)?;
        let credential = self
            .signer
            .current_membership_credential(local_device_id)
            .await
            .map_err(map_signature_error)?;
        if credential.member_instance_id(local_device_id) != local_member {
            return Err(DecideDeviceTrustChangeError::RecoveryRequired);
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
            .map_err(|_| DecideDeviceTrustChangeError::RecoveryRequired)?;
        decision.signature = self
            .signer
            .sign_current_member_payload(&decision.signing_payload())
            .await
            .map_err(map_signature_error)?;
        let proposed_by_device_id = history
            .admission_facts_for(event.author_member_instance_id)
            .map(|facts| facts.device_id.clone())
            .ok_or(DecideDeviceTrustChangeError::RecoveryRequired)?;
        let target_member = match &event.operation {
            MembershipOperationV2::RemoveDevice { member } => *member,
            MembershipOperationV2::AddDevice { .. } => {
                return Err(DecideDeviceTrustChangeError::RecoveryRequired);
            }
        };
        let target_device_id = history
            .admission_facts_for(target_member)
            .map(|facts| facts.device_id.clone())
            .ok_or(DecideDeviceTrustChangeError::RecoveryRequired)?;
        let decision_payload = postcard::to_stdvec(&decision)
            .map_err(|_| DecideDeviceTrustChangeError::RecoveryRequired)?;
        let expected_revision = snapshot.record().revision;
        let expected_history_digest = snapshot.history_digest();
        let decision_for_commit = decision.clone();
        self.ledger
            .compare_and_commit_history(
                expected_revision,
                expected_history_digest,
                move |record, history, verifier| {
                    if history
                        .apply_signed_local_removal_decision(
                            decision_for_commit.clone(),
                            local_member,
                            verifier,
                        )
                        .map_err(|_| MembershipLedgerError::Corrupt)?
                        != MembershipDecisionStoreOutcome::Stored
                    {
                        return Err(MembershipLedgerError::Corrupt);
                    }
                    let next_relationship = match decision_choice {
                        RemovalDecision::Accept => MembershipHistoryRelationship::Consistent,
                        RemovalDecision::Reject => MembershipHistoryRelationship::Diverged,
                    };
                    record
                        .peer_reconciliation
                        .entry(proposed_by_device_id.clone())
                        .and_modify(|relationship| {
                            relationship.relationship = next_relationship;
                            relationship.restricted_delivery =
                                vec![RestrictedMembershipDelivery::Decision(
                                    decision_for_commit.clone(),
                                )];
                        })
                        .or_insert(crate::space::membership::PeerReconciliationRecord {
                            peer_device_id: proposed_by_device_id,
                            relationship: next_relationship,
                            confirmed_position: None,
                            sync_state: Default::default(),
                            restricted_delivery: vec![RestrictedMembershipDelivery::Decision(
                                decision_for_commit,
                            )],
                            updated_at_ms: 0,
                        });
                    if decision_choice == RemovalDecision::Accept {
                        record.pending_effects.insert(
                            *input.change_id.as_bytes(),
                            PendingMembershipEffect {
                                event_id: *input.change_id.as_bytes(),
                                kind: MembershipEffectKind::RemoveDevice,
                                phase: MembershipEffectPhase::Prepared,
                                affected_device_ids: vec![target_device_id],
                                payload: decision_payload,
                            },
                        );
                    }
                    Ok(())
                },
            )
            .await
            .map_err(map_ledger_error)?;
        let _ = self.effects.recover_membership_effects().await;
        self.maintenance.wake();
        let status = self
            .query
            .execute()
            .await
            .map_err(|_| DecideDeviceTrustChangeError::CommittedButPending)?;
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
        CurrentMemberSignatureError::InvalidState => DecideDeviceTrustChangeError::RecoveryRequired,
        CurrentMemberSignatureError::Unavailable | CurrentMemberSignatureError::Repository(_) => {
            DecideDeviceTrustChangeError::Unavailable
        }
    }
}

fn map_ledger_error(error: MembershipLedgerError) -> DecideDeviceTrustChangeError {
    match error {
        MembershipLedgerError::Locked => DecideDeviceTrustChangeError::Locked,
        MembershipLedgerError::Corrupt | MembershipLedgerError::RecoveryRequired => {
            DecideDeviceTrustChangeError::RecoveryRequired
        }
        MembershipLedgerError::Conflict => DecideDeviceTrustChangeError::StateChanged,
        MembershipLedgerError::Unavailable => DecideDeviceTrustChangeError::Unavailable,
    }
}
