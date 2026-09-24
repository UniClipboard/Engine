use std::sync::Arc;

use rand::RngCore;
use sha2::{Digest, Sha256};
use uc_core::membership::{
    LedgerInput, MembershipBranchTransitionV1, MembershipConflictPolicy, PeerEvidence,
};
use uc_core::ports::ClockPort;

use crate::space::membership::{
    ledger_error, CurrentMemberSignaturePort, MembershipBranchRecoverySession,
    MembershipLedgerError, MembershipOwner,
};

use super::{
    BeginMembershipBranchRecoveryInput, IssueMembershipBranchRecoveryError,
    IssueMembershipBranchRecoveryInput, IssueMembershipBranchRecoveryPort,
    PrepareMembershipBranchRecoveryMaterialError, PrepareMembershipBranchRecoveryMaterialInput,
    PrepareMembershipBranchRecoveryMaterialPort,
};

const RECOVERY_PACKAGE_TTL_MS: i64 = 5 * 60 * 1_000;

pub(crate) struct IssueMembershipBranchRecoveryUseCase {
    owner: Arc<MembershipOwner>,
    material: Arc<dyn PrepareMembershipBranchRecoveryMaterialPort>,
    signatures: Arc<dyn CurrentMemberSignaturePort>,
    clock: Arc<dyn ClockPort>,
}

impl IssueMembershipBranchRecoveryUseCase {
    pub(crate) fn new(
        owner: Arc<MembershipOwner>,
        material: Arc<dyn PrepareMembershipBranchRecoveryMaterialPort>,
        signatures: Arc<dyn CurrentMemberSignaturePort>,
        clock: Arc<dyn ClockPort>,
    ) -> Self {
        Self {
            owner,
            material,
            signatures,
            clock,
        }
    }

    async fn authorize(
        &self,
        source_device_id: &uc_core::ids::DeviceId,
        conflict_id: uc_core::membership::MembershipConflictId,
        target_branch_id: uc_core::membership::MembershipBranchId,
        recipient_member: uc_core::membership::MemberInstanceId,
    ) -> Result<
        (
            uc_core::membership::VersionedMembershipHistory,
            uc_core::membership::MemberInstanceId,
        ),
        IssueMembershipBranchRecoveryError,
    > {
        let view = self.owner.load().await.map_err(map_ledger_error)?;
        let space = view.space().ok_or_else(corrupt)?;
        let history = space.history().clone();
        let record = space
            .branch_recovery()
            .conflicts
            .get(&conflict_id)
            .ok_or_else(rejected)?;
        if record.local_branch_id != target_branch_id
            || !MembershipConflictPolicy::matches_persisted_branch(&history, target_branch_id)
                .map_err(|error| IssueMembershipBranchRecoveryError::Corrupt {
                    source: anyhow::Error::new(error),
                })?
        {
            return Err(rejected());
        }
        history
            .admission_facts_for(recipient_member)
            .filter(|facts| &facts.device_id == source_device_id)
            .filter(|_| history.active_members().contains(&recipient_member))
            .ok_or_else(rejected)?;
        let local_device_id = space.local_device_id();
        let authorizing_member = space.local_member();
        if !history.active_members().contains(&authorizing_member)
            || history
                .admission_facts_for(authorizing_member)
                .is_none_or(|facts| &facts.device_id != local_device_id)
        {
            return Err(corrupt());
        }
        Ok((history, authorizing_member))
    }
}

#[async_trait::async_trait]
impl IssueMembershipBranchRecoveryPort for IssueMembershipBranchRecoveryUseCase {
    async fn begin_membership_branch_recovery(
        &self,
        input: BeginMembershipBranchRecoveryInput,
    ) -> Result<Vec<u8>, IssueMembershipBranchRecoveryError> {
        self.authorize(
            &input.source_device_id,
            input.conflict_id,
            input.target_branch_id,
            input.recipient_member,
        )
        .await?;
        let group_info = self
            .material
            .export_membership_branch_recovery_group_info()
            .await
            .map_err(map_material_error)?;
        if group_info.is_empty() {
            return Err(corrupt());
        }
        Ok(group_info)
    }

