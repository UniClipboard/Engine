use std::sync::Arc;

use sha2::Digest;
use uc_core::ids::DeviceId;
use uc_core::membership::{MembershipHistoryRelationship, MembershipHistoryV2ReceiveOutcome};

use crate::space::membership::QueryDeviceTrustUseCase;
use crate::space::membership::RecoverMembershipEffectsPort;
use crate::space::membership::{CurrentMemberSignatureError, CurrentMemberSignaturePort};
use crate::space::membership::{
    MembershipEffectKind, MembershipEffectPhase, MembershipLedger, MembershipLedgerError,
    PendingMembershipEffect, RestrictedMembershipDelivery,
};

use super::{
    MembershipCommitReceipt, RemoveSpaceMemberError, RemoveSpaceMemberResult,
    WakeSpaceMembershipMaintenancePort,
};

pub(crate) struct RemoveSpaceMemberUseCase {
    ledger: Arc<MembershipLedger>,
    signer: Arc<dyn CurrentMemberSignaturePort>,
    query: Arc<QueryDeviceTrustUseCase>,
    effects: Arc<dyn RecoverMembershipEffectsPort>,
    maintenance: Arc<dyn WakeSpaceMembershipMaintenancePort>,
    execution_lock: tokio::sync::Mutex<()>,
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
        match self.execute_once(target_device_id).await {
            Err(RemoveSpaceMemberError::StateChanged) => self.execute_once(target_device_id).await,
            result => result,
        }
    }

    async fn execute_once(
        &self,
        target_device_id: &DeviceId,
    ) -> Result<RemoveSpaceMemberResult, RemoveSpaceMemberError> {
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
        if target_device_id == local_device_id {
            return Err(RemoveSpaceMemberError::SelfTarget);
        }
        let target_member = history
            .effective_member_for_device(target_device_id)
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
                target_member,
                uuid::Uuid::new_v4().into_bytes(),
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
        let target_for_commit = target_device_id.clone();
        let event_payload =
            postcard::to_stdvec(&event).map_err(|_| RemoveSpaceMemberError::RecoveryRequired)?;
        let expected_revision = record.revision;
        let expected_history_digest = snapshot.history_digest();
        let (committed, ()) = self
            .ledger
            .compare_and_commit_history(
                expected_revision,
                expected_history_digest,
                move |record, history, verifier| {
                    if history
                        .verify_and_receive_event(event_for_commit.clone(), verifier)
                        .map_err(|_| MembershipLedgerError::Corrupt)?
                        != MembershipHistoryV2ReceiveOutcome::Applied
                    {
                        return Err(MembershipLedgerError::Corrupt);
                    }
                    record.pending_effects.insert(
                        *change_id.as_bytes(),
                        PendingMembershipEffect {
                            event_id: *change_id.as_bytes(),
                            kind: MembershipEffectKind::RemoveDevice,
                            phase: MembershipEffectPhase::Prepared,
                            affected_device_ids: vec![target_for_commit.clone()],
                            payload: event_payload,
                        },
                    );
                    record
                        .peer_reconciliation
                        .entry(target_for_commit.clone())
                        .and_modify(|relationship| {
                            relationship.relationship =
                                MembershipHistoryRelationship::PendingRemovalDecision;
                            relationship.restricted_delivery =
                                vec![RestrictedMembershipDelivery::Event(
                                    event_for_commit.clone(),
                                )];
                        })
                        .or_insert(crate::space::membership::PeerReconciliationRecord {
                            peer_device_id: target_for_commit,
                            relationship: MembershipHistoryRelationship::PendingRemovalDecision,
                            confirmed_position: None,
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

        let _ = self.effects.recover_membership_effects().await;
        self.maintenance.wake();
        let status = self
            .query
            .execute()
            .await
            .map_err(|_| RemoveSpaceMemberError::CommittedButPending { change_id })?;
        let committed_history = committed
            .membership_history
            .as_deref()
            .ok_or(RemoveSpaceMemberError::CommittedButPending { change_id })?;
        let history_digest = <[u8; 32]>::from(sha2::Sha256::digest(committed_history));
        Ok(RemoveSpaceMemberResult {
            change_id,
            commit: MembershipCommitReceipt {
                revision: committed.revision,
                history_digest,
            },
            status,
        })
    }
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
