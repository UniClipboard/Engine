use std::sync::Arc;

use sha2::{Digest, Sha256};
use uc_core::ids::{DeviceId, SpaceId};
use uc_core::membership::{
    AdmissionMemberBindingV2, MemberInstanceId, MembershipEventId, MembershipHistoryRelationship,
    MembershipHistoryV2ReceiveOutcome, VersionedMembershipHistory,
};

use crate::space::membership::{
    CurrentMemberSignatureError, CurrentMemberSignaturePort, InitiatedMembershipRemovalEffect,
    LoadedMembershipLedger, MembershipEffectKind, MembershipEffectPhase, MembershipLedger,
    MembershipLedgerError, MembershipMaintenanceStepOutcome, PeerReconciliationRecord,
    PendingMembershipEffect, QueryDeviceTrustUseCase, RecoverMembershipEffectsPort,
    RestrictedMembershipDelivery, WakeSpaceMembershipMaintenancePort,
};

use super::{
    AdmissionAbandonmentRevocationTarget, AdmissionRevocationPort, AdmissionRevocationResult,
    AdmissionRevocationTarget, MembershipCommitReceipt, RemoveSpaceMemberError,
    RemoveSpaceMemberResult,
};

pub(crate) struct RemoveSpaceMemberUseCase {
    ledger: Arc<MembershipLedger>,
    signer: Arc<dyn CurrentMemberSignaturePort>,
    query: Arc<QueryDeviceTrustUseCase>,
    effects: Arc<dyn RecoverMembershipEffectsPort>,
    maintenance: Arc<dyn WakeSpaceMembershipMaintenancePort>,
    execution_lock: tokio::sync::Mutex<()>,
}

#[derive(Clone)]
struct ExactRemovalTarget {
    space_id: SpaceId,
    member_instance_id: MemberInstanceId,
    origin: RemovalTargetOrigin,
    operation_id: [u8; 16],
}

#[derive(Clone, Copy)]
enum RemovalTargetOrigin {
    ActivationBaseline,
    Admission(MembershipEventId),
}

#[derive(Clone, Copy)]
enum ExactRemovalKind {
    Removed,
    AlreadyAbsent,
}

struct ExactRemovalCommit {
    kind: ExactRemovalKind,
    change_id: MembershipEventId,
    committed: LoadedMembershipLedger,
}

