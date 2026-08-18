use std::sync::Arc;

use async_trait::async_trait;
use tracing::info;
use uc_core::ids::DeviceId;
use uc_core::membership::{
    GroupBootstrapPort, LegacyProtectionCommand, LegacyProtectionPort, LegacyProtectionResult,
    LegacyProtectionSnapshot, LegacyRequestInspection, LegacyUpgradeDescriptor, LegacyUpgradeError,
    LegacyUpgradeRequest, MemberProtectionStatus, ProtectionGroupId, SpaceProtectionStatusPort,
    SpaceSecurityMode,
};
use uc_core::space_access::PreparedGroupJoin;

use super::mls_group::{MlsClientState, MlsGroupEngine};
use super::{DefaultSpaceAccessAdapter, InMemorySession};

use self::proof::{request_id, request_transcript};

pub(crate) mod proof;

pub(crate) struct PreparedLegacyUpgradeAttempt {
    request: LegacyUpgradeRequest,
    pending_group_join: PreparedGroupJoin,
}

impl PreparedLegacyUpgradeAttempt {
    pub(crate) fn new(
        request: LegacyUpgradeRequest,
        pending_group_join: PreparedGroupJoin,
    ) -> Self {
        Self {
            request,
            pending_group_join,
        }
    }

    pub(crate) fn request(&self) -> &LegacyUpgradeRequest {
        &self.request
    }

    pub(crate) fn pending_group_join(&self) -> &PreparedGroupJoin {
        &self.pending_group_join
    }

    pub(crate) fn into_parts(self) -> (LegacyUpgradeRequest, PreparedGroupJoin) {
        (self.request, self.pending_group_join)
    }
}

#[async_trait]
pub(crate) trait LegacyUpgradeAttemptStore: Send + Sync {
    async fn save_pending_attempt(
        &self,
        peer: &DeviceId,
        pending: &PreparedLegacyUpgradeAttempt,
        now_ms: i64,
    ) -> Result<(), LegacyUpgradeError>;

    async fn load_pending_attempt(
        &self,
        peer: &DeviceId,
    ) -> Result<Option<PreparedLegacyUpgradeAttempt>, LegacyUpgradeError>;

    async fn clear_pending_attempt(&self, peer: &DeviceId) -> Result<(), LegacyUpgradeError>;
}

pub struct DefaultLegacyProtection {
    space_access: Arc<DefaultSpaceAccessAdapter>,
    attempt_store: Arc<dyn LegacyUpgradeAttemptStore>,
}

impl DefaultLegacyProtection {
    pub(crate) fn new(
        space_access: Arc<DefaultSpaceAccessAdapter>,
        attempt_store: Arc<dyn LegacyUpgradeAttemptStore>,
    ) -> Self {
        Self {
            space_access,
            attempt_store,
        }
    }

    fn current_space_id(&self) -> Result<uc_core::ids::SpaceId, LegacyUpgradeError> {
        self.space_access
            .session
            .current_space_id()
            .map_err(|_| LegacyUpgradeError::Unavailable)
    }

    async fn descriptor(&self) -> Result<LegacyUpgradeDescriptor, LegacyUpgradeError> {
        let space_id = self.current_space_id()?;
        let upgrade_id = self
            .space_access
            .session
            .legacy_upgrade_id()
            .map_err(|error| LegacyUpgradeError::Internal(error.to_string()))?;
        let repository = self
            .space_access
            .key_epoch_repository
            .as_ref()
            .ok_or(LegacyUpgradeError::Unavailable)?;
        let material = repository
            .load_space_material(&space_id)
            .await
            .map_err(|error| LegacyUpgradeError::Internal(error.to_string()))?;
        match material {
            None => Ok(LegacyUpgradeDescriptor::legacy(upgrade_id)),
            Some(material) if material.state().mode() != SpaceSecurityMode::Ready => {
                Ok(LegacyUpgradeDescriptor::legacy(upgrade_id))
            }
            Some(mut material) => {
                let protection_group_id = match material.state().protection_group_id().cloned() {
                    Some(protection_group_id) => protection_group_id,
                    None => {
                        MlsGroupEngine::validate_state(
                            &MlsClientState::from_bytes(material.group_state().to_vec()),
                            space_id.as_ref().as_bytes(),
                        )
                        .map_err(|error| LegacyUpgradeError::Internal(error.to_string()))?;
                        let validator = InMemorySession::new();
                        validator.set_master_key_for_space(
                            space_id.clone(),
                            self.space_access
                                .session
                                .get_master_key()
                                .map_err(|error| LegacyUpgradeError::Internal(error.to_string()))?,
                        );
                        validator
                            .install_space_material(&material)
                            .map_err(|error| LegacyUpgradeError::Internal(error.to_string()))?;
                        let protection_group_id = ProtectionGroupId::generate();
                        material
                            .backfill_protection_group_id(protection_group_id.clone())
                            .map_err(|error| LegacyUpgradeError::Internal(error.to_string()))?;
                        repository
                            .save_space_material(&material)
                            .await
                            .map_err(|error| LegacyUpgradeError::Internal(error.to_string()))?;
                        self.space_access
                            .session
                            .install_space_material(&material)
                            .map_err(|error| LegacyUpgradeError::Internal(error.to_string()))?;
                        info!(
                            event = "legacy_upgrade.ready_group_id_backfilled",
                            space_id = %space_id,
                            "backfilled legacy ready protection group identity"
                        );
                        protection_group_id
                    }
                };
                Ok(LegacyUpgradeDescriptor::ready(
                    upgrade_id,
                    protection_group_id,
                ))
            }
        }
    }