    async fn issue_membership_branch_recovery(
        &self,
        input: IssueMembershipBranchRecoveryInput,
    ) -> Result<
        uc_core::membership::MembershipBranchRecoveryPackageV1,
        IssueMembershipBranchRecoveryError,
    > {
        if input.external_commit.is_empty() {
            return Err(rejected());
        }
        let (history, authorizing_member) = self
            .authorize(
                &input.source_device_id,
                input.conflict_id,
                input.target_branch_id,
                input.recipient_member,
            )
            .await?;
        let external_commit_digest = Sha256::digest(&input.external_commit).into();
        let transition_id =
            MembershipBranchTransitionV1::derive_id(input.conflict_id, input.target_branch_id);
        let view = self.owner.load().await.map_err(map_ledger_error)?;
        if let Some(session) = view.space().and_then(|space| {
            space
                .branch_recovery()
                .recovery_sessions
                .get(&transition_id)
        }) {
            if let Some((digest, package)) = session.target_completion() {
                return (digest == external_commit_digest)
                    .then(|| package.clone())
                    .ok_or_else(rejected);
            }
            if let Some((digest, staged, package)) = session.target_preparation() {
                if digest != external_commit_digest {
                    return Err(rejected());
                }
                let staged = staged.to_vec();
                let package = package.clone();
                self.commit_target_material(transition_id, input.source_device_id, staged)
                    .await?;
                return Ok(package);
            }
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
                    external_commit: input.external_commit,
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
        let package = unsigned.with_authorization_signature(signature);
        let target_staged_space_material = prepared.target_staged_space_material;
        let session = MembershipBranchRecoverySession::new_target_prepared(
            transition_id,
            input.conflict_id,
            input.target_branch_id,
            input.recipient_member,
            external_commit_digest,
            target_staged_space_material.clone(),
            package.clone(),
        )
        .ok_or_else(corrupt)?;
        self.owner
            .commit(move |draft| {
                if draft
                    .branch_recovery_mut()?
                    .recovery_sessions
                    .insert(transition_id, session)
                    .is_some()
                {
                    return Err(MembershipLedgerError::Conflict);
                }
                Ok(())
            })
            .await
            .map_err(map_ledger_error)?;
        self.commit_target_material(
            transition_id,
            input.source_device_id,
            target_staged_space_material,
        )
        .await?;
        Ok(package)
    }
}

impl IssueMembershipBranchRecoveryUseCase {
    async fn commit_target_material(
        &self,
        transition_id: [u8; 32],
        recipient_device_id: uc_core::ids::DeviceId,
        target_staged_space_material: Vec<u8>,
    ) -> Result<(), IssueMembershipBranchRecoveryError> {
        self.material
            .commit_membership_branch_recovery_material(target_staged_space_material)
            .await
            .map_err(map_material_error)?;
        self.owner
            .commit(move |draft| {
                let record = draft.branch_recovery_mut()?;
                record
                    .recovery_sessions
                    .get_mut(&transition_id)
                    .ok_or(MembershipLedgerError::Conflict)?
                    .commit_target()
                    .then_some(())
                    .ok_or(MembershipLedgerError::Conflict)?;
                let (_, package) = record
                    .recovery_sessions
                    .get(&transition_id)
                    .and_then(MembershipBranchRecoverySession::target_completion)
                    .ok_or(MembershipLedgerError::Conflict)?;
                let conflict_id = package.conflict_id();
                let target_branch_id = package.target_branch_id();
                let conflict = record
                    .conflicts
                    .get_mut(&conflict_id)
                    .ok_or(MembershipLedgerError::Conflict)?;
                if conflict.local_branch_id != target_branch_id {
                    return Err(MembershipLedgerError::Conflict);
                }
                conflict.status = crate::space::membership::MembershipConflictStatus::Completed;
                conflict.selected_branch_id = Some(target_branch_id);
                conflict.transition_id = Some(transition_id);
                // 接收方已持有本机分支的完整恢复包，双方历史回到一致。
                draft
                    .apply(LedgerInput::PeerEvidenceReconciled {
                        source: recipient_device_id,
                        history: None,
                        evidence: PeerEvidence::Consistent,
                    })
                    .map_err(ledger_error)?;
                Ok(())
            })
            .await
            .map_err(map_ledger_error)?;
        Ok(())
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
        PrepareMembershipBranchRecoveryMaterialError::SecurityState { .. } => {
            IssueMembershipBranchRecoveryError::Unavailable {
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

#[cfg(test)]
mod error_mapping_tests {
    use std::error::Error;

    use super::*;

    #[test]
    fn security_state_failure_stays_unavailable_and_preserves_source() {
        let error = map_material_error(
            PrepareMembershipBranchRecoveryMaterialError::SecurityState {
                source: anyhow::anyhow!("injected security state failure"),
            },
        );

        assert!(matches!(
            error,
            IssueMembershipBranchRecoveryError::Unavailable { .. }
        ));
        assert!(error.source().is_some());
    }
}