impl RemoveSpaceMemberUseCase {
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
        target_device_id: &DeviceId,
    ) -> Result<RemoveSpaceMemberResult, RemoveSpaceMemberError> {
        let _guard = self.execution_lock.lock().await;
        let target = self.resolve_device_target(target_device_id).await?;
        let committed = self.execute_exact_with_retry(target).await?;
        let _ = self.effects.recover_membership_effects().await;
        self.maintenance.wake();
        self.public_result(committed).await
    }

    async fn resolve_device_target(
        &self,
        target_device_id: &DeviceId,
    ) -> Result<ExactRemovalTarget, RemoveSpaceMemberError> {
        let snapshot = self
            .ledger
            .load_verified()
            .await
            .map_err(map_ledger_error)?;
        let record = snapshot.record();
        let history = snapshot
            .history()
            .ok_or(RemoveSpaceMemberError::RecoveryRequired)?;
        let local_device_id = record
            .local_device_id
            .as_ref()
            .ok_or(RemoveSpaceMemberError::RecoveryRequired)?;
        if target_device_id == local_device_id {
            return Err(RemoveSpaceMemberError::SelfTarget);
        }
        let member_instance_id = history
            .effective_member_for_device(target_device_id)
            .ok_or(RemoveSpaceMemberError::TargetNotFound)?;
        let origin = match history.admission_event_id_for(member_instance_id) {
            Some(event_id) => RemovalTargetOrigin::Admission(event_id),
            None => RemovalTargetOrigin::ActivationBaseline,
        };
        Ok(ExactRemovalTarget {
            space_id: SpaceId::from_str(history.lineage_id()),
            member_instance_id,
            origin,
            operation_id: uuid::Uuid::new_v4().into_bytes(),
        })
    }

    async fn execute_exact_with_retry(
        &self,
        target: ExactRemovalTarget,
    ) -> Result<ExactRemovalCommit, RemoveSpaceMemberError> {
        match self.execute_exact_once(target.clone()).await {
            Err(RemoveSpaceMemberError::StateChanged) => self.execute_exact_once(target).await,
            result => result,
        }
    }

    async fn execute_exact_once(
        &self,
        target: ExactRemovalTarget,
    ) -> Result<ExactRemovalCommit, RemoveSpaceMemberError> {
        let snapshot = self
            .ledger
            .load_verified()
            .await
            .map_err(map_ledger_error)?;
        let record = snapshot.record();
        let history = snapshot
            .history()
            .ok_or(RemoveSpaceMemberError::RecoveryRequired)?;
        let local_device_id = record
            .local_device_id
            .as_ref()
            .ok_or(RemoveSpaceMemberError::RecoveryRequired)?;
        let local_member = record
            .local_member_instance
            .ok_or(RemoveSpaceMemberError::RecoveryRequired)?;
        if !record.local_join_active || !history.active_members().contains(&local_member) {
            return Err(RemoveSpaceMemberError::LocalMemberRemoved);
        }
        if local_member == target.member_instance_id {
            return Err(RemoveSpaceMemberError::SelfTarget);
        }
        if !target.matches(history) {
            return Err(RemoveSpaceMemberError::TargetNotFound);
        }
        if !history
            .effective_members()
            .contains(&target.member_instance_id)
        {
            let change_id = history
                .removal_event_id_for(target.member_instance_id)
                .ok_or(RemoveSpaceMemberError::TargetNotFound)?;
            return Ok(ExactRemovalCommit {
                kind: ExactRemovalKind::AlreadyAbsent,
                change_id,
                committed: record.clone(),
            });
        }
        let target_device_id = history
            .admission_facts_for(target.member_instance_id)
            .map(|facts| facts.device_id.clone())
            .ok_or(RemoveSpaceMemberError::TargetNotFound)?;
        let credential = self
            .signer
            .current_membership_credential(local_device_id)
            .await
            .map_err(map_signature_error)?;
        if credential.member_instance_id(local_device_id) != local_member {
            return Err(RemoveSpaceMemberError::RecoveryRequired);
        }
        let history_digest = history
            .current_position()
            .map_err(|_| RemoveSpaceMemberError::RecoveryRequired)?
            .history_digest;
        let mut event = history
            .create_unsigned_local_removal_event(
                local_member,
                &credential,
                target.member_instance_id,
                target.operation_id,
                history_digest,
            )
            .map_err(|_| RemoveSpaceMemberError::RecoveryRequired)?;
        event.signature = self
            .signer
            .sign_current_member_payload(&event.signing_payload())
            .await
            .map_err(map_signature_error)?;
        let change_id = event.event_id();
        let event_for_commit = event.clone();
        let target_for_commit = target;
        let target_device_for_commit = target_device_id.clone();
        let expected_revision = record.revision;
        let expected_history_digest = snapshot.history_digest();
        let (committed, ()) = self
            .ledger
            .compare_and_commit_history(
                expected_revision,
                expected_history_digest,
                move |record, history, verifier| {
                    if !target_for_commit.matches(history)
                        || history
                            .verify_and_receive_event(event_for_commit.clone(), verifier)
                            .map_err(|_| MembershipLedgerError::Corrupt)?
                            != MembershipHistoryV2ReceiveOutcome::Applied
                    {
                        return Err(MembershipLedgerError::Corrupt);
                    }
                    let retained_device_ids = history
                        .effective_members()
                        .into_iter()
                        .filter(|member| {
                            *member != target_for_commit.member_instance_id
                                && *member != local_member
                        })
                        .filter_map(|member| history.admission_facts_for(member))
                        .map(|facts| facts.device_id.clone())
                        .collect::<Vec<_>>();
                    let effect_payload = postcard::to_stdvec(&InitiatedMembershipRemovalEffect {
                        event: event_for_commit.clone(),
                        retained_device_ids,
                    })
                    .map_err(|_| MembershipLedgerError::Corrupt)?;
                    record.effect_journal.insert(
                        *change_id.as_bytes(),
                        PendingMembershipEffect {
                            event_id: *change_id.as_bytes(),
                            kind: MembershipEffectKind::RemoveDevice,
                            phase: MembershipEffectPhase::Prepared,
                            affected_device_ids: vec![target_device_for_commit.clone()],
                            payload: effect_payload,
                        },
                    );
                    record
                        .peer_reconciliation
                        .entry(target_device_for_commit.clone())
                        .and_modify(|relationship| {
                            relationship.relationship =
                                MembershipHistoryRelationship::PendingRemovalDecision;
                            relationship.restricted_delivery =
                                vec![RestrictedMembershipDelivery::Event(
                                    event_for_commit.clone(),
                                )];
                        })
                        .or_insert(PeerReconciliationRecord {
                            peer_device_id: target_device_for_commit,
                            relationship: MembershipHistoryRelationship::PendingRemovalDecision,
                            confirmed_position: None,
                            sync_state: Default::default(),
                            restricted_delivery: vec![RestrictedMembershipDelivery::Event(
                                event_for_commit,
                            )],
                            updated_at_ms: 0,
                        });
                    Ok(())
                },
            )
            .await
            .map_err(map_ledger_error)?;

        Ok(ExactRemovalCommit {
            kind: ExactRemovalKind::Removed,
            change_id,
            committed,
        })
    }

    async fn public_result(
        &self,
        committed: ExactRemovalCommit,
    ) -> Result<RemoveSpaceMemberResult, RemoveSpaceMemberError> {
        let change_id = committed.change_id;
        let status = self
            .query
            .execute()
            .await
            .map_err(|_| RemoveSpaceMemberError::CommittedButPending { change_id })?;
        let committed_history = committed
            .committed
            .membership_history
            .as_deref()
            .ok_or(RemoveSpaceMemberError::CommittedButPending { change_id })?;
        let history_digest = <[u8; 32]>::from(Sha256::digest(committed_history));
        Ok(RemoveSpaceMemberResult {
            change_id,
            commit: MembershipCommitReceipt {
                revision: committed.committed.revision,
                history_digest,
            },
            status,
        })
    }

    async fn execute_admission_revocation(
        &self,
        target: AdmissionRevocationTarget,
    ) -> Result<AdmissionRevocationResult, RemoveSpaceMemberError> {
        let binding = target.member_binding();
        let exact = ExactRemovalTarget {
            space_id: binding.space_id().clone(),
            member_instance_id: binding.member_instance_id(),
            origin: RemovalTargetOrigin::Admission(binding.add_event_id()),
            operation_id: admission_revocation_operation_id(&target),
        };
        let committed = self.execute_exact_with_retry(exact).await?;
        let effects = self.effects.recover_membership_effects().await;
        self.maintenance.wake();
        if effects != MembershipMaintenanceStepOutcome::Completed {
            return Ok(AdmissionRevocationResult::LocalEffectsPending {
                change_id: committed.change_id,
            });
        }
        Ok(match committed.kind {
            ExactRemovalKind::Removed => AdmissionRevocationResult::Removed {
                change_id: committed.change_id,
            },
            ExactRemovalKind::AlreadyAbsent => AdmissionRevocationResult::AlreadyAbsent {
                change_id: committed.change_id,
            },
        })
    }
}