    fn sign_request(
        &self,
        request: LegacyUpgradeRequest,
    ) -> Result<LegacyUpgradeRequest, LegacyUpgradeError> {
        let proof = self
            .space_access
            .session
            .legacy_upgrade_proof(&request_transcript(&request))
            .map_err(|error| LegacyUpgradeError::Internal(error.to_string()))?;
        Ok(request.with_proof(proof.to_vec()))
    }
}

#[async_trait]
impl LegacyProtectionPort for DefaultLegacyProtection {
    async fn snapshot(
        &self,
        member_ids: &[DeviceId],
    ) -> Result<LegacyProtectionSnapshot, LegacyUpgradeError> {
        let descriptor = self.descriptor().await?;
        let protection = SpaceProtectionStatusPort::query_space_protection(
            self.space_access.as_ref(),
            member_ids,
        )
        .await
        .map_err(|error| LegacyUpgradeError::Internal(error.to_string()))?;
        let mut protected_members = Vec::new();
        let mut pending_readmission_members = Vec::new();
        for member in protection.members {
            match member.status {
                MemberProtectionStatus::Protected => protected_members.push(member.device_id),
                MemberProtectionStatus::AwaitingReadmission => {
                    pending_readmission_members.push(member.device_id);
                }
                _ => {}
            }
        }
        Ok(LegacyProtectionSnapshot {
            descriptor,
            protected_members,
            pending_readmission_members,
        })
    }

    async fn begin_attempt(
        &self,
        source_device_id: &DeviceId,
        target_device_id: &DeviceId,
    ) -> Result<LegacyUpgradeRequest, LegacyUpgradeError> {
        let descriptor = self.descriptor().await?;
        if let Some(pending) = self
            .attempt_store
            .load_pending_attempt(target_device_id)
            .await?
        {
            let request = pending.request();
            if request.source_device_id() == source_device_id
                && request.target_device_id() == target_device_id
                && request.descriptor() == &descriptor
            {
                return Ok(request.clone());
            }
            self.attempt_store
                .clear_pending_attempt(target_device_id)
                .await?;
        }
        let pending_group_join = self
            .space_access
            .prepare_group_join(source_device_id)
            .await
            .map_err(|error| LegacyUpgradeError::Internal(error.to_string()))?;
        let unsigned = LegacyUpgradeRequest::unsigned(
            *source_device_id,
            *target_device_id,
            descriptor,
            pending_group_join.key_package.clone(),
        );
        let proof = self
            .space_access
            .session
            .legacy_upgrade_proof(&request_transcript(&unsigned))
            .map_err(|error| LegacyUpgradeError::Internal(error.to_string()))?;
        let request = unsigned.with_proof(proof.to_vec());
        self.attempt_store
            .save_pending_attempt(
                target_device_id,
                &PreparedLegacyUpgradeAttempt::new(request.clone(), pending_group_join),
                chrono::Utc::now().timestamp_millis(),
            )
            .await?;
        Ok(request)
    }

    async fn begin_readmission_confirmation(
        &self,
        source_device_id: &DeviceId,
        target_device_id: &DeviceId,
    ) -> Result<LegacyUpgradeRequest, LegacyUpgradeError> {
        let unsigned = LegacyUpgradeRequest::readmission_confirmation(
            *source_device_id,
            *target_device_id,
            self.descriptor().await?,
        );
        self.sign_request(unsigned)
    }

    async fn begin_readmission_probe(
        &self,
        source_device_id: &DeviceId,
        target_device_id: &DeviceId,
    ) -> Result<LegacyUpgradeRequest, LegacyUpgradeError> {
        let unsigned = LegacyUpgradeRequest::readmission_probe(
            *source_device_id,
            *target_device_id,
            self.descriptor().await?,
        );
        self.sign_request(unsigned)
    }

