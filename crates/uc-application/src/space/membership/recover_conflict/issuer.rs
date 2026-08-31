use std::sync::Arc;

use rand::RngCore;
use uc_core::membership::MembershipConflictPolicy;
use uc_core::ports::ClockPort;

use crate::space::membership::{
    CurrentMemberSignaturePort, MembershipLedger, MembershipLedgerError,
};

use super::{
    IssueMembershipBranchRecoveryError, IssueMembershipBranchRecoveryInput,
    IssueMembershipBranchRecoveryPort, PrepareMembershipBranchRecoveryMaterialError,
    PrepareMembershipBranchRecoveryMaterialInput, PrepareMembershipBranchRecoveryMaterialPort,
};

const RECOVERY_PACKAGE_TTL_MS: i64 = 5 * 60 * 1_000;

pub(crate) struct IssueMembershipBranchRecoveryUseCase {
    ledger: Arc<MembershipLedger>,
    material: Arc<dyn PrepareMembershipBranchRecoveryMaterialPort>,
    signatures: Arc<dyn CurrentMemberSignaturePort>,
    clock: Arc<dyn ClockPort>,
}

impl IssueMembershipBranchRecoveryUseCase {
    pub(crate) fn new(
        ledger: Arc<MembershipLedger>,
        material: Arc<dyn PrepareMembershipBranchRecoveryMaterialPort>,
        signatures: Arc<dyn CurrentMemberSignaturePort>,
        clock: Arc<dyn ClockPort>,
    ) -> Self {
        Self {
            ledger,
            material,
            signatures,
            clock,
        }
    }
}

#[async_trait::async_trait]
impl IssueMembershipBranchRecoveryPort for IssueMembershipBranchRecoveryUseCase {
    async fn issue_membership_branch_recovery(
        &self,
        input: IssueMembershipBranchRecoveryInput,
    ) -> Result<
        uc_core::membership::MembershipBranchRecoveryPackageV1,
        IssueMembershipBranchRecoveryError,
    > {
        let snapshot = self
            .ledger
            .load_verified()
            .await
            .map_err(map_ledger_error)?;
        let history = snapshot.history().ok_or_else(corrupt)?.clone();
        let record = snapshot
            .record()
            .membership_conflicts
            .get(&input.conflict_id)
            .ok_or_else(rejected)?;
        if record.local_branch_id != input.target_branch_id
            || MembershipConflictPolicy::branch_id(&history).map_err(|error| {
                IssueMembershipBranchRecoveryError::Corrupt {
                    source: anyhow::Error::new(error),
                }
            })? != input.target_branch_id
        {
            return Err(rejected());
        }
        history
            .admission_facts_for(input.recipient_member)
            .filter(|facts| facts.device_id == input.source_device_id)
            .filter(|_| history.active_members().contains(&input.recipient_member))
            .ok_or_else(rejected)?;
        let local_device_id = snapshot
            .record()
            .local_device_id
            .as_ref()
            .ok_or_else(corrupt)?;
        let authorizing_member = snapshot
            .record()
            .local_member_instance
            .ok_or_else(corrupt)?;
        if !history.active_members().contains(&authorizing_member)
            || history
                .admission_facts_for(authorizing_member)
                .is_none_or(|facts| &facts.device_id != local_device_id)
        {
            return Err(corrupt());
        }
        let prepared = self
            .material
            .prepare_membership_branch_recovery_material(
                PrepareMembershipBranchRecoveryMaterialInput {
                    conflict_id: input.conflict_id,
                    target_branch_id: input.target_branch_id,
                    recipient_member: input.recipient_member,
                    target_history: history.clone(),
                },
            )
            .await
            .map_err(map_material_error)?;
        let expires_at_ms = self
            .clock
            .now_ms()
            .checked_add(RECOVERY_PACKAGE_TTL_MS)
            .ok_or_else(corrupt)?;
        let mut nonce = [0; 32];
        rand::rng().fill_bytes(&mut nonce);
        if nonce == [0; 32]
            || prepared.sealed_mls_recovery_material.is_empty()
            || prepared.encrypted_content_key_catalog.is_empty()
        {
            return Err(corrupt());
        }
        let history_bytes = history.encode_persisted_v2().map_err(|error| {
            IssueMembershipBranchRecoveryError::Corrupt {
                source: anyhow::Error::new(error),
            }
        })?;
        let unsigned = uc_core::membership::MembershipBranchRecoveryPackageV1::new_unsigned(
            input.conflict_id,
            input.target_branch_id,
            input.recipient_member,
            authorizing_member,
            expires_at_ms,
            nonce,
            history_bytes,
            prepared.sealed_mls_recovery_material,
            prepared.encrypted_content_key_catalog,
        )
        .map_err(|error| IssueMembershipBranchRecoveryError::Corrupt {
            source: anyhow::Error::new(error),
        })?;
        let signature = self
            .signatures
            .sign_current_member_payload(&unsigned.authorization_signing_payload())
            .await
            .map_err(|error| IssueMembershipBranchRecoveryError::Unavailable {
                source: anyhow::Error::new(error),
            })?;
        Ok(unsigned.with_authorization_signature(signature))
    }
}

fn map_ledger_error(error: MembershipLedgerError) -> IssueMembershipBranchRecoveryError {
    match error {
        MembershipLedgerError::Locked | MembershipLedgerError::Unavailable => {
            IssueMembershipBranchRecoveryError::Unavailable {
                source: anyhow::Error::new(error),
            }
        }
        MembershipLedgerError::Conflict => rejected_with(error),
        MembershipLedgerError::Corrupt | MembershipLedgerError::RecoveryRequired => {
            IssueMembershipBranchRecoveryError::Corrupt {
                source: anyhow::Error::new(error),
            }
        }
    }
}

fn map_material_error(
    error: PrepareMembershipBranchRecoveryMaterialError,
) -> IssueMembershipBranchRecoveryError {
    match error {
        PrepareMembershipBranchRecoveryMaterialError::Unavailable { .. } => {
            IssueMembershipBranchRecoveryError::Unavailable {
                source: anyhow::Error::new(error),
            }
        }
        PrepareMembershipBranchRecoveryMaterialError::Invalid { .. } => {
            IssueMembershipBranchRecoveryError::Corrupt {
                source: anyhow::Error::new(error),
            }
        }
    }
}

fn rejected() -> IssueMembershipBranchRecoveryError {
    rejected_with(MembershipLedgerError::Conflict)
}

fn rejected_with(error: MembershipLedgerError) -> IssueMembershipBranchRecoveryError {
    IssueMembershipBranchRecoveryError::Rejected {
        source: anyhow::Error::new(error),
    }
}

fn corrupt() -> IssueMembershipBranchRecoveryError {
    IssueMembershipBranchRecoveryError::Corrupt {
        source: anyhow::Error::new(MembershipLedgerError::Corrupt),
    }
}
