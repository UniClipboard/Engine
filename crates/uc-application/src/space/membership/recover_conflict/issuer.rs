use std::sync::Arc;

use rand::RngCore;
use sha2::{Digest, Sha256};
use uc_core::membership::{
    MembershipBranchTransitionV1, MembershipConflictPolicy, MembershipHistoryRelationship,
};
use uc_core::ports::ClockPort;

use crate::space::membership::{
    CurrentMemberSignaturePort, MembershipBranchRecoverySession, MembershipLedger,
    MembershipLedgerError, PeerReconciliationRecord,
};

use super::{
    BeginMembershipBranchRecoveryInput, IssueMembershipBranchRecoveryError,
    IssueMembershipBranchRecoveryInput, IssueMembershipBranchRecoveryPort,
    PrepareMembershipBranchRecoveryMaterialError, PrepareMembershipBranchRecoveryMaterialInput,
    PrepareMembershipBranchRecoveryMaterialPort,
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
        let snapshot = self
            .ledger
            .load_verified()
            .await
            .map_err(map_ledger_error)?;
        let history = snapshot.history().ok_or_else(corrupt)?.clone();
        let record = snapshot
            .record()
            .membership_conflicts
            .get(&conflict_id)
            .ok_or_else(rejected)?;
        if record.local_branch_id != target_branch_id
            || MembershipConflictPolicy::branch_id(&history).map_err(|error| {
                IssueMembershipBranchRecoveryError::Corrupt {
                    source: anyhow::Error::new(error),
                }
            })? != target_branch_id
        {
            return Err(rejected());
        }
        history
            .admission_facts_for(recipient_member)
            .filter(|facts| &facts.device_id == source_device_id)
            .filter(|_| history.active_members().contains(&recipient_member))
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
        let snapshot = self
            .ledger
            .load_verified()
            .await
            .map_err(map_ledger_error)?;
        if let Some(session) = snapshot
            .record()
            .membership_branch_recovery_sessions
            .get(&transition_id)
        {
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
        self.ledger
            .compare_and_commit(move |record| {
                if record
                    .membership_branch_recovery_sessions
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
        let updated_at_ms = self.clock.now_ms();
        self.ledger
            .compare_and_commit(move |record| {
                record
                    .membership_branch_recovery_sessions
                    .get_mut(&transition_id)
                    .ok_or(MembershipLedgerError::Conflict)?
                    .commit_target()
                    .then_some(())
                    .ok_or(MembershipLedgerError::Conflict)?;
                let (_, package) = record
                    .membership_branch_recovery_sessions
                    .get(&transition_id)
                    .and_then(MembershipBranchRecoverySession::target_completion)
                    .ok_or(MembershipLedgerError::Conflict)?;
                let conflict = record
                    .membership_conflicts
                    .get_mut(&package.conflict_id())
                    .ok_or(MembershipLedgerError::Conflict)?;
                if conflict.local_branch_id != package.target_branch_id() {
                    return Err(MembershipLedgerError::Conflict);
                }
                conflict.status = crate::space::membership::MembershipConflictStatus::Completed;
                conflict.selected_branch_id = Some(package.target_branch_id());
                conflict.transition_id = Some(transition_id);
                let peer = record
                    .peer_reconciliation
                    .entry(recipient_device_id.clone())
                    .or_insert_with(|| PeerReconciliationRecord {
                        peer_device_id: recipient_device_id,
                        relationship: MembershipHistoryRelationship::Unknown,
                        confirmed_position: None,
                        sync_state: Default::default(),
                        restricted_delivery: Vec::new(),
                        updated_at_ms,
                    });
                peer.relationship = MembershipHistoryRelationship::Consistent;
                peer.updated_at_ms = updated_at_ms;
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