    async fn inspect_request(
        &self,
        request: &LegacyUpgradeRequest,
    ) -> Result<LegacyRequestInspection, LegacyUpgradeError> {
        let descriptor = self.descriptor().await?;
        if descriptor.upgrade_id() != request.descriptor().upgrade_id()
            || !self
                .space_access
                .session
                .verify_legacy_upgrade_proof(&request_transcript(request), request.proof())
                .map_err(|error| LegacyUpgradeError::Internal(error.to_string()))?
        {
            return Ok(LegacyRequestInspection::Invalid);
        }
        let space_id = self.current_space_id()?;
        let repository = self
            .space_access
            .key_epoch_repository
            .as_ref()
            .ok_or(LegacyUpgradeError::Unavailable)?;
        let cached = repository
            .load_space_material(&space_id)
            .await
            .map_err(|error| LegacyUpgradeError::Internal(error.to_string()))?
            .and_then(|material| {
                material
                    .cached_group_admission(request.source_device_id(), request_id(request))
                    .cloned()
            });
        match cached {
            Some(admission) => Ok(LegacyRequestInspection::Replay(admission)),
            None => Ok(LegacyRequestInspection::Verified),
        }
    }

    async fn execute(
        &self,
        command: LegacyProtectionCommand,
    ) -> Result<LegacyProtectionResult, LegacyUpgradeError> {
        match command {
            LegacyProtectionCommand::CreateGroup {
                sponsor,
                retained_members,
            } => {
                GroupBootstrapPort::bootstrap_legacy_space(
                    self.space_access.as_ref(),
                    &sponsor,
                    &retained_members,
                    chrono::Utc::now().timestamp_millis(),
                )
                .await
                .map_err(|error| LegacyUpgradeError::Internal(error.to_string()))?;
                Ok(LegacyProtectionResult::GroupReady(self.descriptor().await?))
            }
            LegacyProtectionCommand::AdmitMember {
                sponsor,
                existing_members,
                request,
            } => {
                let space_id = self.current_space_id()?;
                let (_, admission) = self
                    .space_access
                    .admit_group_member_with_replay(
                        &space_id,
                        &sponsor,
                        request.source_device_id(),
                        &existing_members,
                        request.key_package(),
                        Some((*request.source_device_id(), request_id(&request))),
                    )
                    .await
                    .map_err(|error| LegacyUpgradeError::Internal(error.to_string()))?;
                let admission = admission.ok_or_else(|| {
                    LegacyUpgradeError::Internal("legacy upgrade admission was not cached".into())
                })?;
                Ok(LegacyProtectionResult::MemberAdmitted(admission))
            }
            LegacyProtectionCommand::JoinGroup { peer, admission } => {
                let pending = self
                    .attempt_store
                    .load_pending_attempt(&peer)
                    .await?
                    .ok_or(LegacyUpgradeError::InvalidRequest)?;
                let (_, pending_group_join) = pending.into_parts();
                let space_id = self.current_space_id()?;
                let repository = self
                    .space_access
                    .key_epoch_repository
                    .as_ref()
                    .ok_or(LegacyUpgradeError::Unavailable)?;
                let incoming = self
                    .space_access
                    .complete_group_join_material(
                        &space_id,
                        pending_group_join,
                        &admission.admission.welcome,
                        &admission.admission.encrypted_key_catalog,
                        admission.admission.group_epoch,
                    )
                    .map_err(|error| LegacyUpgradeError::Internal(error.to_string()))?;
                if incoming.state().protection_group_id() != Some(&admission.protection_group_id) {
                    return Err(LegacyUpgradeError::InvalidRequest);
                }
                let material = match repository
                    .load_space_material(&space_id)
                    .await
                    .map_err(|error| LegacyUpgradeError::Internal(error.to_string()))?
                {
                    Some(previous) => self
                        .space_access
                        .session
                        .merge_space_material_history(&previous, incoming)
                        .map_err(|error| LegacyUpgradeError::Internal(error.to_string()))?,
                    None => incoming,
                };
                let validator = InMemorySession::new();
                validator.set_master_key_for_space(
                    space_id,
                    self.space_access
                        .session
                        .get_master_key()
                        .map_err(|error| LegacyUpgradeError::Internal(error.to_string()))?,
                );
                validator
                    .install_space_material(&material)
                    .map_err(|error| LegacyUpgradeError::Internal(error.to_string()))?;
                repository
                    .save_space_material(&material)
                    .await
                    .map_err(|error| LegacyUpgradeError::Internal(error.to_string()))?;
                self.space_access
                    .session
                    .install_space_material(&material)
                    .map_err(|error| LegacyUpgradeError::Internal(error.to_string()))?;
                self.attempt_store.clear_pending_attempt(&peer).await?;
                Ok(LegacyProtectionResult::GroupReady(self.descriptor().await?))
            }
            LegacyProtectionCommand::AcknowledgeReadmission { member } => {
                let space_id = self.current_space_id()?;
                self.space_access
                    .acknowledge_bootstrap_readmission_after_handoff(
                        &space_id,
                        &member,
                        chrono::Utc::now().timestamp_millis(),
                    )
                    .await?;
                Ok(LegacyProtectionResult::GroupReady(self.descriptor().await?))
            }
        }
    }
}