#[async_trait::async_trait]
impl AdmissionRevocationPort for RemoveSpaceMemberUseCase {
    async fn revoke_admission(
        &self,
        target: AdmissionRevocationTarget,
    ) -> Result<AdmissionRevocationResult, RemoveSpaceMemberError> {
        let _guard = self.execution_lock.lock().await;
        self.execute_admission_revocation(target).await
    }

    async fn revoke_abandoned_admission(
        &self,
        target: AdmissionAbandonmentRevocationTarget,
    ) -> Result<AdmissionRevocationResult, RemoveSpaceMemberError> {
        let _guard = self.execution_lock.lock().await;
        let snapshot = self
            .ledger
            .load_verified()
            .await
            .map_err(map_ledger_error)?;
        let history = snapshot
            .history()
            .ok_or(RemoveSpaceMemberError::RecoveryRequired)?;
        let binding = AdmissionMemberBindingV2::new(
            target.attempt_digest(),
            SpaceId::from_str(history.lineage_id()),
            target.member_instance_id(),
            target.add_event_id(),
        )
        .map_err(|_| RemoveSpaceMemberError::RecoveryRequired)?;
        self.execute_admission_revocation(AdmissionRevocationTarget::new(
            target.admission_id(),
            binding,
        ))
        .await
    }
}

impl ExactRemovalTarget {
    fn matches(&self, history: &VersionedMembershipHistory) -> bool {
        if history.lineage_id() != self.space_id.as_ref()
            || history
                .admission_facts_for(self.member_instance_id)
                .is_none()
        {
            return false;
        }
        match self.origin {
            RemovalTargetOrigin::ActivationBaseline => history
                .admission_event_id_for(self.member_instance_id)
                .is_none(),
            RemovalTargetOrigin::Admission(add_event_id) => {
                history.admission_event_id_for(self.member_instance_id) == Some(add_event_id)
            }
        }
    }
}

fn admission_revocation_operation_id(target: &AdmissionRevocationTarget) -> [u8; 16] {
    let mut hasher = Sha256::new();
    hasher.update(b"uniclipboard/admission-revocation-operation/v1\0");
    hasher.update(target.admission_id().as_bytes());
    hasher.update(target.member_binding().digest());
    let digest = hasher.finalize();
    let mut operation_id = [0; 16];
    operation_id.copy_from_slice(&digest[..16]);
    operation_id
}

fn map_ledger_error(error: MembershipLedgerError) -> RemoveSpaceMemberError {
    match error {
        MembershipLedgerError::Locked => RemoveSpaceMemberError::Locked,
        MembershipLedgerError::Conflict => RemoveSpaceMemberError::StateChanged,
        MembershipLedgerError::Corrupt | MembershipLedgerError::RecoveryRequired => {
            RemoveSpaceMemberError::RecoveryRequired
        }
        MembershipLedgerError::Unavailable => RemoveSpaceMemberError::Unavailable,
    }
}

fn map_signature_error(error: CurrentMemberSignatureError) -> RemoveSpaceMemberError {
    match error {
        CurrentMemberSignatureError::InvalidState => RemoveSpaceMemberError::RecoveryRequired,
        CurrentMemberSignatureError::Unavailable | CurrentMemberSignatureError::Repository(_) => {
            RemoveSpaceMemberError::Unavailable
        }
    }
}
