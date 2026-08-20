use std::sync::Arc;

use rand::RngCore;
use sha2::{Digest, Sha256};
use uc_core::ids::DeviceId;
use uc_core::membership::{
    AdmissionActivationReceipt, AdmissionAttemptId, AdmissionAttemptRepositoryError,
    AdmissionAttemptRepositoryPort, AdmissionAttemptRoleStateV1, AdmissionAttemptV1,
    AdmissionCompletionRecoveryChallengeV1, AdmissionCompletionRecoveryResponseV1,
    AdmissionContentKeyCatalogV1, AdmissionIdentityBindingV1, AdmissionInboxRecordV1,
    AdmissionOutboxDeliveryPort, AdmissionOutboxDeliveryResultV1, AdmissionOutboxDeliveryRouteV1,
    AdmissionOutboxMessageV1, AdmissionOutboxPurposeV1, AdmissionRejectionReasonV1,
    AdmissionSecurityCommitmentV1, AdmissionSecurityTransitionInput,
    AdmissionSecurityTransitionPort, AdmissionSpaceTransitionError, AdmissionSpaceTransitionPort,
    AdmissionSpaceTransitionPreparationV2, AdmissionSpaceTransitionResultV2,
    AdmissionSpaceTransitionStepV2, AdmissionSpaceTransitionV2, AdmissionTerminalResultV1,
    CompletionHelperAdmissionStageV1, CompletionHelperAdmissionStateV1,
    HistoricalMembershipSignatureVerifier, InvitationConsumeDeliveryResultV1,
    JoinerAdmissionStageV1, JoinerAdmissionStateV1, LocalJoinStartMutationV1, MembershipEventV2,
    MembershipHistoryV2ReceiveOutcome, MembershipOperationV2, SponsorAdmissionStageV1,
    SponsorAdmissionStateV1, SupersedeAdmissionAttemptError, VersionedMembershipHistory,
};
use uc_core::ports::space::GroupAdmissionPort;
use uc_core::space_access::PreparedGroupJoin;

use super::super::{
    CurrentJoinStatus, JoinedSpace, PendingInboundMember, WorkspaceConvergenceError,
};

/// Owns durable admission progression. Network and product callers never
/// construct or advance the stored state directly.
pub(crate) struct DurableAdmissionTransaction {
    repository: Arc<dyn AdmissionAttemptRepositoryPort>,
    history_verifier: Arc<dyn HistoricalMembershipSignatureVerifier>,
    security_transition: Arc<dyn AdmissionSecurityTransitionPort>,
    space_transition: Arc<dyn AdmissionSpaceTransitionPort>,
}

pub(crate) struct DurableAdmissionProjection {
    repository: Arc<dyn AdmissionAttemptRepositoryPort>,
}

fn admission_outbox_delivery_route(
    attempt: &AdmissionAttemptV1,
    message: &AdmissionOutboxMessageV1,
) -> Result<Option<AdmissionOutboxDeliveryRouteV1>, WorkspaceConvergenceError> {
    if message.purpose != AdmissionOutboxPurposeV1::CancelRequested {
        return Ok(None);
    }
    if let (Some(event), Some(relationships)) = (
        attempt.candidate_event.as_deref(),
        attempt.target_relationships.as_deref(),
    ) {
        let event: MembershipEventV2 = postcard::from_bytes(event).map_err(admission_storage)?;
        let mut sponsors = relationships
            .iter()
            .filter(|facts| facts.member_instance == event.author_member_instance_id);
        let sponsor = sponsors.next().ok_or_else(|| {
            inconsistent("superseded join has no sponsor continuation relationship")
        })?;
        if sponsors.next().is_some() {
            return Err(inconsistent(
                "superseded join has duplicate sponsor continuation relationships",
            ));
        }
        if !sponsor.transport_address_blob.is_empty() {
            return Ok(Some(AdmissionOutboxDeliveryRouteV1::Continuation(
                sponsor.transport_address_blob.clone(),
            )));
        }
    }
    if let Some(address) = attempt
        .sponsor_continuation_address
        .as_ref()
        .filter(|address| !address.is_empty())
    {
        return Ok(Some(AdmissionOutboxDeliveryRouteV1::Continuation(
            address.clone(),
        )));
    }
    if message.recipient.is_empty() {
        return Err(inconsistent(
            "superseded join cleanup has no delivery route",
        ));
    }
    Ok(Some(AdmissionOutboxDeliveryRouteV1::Invitation(
        message.recipient.clone(),
    )))
}

impl DurableAdmissionProjection {
    pub(crate) fn new(repository: Arc<dyn AdmissionAttemptRepositoryPort>) -> Self {
        Self { repository }
    }

    pub(crate) async fn current_local_join(
        &self,
    ) -> Result<Option<CurrentJoinStatus>, WorkspaceConvergenceError> {
        let Some(projection) = self
            .repository
            .project_current_local_join()
            .await
            .map_err(map_repository_error)?
        else {
            return Ok(None);
        };
        if projection.terminal_result.is_none() {
            let attempt = self
                .repository
                .load(projection.attempt_id)
                .await
                .map_err(map_repository_error)?
                .ok_or_else(|| inconsistent("current local join attempt is missing"))?;
            let binding = attempt
                .identity_binding
                .as_deref()
                .map(AdmissionIdentityBindingV1::decode)
                .transpose()
                .map_err(|error| inconsistent(error.to_string()))?;
            return Ok(Some(CurrentJoinStatus::Pending {
                join_id: projection.join_id,
                target_space_id: attempt.lineage_id,
                sponsor_device_id: binding
                    .as_ref()
                    .map(|binding| binding.sponsor_device_id.clone()),
                sponsor_identity_fingerprint: binding
                    .map(|binding| binding.sponsor_identity_fingerprint),
                cancel_requested: attempt.cancel_request.is_some(),
            }));
        }
        match projection.terminal_result {
            Some(AdmissionTerminalResultV1::Rejected) => Ok(Some(CurrentJoinStatus::Rejected {
                join_id: projection.join_id,
                reason: projection
                    .rejection_reason
                    .ok_or_else(|| inconsistent("rejected local join reason is missing"))?,
            })),
            Some(AdmissionTerminalResultV1::Active) => {
                let terminal = self
                    .repository
                    .load_terminal(projection.attempt_id)
                    .await
                    .map_err(map_repository_error)?
                    .ok_or_else(|| inconsistent("active local join terminal is missing"))?;
                let binding = AdmissionIdentityBindingV1::decode(
                    terminal
                        .identity_binding
                        .as_deref()
                        .ok_or_else(|| inconsistent("active local join identity is missing"))?,
                )
                .map_err(|error| inconsistent(error.to_string()))?;
                let (migrated_records, preserved_unreadable_records) = terminal
                    .space_transition_result
                    .as_deref()
                    .and_then(AdmissionSpaceTransitionResultV2::decode)
                    .map(|result| match result {
                        AdmissionSpaceTransitionResultV2::CrossSpace(result) => (
                            Some(result.migrated_records),
                            Some(result.preserved_unreadable_records),
                        ),
                        AdmissionSpaceTransitionResultV2::Fresh { .. } => (None, None),
                        AdmissionSpaceTransitionResultV2::SameSpace { .. } => (Some(0), Some(0)),
                    })
                    .unwrap_or((None, None));
                Ok(Some(CurrentJoinStatus::Active {
                    join_id: projection.join_id,
                    joined_space: JoinedSpace {
                        sponsor_device_id: binding.sponsor_device_id,
                        sponsor_identity_fingerprint: binding.sponsor_identity_fingerprint,
                        space_id: binding.lineage_id,
                        self_device_id: binding.joiner_device_id,
                        self_identity_fingerprint: binding.joiner_identity_fingerprint,
                        migrated_records,
                        preserved_unreadable_records,
                    },
                }))
            }
            Some(AdmissionTerminalResultV1::Completed) => Err(inconsistent(
                "local join terminal has a sponsor-only completion result",
            )),
            Some(AdmissionTerminalResultV1::SupersededByNewJoin) => Err(inconsistent(
                "superseded local join was selected as the current join",
            )),
            None => unreachable!(),
        }
    }

    pub(crate) async fn cancel_local_join(
        &self,
        join_id: [u8; 16],
    ) -> Result<CurrentJoinStatus, WorkspaceConvergenceError> {
        let projection = self
            .repository
            .project_current_local_join()
            .await
            .map_err(map_repository_error)?
            .filter(|projection| projection.join_id == join_id)
            .ok_or(WorkspaceConvergenceError::JoinNotFound)?;
        if projection.terminal_result.is_some() {
            return self
                .current_local_join()
                .await?
                .ok_or(WorkspaceConvergenceError::JoinNotFound);
        }
        let mut attempt = self
            .repository
            .load(projection.attempt_id)
            .await
            .map_err(map_repository_error)?
            .ok_or_else(|| inconsistent("current local join attempt is missing"))?;
        if attempt.cancel_request.is_none() {
            let recipient = attempt
                .outboxes
                .iter()
                .find(|message| message.purpose == AdmissionOutboxPurposeV1::JoinRequest)
                .map(|message| message.recipient.clone())
                .ok_or_else(|| inconsistent("local join request recipient is missing"))?;
            let predecessor = attempt
                .outboxes
                .iter()
                .rev()
                .find(|message| !message.superseded)
                .map(|message| message.message_id);
            let payload = b"cancel_requested";
            attempt.cancel_request = Some(payload.to_vec());
            attempt.outboxes.push(outbound_message(
                projection.attempt_id,
                AdmissionOutboxPurposeV1::CancelRequested,
                &recipient,
                predecessor,
                payload,
            ));
            let expected = attempt.record_version;
            attempt.record_version = expected
                .checked_add(1)
                .ok_or_else(|| inconsistent("admission record version overflow"))?;
            self.repository
                .compare_and_advance(projection.attempt_id, expected, &attempt)
                .await
                .map_err(map_repository_error)?;
        }
        self.current_local_join()
            .await?
            .ok_or(WorkspaceConvergenceError::JoinNotFound)
    }

    pub(crate) async fn reset_join_projection_if_quiet(
        &self,
    ) -> Result<uc_core::membership::AdmissionProfileMetadataV1, WorkspaceConvergenceError> {
        let metadata = self
            .repository
            .profile_metadata()
            .await
            .map_err(map_repository_error)?;
        self.repository
            .advance_projection_floor(metadata.device_trust_revision)
            .await
            .map_err(|error| match error {
                AdmissionAttemptRepositoryError::VersionConflict => {
                    WorkspaceConvergenceError::Unavailable
                }
                other => map_repository_error(other),
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct DurableAdmissionCandidateV1 {
    pub lineage_id: String,
    pub base_history_position: Vec<u8>,
    pub candidate_event: Vec<u8>,
    pub candidate_event_id: [u8; 32],
    pub candidate_key_package: Vec<u8>,
    pub resume_public_key: Vec<u8>,
    pub target_members_digest: [u8; 32],
    pub security_commitment: Vec<u8>,
    pub security_commit: Vec<u8>,
    pub security_welcome: Vec<u8>,
    pub target_protection_group_id: String,
    pub target_key_catalog: Vec<u8>,
    pub target_relationships: Vec<uc_core::membership::AdmissionChangeFacts>,
    pub existing_member_deliveries: Vec<uc_core::membership::SponsorAdmissionSecurityDelivery>,
    pub staged_security_state: Vec<u8>,
    pub identity_binding: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct DurableAdmissionCandidatePayloadV1 {
    pub format_version: u16,
    pub base_membership_history: Vec<u8>,
    pub candidate: DurableAdmissionCandidateV1,
}

impl DurableAdmissionCandidatePayloadV1 {
    const FORMAT_V1: u16 = 1;

    pub(crate) fn new(
        base_membership_history: Vec<u8>,
        candidate: DurableAdmissionCandidateV1,
    ) -> Self {
        Self {
            format_version: Self::FORMAT_V1,
            base_membership_history,
            candidate,
        }
    }

    pub(crate) fn encode(&self) -> Result<Vec<u8>, WorkspaceConvergenceError> {
        postcard::to_stdvec(self).map_err(admission_storage)
    }

    pub(crate) fn decode(encoded: &[u8]) -> Result<Self, WorkspaceConvergenceError> {
        let payload: Self = postcard::from_bytes(encoded).map_err(admission_storage)?;
        if payload.format_version != Self::FORMAT_V1 {
            return Err(inconsistent("unsupported durable candidate payload"));
        }
        Ok(payload)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct CompletionRecoveryRouteV1 {
    pub member_instance: uc_core::membership::MemberInstanceId,
    pub device_id: DeviceId,
    pub transport_public_key: Vec<u8>,
    pub transport_address_blob: Vec<u8>,
}

impl From<&uc_core::membership::AdmissionChangeFacts> for CompletionRecoveryRouteV1 {
    fn from(facts: &uc_core::membership::AdmissionChangeFacts) -> Self {
        Self {
            member_instance: facts.member_instance,
            device_id: facts.device_id,
            transport_public_key: facts.transport_public_key.clone(),
            transport_address_blob: facts.transport_address_blob.clone(),
        }
    }
}

pub(crate) fn completion_recovery_routes(
    relationships: &[uc_core::membership::AdmissionChangeFacts],
) -> Vec<CompletionRecoveryRouteV1> {
    relationships
        .iter()
        .map(CompletionRecoveryRouteV1::from)
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct DurableAdmissionCommitPayloadV1 {
    pub format_version: u16,
    pub candidate_event_id: [u8; 32],
    pub security_commitment_id: [u8; 32],
    pub prepared_proof: Vec<u8>,
    pub resume_public_key: Vec<u8>,
    pub existing_member_deliveries: Vec<uc_core::membership::SponsorAdmissionSecurityDelivery>,
    pub completion_recovery_routes: Vec<CompletionRecoveryRouteV1>,
}

impl DurableAdmissionCommitPayloadV1 {
    pub(crate) const FORMAT_V1: u16 = 1;
    const MAX_COMPLETION_RECOVERY_ROUTES: usize = 256;

    pub(crate) fn encode(&self) -> Result<Vec<u8>, WorkspaceConvergenceError> {
        self.validate()?;
        postcard::to_stdvec(self).map_err(admission_storage)
    }

    pub(crate) fn decode(encoded: &[u8]) -> Result<Self, WorkspaceConvergenceError> {
        let payload: Self = postcard::from_bytes(encoded).map_err(admission_storage)?;
        payload.validate()?;
        Ok(payload)
    }

    fn validate(&self) -> Result<(), WorkspaceConvergenceError> {
        if self.format_version != Self::FORMAT_V1 {
            return Err(inconsistent("unsupported durable commit payload"));
        }
        if self.completion_recovery_routes.len() > Self::MAX_COMPLETION_RECOVERY_ROUTES {
            return Err(inconsistent("too many completion recovery routes"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DurableJoinRecoveryMaterialV1 {
    pub pending_security_state: Vec<u8>,
    pub candidate_key_package: Vec<u8>,
    pub member_instance: uc_core::membership::MemberInstanceId,
    pub resume_public_key: Vec<u8>,
    pub resume_private_key: Vec<u8>,
}

#[derive(Debug)]
pub(crate) struct DurableJoinStartV1 {
    pub attempt: AdmissionAttemptV1,
    pub prepared_group_join: PreparedGroupJoin,
}

impl DurableJoinStartV1 {
    pub(crate) fn request_message_id(&self) -> Result<[u8; 32], WorkspaceConvergenceError> {
        self.attempt
            .outboxes
            .iter()
            .find(|message| message.purpose == AdmissionOutboxPurposeV1::JoinRequest)
            .map(|message| message.message_id)
            .ok_or_else(|| inconsistent("durable join request outbox is missing"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) enum InvitationConsumeResultV1 {
    Consumed,
    NotFound,
    Conflict,
    Retryable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PendingMemberRemovalOutcomeV1 {
    AdmissionRejected(AdmissionOutboxMessageV1),
    OrdinaryMemberRemovalRequired,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct AdmissionRecoveryReportV1 {
    pub deliveries_attempted: usize,
    pub deliveries_confirmed: usize,
    pub attempts_compacted: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum JoinerActivationOutcomeV1 {
    Active(AdmissionInboxRecordV1),
    SpaceTransitionRequired,
}

pub(crate) fn verify_candidate_preparation(
    mut history: VersionedMembershipHistory,
    candidate_event: &MembershipEventV2,
    sponsor_commitment: &AdmissionSecurityCommitmentV1,
    joiner_commitment: &AdmissionSecurityCommitmentV1,
    verifier: &(impl HistoricalMembershipSignatureVerifier + ?Sized),
) -> Result<VersionedMembershipHistory, WorkspaceConvergenceError> {
    sponsor_commitment
        .validate()
        .map_err(|error| inconsistent(error.to_string()))?;
    joiner_commitment
        .validate()
        .map_err(|error| inconsistent(error.to_string()))?;
    if sponsor_commitment != joiner_commitment {
        return Err(inconsistent(
            "joiner security result does not match sponsor candidate",
        ));
    }
    if candidate_event.lineage_id != sponsor_commitment.lineage_id
        || candidate_event.parent_event_id != sponsor_commitment.base_history_position.event_id
        || candidate_event.parent_depth
            != sponsor_commitment
                .base_history_position
                .depth
                .saturating_add(1)
        || candidate_event.admission_bundle_digest
            != Some(sponsor_commitment.admission_bundle_digest)
    {
        return Err(inconsistent(
            "candidate event does not match its base history and security result",
        ));
    }
    let MembershipOperationV2::AddDevice { admission } = &candidate_event.operation else {
        return Err(inconsistent("candidate event is not an AddDevice"));
    };
    if admission.security_commitment_id != sponsor_commitment.security_commitment_id {
        return Err(inconsistent(
            "candidate event does not bind the verified security result",
        ));
    }
    match history.verify_and_receive_event(candidate_event.clone(), verifier) {
        Ok(MembershipHistoryV2ReceiveOutcome::Applied) => Ok(history),
        Ok(MembershipHistoryV2ReceiveOutcome::AlreadyKnown) => Err(inconsistent(
            "candidate event already exists in the base history",
        )),
        Ok(MembershipHistoryV2ReceiveOutcome::Diverged) => Err(inconsistent(
            "candidate event does not extend the supplied base history",
        )),
        Err(error) => Err(inconsistent(error.to_string())),
    }
}

// The versioned channel calls these transitions in the protocol-integration
// stage. Until then production startup uses only `recoverable`.
#[cfg_attr(not(test), allow(dead_code))]
impl DurableAdmissionTransaction {
    pub(crate) fn new(
        repository: Arc<dyn AdmissionAttemptRepositoryPort>,
        history_verifier: Arc<dyn HistoricalMembershipSignatureVerifier>,
        security_transition: Arc<dyn AdmissionSecurityTransitionPort>,
        space_transition: Arc<dyn AdmissionSpaceTransitionPort>,
    ) -> Self {
        Self {
            repository,
            history_verifier,
            security_transition,
            space_transition,
        }
    }

    pub(crate) async fn start_join(
        &self,
        attempt_id: AdmissionAttemptId,
        join_id: [u8; 16],
        sponsor: &[u8],
        request_payload: &[u8],
        pending_security_state: &[u8],
        candidate_key_package: &[u8],
        target_access_state: &[u8],
    ) -> Result<AdmissionAttemptV1, WorkspaceConvergenceError> {
        self.start_join_inner(
            attempt_id,
            join_id,
            sponsor,
            request_payload,
            pending_security_state,
            candidate_key_package,
            Some(target_access_state),
            None,
            false,
        )
        .await
    }

    pub(crate) async fn start_join_before_network(
        &self,
        attempt_id: AdmissionAttemptId,
        join_id: [u8; 16],
        sponsor: &[u8],
        request_payload: &[u8],
        pending_security_state: &[u8],
        candidate_key_package: &[u8],
    ) -> Result<AdmissionAttemptV1, WorkspaceConvergenceError> {
        self.start_join_inner(
            attempt_id,
            join_id,
            sponsor,
            request_payload,
            pending_security_state,
            candidate_key_package,
            None,
            None,
            false,
        )
        .await
    }

    pub(crate) async fn start_join_with_recovery_material(
        &self,
        attempt_id: AdmissionAttemptId,
        join_id: [u8; 16],
        sponsor: &[u8],
        request_payload: &[u8],
        material: &DurableJoinRecoveryMaterialV1,
    ) -> Result<AdmissionAttemptV1, WorkspaceConvergenceError> {
        self.start_join_with_recovery_material_and_policy(
            attempt_id,
            join_id,
            sponsor,
            request_payload,
            material,
            false,
        )
        .await
    }

    async fn start_join_with_recovery_material_and_policy(
        &self,
        attempt_id: AdmissionAttemptId,
        join_id: [u8; 16],
        sponsor: &[u8],
        request_payload: &[u8],
        material: &DurableJoinRecoveryMaterialV1,
        preserve_unreadable_history: bool,
    ) -> Result<AdmissionAttemptV1, WorkspaceConvergenceError> {
        self.start_join_inner(
            attempt_id,
            join_id,
            sponsor,
            request_payload,
            &material.pending_security_state,
            &material.candidate_key_package,
            None,
            Some(material),
            preserve_unreadable_history,
        )
        .await
    }

    pub(crate) async fn preflight_join_source(
        &self,
        preserve_unreadable_history: bool,
    ) -> Result<(), WorkspaceConvergenceError> {
        self.space_transition
            .preflight_source_history(preserve_unreadable_history)
            .await
            .map_err(map_space_transition_error)?;
        if self
            .current_pending_join()
            .await?
            .as_ref()
            .and_then(AdmissionAttemptV1::stage_rank)
            .is_some_and(|rank| rank >= 3)
        {
            return Err(WorkspaceConvergenceError::PreviousJoinCannotBeSuperseded);
        }
        Ok(())
    }

    pub(crate) async fn prepare_join_before_network(
        &self,
        preparation: &(impl GroupAdmissionPort + ?Sized),
        local_device_id: &DeviceId,
        sponsor: &[u8],
        sponsor_continuation_address: &[u8],
        request_payload: &[u8],
        preserve_unreadable_history: bool,
    ) -> Result<DurableJoinStartV1, WorkspaceConvergenceError> {
        self.preflight_join_source(preserve_unreadable_history)
            .await?;
        loop {
            let existing = self.current_pending_join().await?;
            if existing
                .as_ref()
                .and_then(AdmissionAttemptV1::stage_rank)
                .is_some_and(|rank| rank >= 3)
            {
                return Err(WorkspaceConvergenceError::PreviousJoinCannotBeSuperseded);
            }
            let metadata = self
                .repository
                .profile_metadata()
                .await
                .map_err(map_repository_error)?;
            let prepared_group_join = preparation
                .prepare_group_join(local_device_id)
                .await
                .map_err(|error| admission_storage(error.to_string()))?;
            let member_instance = prepared_group_join
                .member_instance()
                .ok_or_else(|| inconsistent("prepared join member instance is missing"))?;
            let mut resume_private_key = [0u8; 32];
            while resume_private_key == [0; 32] {
                rand::rng().fill_bytes(&mut resume_private_key);
            }
            let signing_key = ed25519_dalek::SigningKey::from_bytes(&resume_private_key);
            let mut attempt_bytes = [0u8; 32];
            let mut join_id = [0u8; 16];
            while attempt_bytes == [0; 32] {
                rand::rng().fill_bytes(&mut attempt_bytes);
            }
            while join_id == [0; 16] {
                rand::rng().fill_bytes(&mut join_id);
            }
            let attempt_id = AdmissionAttemptId::from_bytes(attempt_bytes);
            let mut replacement = AdmissionAttemptV1::new_joiner(
                attempt_id,
                join_id,
                JoinerAdmissionStageV1::Initiated,
            );
            replacement.local_join_ordinal = Some(metadata.next_local_join_ordinal);
            replacement.joiner_pending_security_state =
                Some(prepared_group_join.private_state().to_vec());
            replacement.candidate_key_package = Some(prepared_group_join.key_package.clone());
            replacement.joiner_member_instance = Some(member_instance);
            replacement.resume_public_key = Some(signing_key.verifying_key().to_bytes().to_vec());
            replacement.resume_private_key = Some(resume_private_key.to_vec());
            replacement.preserve_unreadable_history = preserve_unreadable_history;
            replacement.sponsor_continuation_address = (!sponsor_continuation_address.is_empty())
                .then(|| sponsor_continuation_address.to_vec());
            replacement.outboxes.push(outbound_message(
                attempt_id,
                AdmissionOutboxPurposeV1::JoinRequest,
                sponsor,
                None,
                request_payload,
            ));

            let mutation = if let Some(previous) = existing {
                let recipient = previous
                    .outboxes
                    .iter()
                    .find(|message| message.purpose == AdmissionOutboxPurposeV1::JoinRequest)
                    .map(|message| message.recipient.clone())
                    .ok_or_else(|| inconsistent("previous join request recipient is missing"))?;
                let predecessor = previous
                    .outboxes
                    .iter()
                    .rev()
                    .find(|message| !message.superseded)
                    .or_else(|| {
                        previous.outboxes.iter().rev().find(|message| {
                            message.purpose == AdmissionOutboxPurposeV1::JoinRequest
                        })
                    })
                    .map(|message| message.message_id);
                let cleanup = outbound_message(
                    previous.attempt_id,
                    AdmissionOutboxPurposeV1::CancelRequested,
                    &recipient,
                    predecessor,
                    b"cancel_requested",
                );
                let expected_previous_record_version = previous.record_version;
                let expected_previous_attempt_id = previous.attempt_id;
                let mut previous_terminal =
                    previous
                        .superseded_by_new_join(cleanup)
                        .map_err(|error| match error {
                            SupersedeAdmissionAttemptError::UnsafeStage => {
                                WorkspaceConvergenceError::PreviousJoinCannotBeSuperseded
                            }
                            SupersedeAdmissionAttemptError::RecoveryRequired => {
                                WorkspaceConvergenceError::RecoveryRequired
                            }
                            other => inconsistent(other.to_string()),
                        })?;
                previous_terminal.record_version = expected_previous_record_version
                    .checked_add(1)
                    .ok_or_else(|| inconsistent("admission record version overflow"))?;
                LocalJoinStartMutationV1::Supersede {
                    expected_previous_attempt_id,
                    expected_previous_record_version,
                    previous_terminal,
                    replacement: replacement.clone(),
                }
            } else {
                LocalJoinStartMutationV1::Create {
                    replacement: replacement.clone(),
                }
            };

            match self.repository.commit_local_join_start(mutation).await {
                Ok(_) => {
                    return Ok(DurableJoinStartV1 {
                        attempt: replacement,
                        prepared_group_join,
                    });
                }
                Err(AdmissionAttemptRepositoryError::VersionConflict) => {
                    let Some(conflicting) = self.current_pending_join().await? else {
                        return Err(WorkspaceConvergenceError::AdmissionInProgress);
                    };
                    if conflicting.stage_rank().is_some_and(|rank| rank >= 3) {
                        return Err(WorkspaceConvergenceError::PreviousJoinCannotBeSuperseded);
                    }
                    continue;
                }
                Err(AdmissionAttemptRepositoryError::PreviousJoinCannotBeSuperseded) => {
                    return Err(WorkspaceConvergenceError::PreviousJoinCannotBeSuperseded);
                }
                Err(error) => return Err(map_repository_error(error)),
            }
        }
    }

    #[cfg(test)]
    pub(crate) async fn prepare_join_before_network_without_route(
        &self,
        preparation: &(impl GroupAdmissionPort + ?Sized),
        local_device_id: &DeviceId,
        sponsor: &[u8],
        request_payload: &[u8],
        preserve_unreadable_history: bool,
    ) -> Result<DurableJoinStartV1, WorkspaceConvergenceError> {
        self.prepare_join_before_network(
            preparation,
            local_device_id,
            sponsor,
            b"test-sponsor-address",
            request_payload,
            preserve_unreadable_history,
        )
        .await
    }

    pub(crate) async fn load_join_recovery_material(
        &self,
        attempt_id: AdmissionAttemptId,
    ) -> Result<DurableJoinRecoveryMaterialV1, WorkspaceConvergenceError> {
        let attempt = self
            .load(attempt_id)
            .await?
            .ok_or(WorkspaceConvergenceError::JoinNotFound)?;
        if !attempt.is_joiner() || attempt.stage_rank() != Some(0) {
            return Err(inconsistent(
                "join recovery material is not in the initiated stage",
            ));
        }
        let material = DurableJoinRecoveryMaterialV1 {
            pending_security_state: attempt
                .joiner_pending_security_state
                .ok_or_else(|| inconsistent("join recovery private state is missing"))?,
            candidate_key_package: attempt
                .candidate_key_package
                .ok_or_else(|| inconsistent("join recovery key package is missing"))?,
            member_instance: attempt
                .joiner_member_instance
                .ok_or_else(|| inconsistent("join recovery member instance is missing"))?,
            resume_public_key: attempt
                .resume_public_key
                .ok_or_else(|| inconsistent("join recovery public key is missing"))?,
            resume_private_key: attempt
                .resume_private_key
                .ok_or_else(|| inconsistent("join recovery private key is missing"))?,
        };
        validate_join_recovery_material(&material)?;
        Ok(material)
    }

    #[allow(clippy::too_many_arguments)]
    async fn start_join_inner(
        &self,
        attempt_id: AdmissionAttemptId,
        join_id: [u8; 16],
        sponsor: &[u8],
        request_payload: &[u8],
        pending_security_state: &[u8],
        candidate_key_package: &[u8],
        target_access_state: Option<&[u8]>,
        recovery_material: Option<&DurableJoinRecoveryMaterialV1>,
        preserve_unreadable_history: bool,
    ) -> Result<AdmissionAttemptV1, WorkspaceConvergenceError> {
        if let Some(existing) = self.load(attempt_id).await? {
            if existing.preserve_unreadable_history != preserve_unreadable_history {
                return Err(WorkspaceConvergenceError::AdmissionInProgress);
            }
            return self.match_existing_start(
                existing,
                join_id,
                sponsor,
                request_payload,
                pending_security_state,
                candidate_key_package,
                target_access_state,
                recovery_material,
            );
        }

        if let Some(material) = recovery_material {
            validate_join_recovery_material(material)?;
        }

        let metadata = self
            .repository
            .profile_metadata()
            .await
            .map_err(map_repository_error)?;
        let mut attempt =
            AdmissionAttemptV1::new_joiner(attempt_id, join_id, JoinerAdmissionStageV1::Initiated);
        attempt.local_join_ordinal = Some(metadata.next_local_join_ordinal);
        attempt.joiner_pending_security_state = Some(pending_security_state.to_vec());
        attempt.candidate_key_package = Some(candidate_key_package.to_vec());
        attempt.target_access_state = target_access_state.map(ToOwned::to_owned);
        attempt.preserve_unreadable_history = preserve_unreadable_history;
        if let Some(material) = recovery_material {
            attempt.joiner_member_instance = Some(material.member_instance);
            attempt.resume_public_key = Some(material.resume_public_key.clone());
            attempt.resume_private_key = Some(material.resume_private_key.clone());
        }
        attempt.outboxes.push(outbound_message(
            attempt_id,
            AdmissionOutboxPurposeV1::JoinRequest,
            sponsor,
            None,
            request_payload,
        ));

        match self.repository.create(&attempt, None, None).await {
            Ok(_) => Ok(attempt),
            Err(AdmissionAttemptRepositoryError::AlreadyExists) => {
                let existing = self.load(attempt_id).await?.ok_or_else(|| {
                    admission_storage("admission start disappeared after conflict")
                })?;
                if existing.preserve_unreadable_history != preserve_unreadable_history {
                    return Err(WorkspaceConvergenceError::AdmissionInProgress);
                }
                self.match_existing_start(
                    existing,
                    join_id,
                    sponsor,
                    request_payload,
                    pending_security_state,
                    candidate_key_package,
                    target_access_state,
                    recovery_material,
                )
            }
            Err(AdmissionAttemptRepositoryError::VersionConflict) => {
                Err(WorkspaceConvergenceError::AdmissionInProgress)
            }
            Err(error) => Err(map_repository_error(error)),
        }
    }

    async fn current_pending_join(
        &self,
    ) -> Result<Option<AdmissionAttemptV1>, WorkspaceConvergenceError> {
        let Some(projection) = self
            .repository
            .project_current_local_join()
            .await
            .map_err(map_repository_error)?
        else {
            return Ok(None);
        };
        if projection.terminal_result.is_some() {
            return Ok(None);
        }
        self.load(projection.attempt_id).await
    }

    pub(crate) async fn sponsor_accept_and_offer(
        &self,
        attempt_id: AdmissionAttemptId,
        invitation_digest: [u8; 32],
        request: &AdmissionOutboxMessageV1,
        candidate: DurableAdmissionCandidateV1,
        base_history: VersionedMembershipHistory,
        candidate_event: &MembershipEventV2,
        sponsor_commitment: &AdmissionSecurityCommitmentV1,
        recipient: &[u8],
        payload: &[u8],
    ) -> Result<AdmissionOutboxMessageV1, WorkspaceConvergenceError> {
        if sponsor_commitment.attempt_id != *attempt_id.as_bytes() {
            return Err(inconsistent(
                "candidate security result is bound to another attempt",
            ));
        }
        require_message(
            attempt_id,
            request,
            AdmissionOutboxPurposeV1::JoinRequest,
            None,
        )?;
        let encoded_base_history = base_history
            .encode_persisted_v2()
            .map_err(|error| inconsistent(error.to_string()))?;
        let verified_history = verify_candidate_preparation(
            base_history,
            candidate_event,
            sponsor_commitment,
            sponsor_commitment,
            self.history_verifier.as_ref(),
        )?;
        require_candidate_encoding(
            &candidate,
            candidate_event,
            sponsor_commitment,
            &verified_history,
        )?;
        let encoded_history = verified_history
            .encode_persisted_v2()
            .map_err(|error| inconsistent(error.to_string()))?;
        let candidate_message = outbound_message(
            attempt_id,
            AdmissionOutboxPurposeV1::Candidate,
            recipient,
            Some(request.message_id),
            payload,
        );
        let invitation_consume = outbound_message(
            attempt_id,
            AdmissionOutboxPurposeV1::InvitationConsume,
            b"pairing-invitation-service",
            Some(request.message_id),
            &invitation_digest,
        );
        if let Some(existing) = self.load(attempt_id).await? {
            if candidate_matches(&existing, &candidate, true)
                && existing.invitation_claim.as_deref() == Some(invitation_digest.as_slice())
                && existing
                    .inbox_dedup
                    .iter()
                    .any(|record| *record == inbox_record(request))
                && existing.verified_membership_history.as_deref()
                    == Some(encoded_history.as_slice())
                && existing.base_membership_history.as_deref()
                    == Some(encoded_base_history.as_slice())
                && existing.outboxes.contains(&candidate_message)
                && existing.outboxes.contains(&invitation_consume)
            {
                return Ok(candidate_message);
            }
            return Err(inconsistent("sponsor admission replay does not match"));
        }
        let mut attempt = sponsor_candidate_attempt(
            attempt_id,
            invitation_digest,
            candidate,
            encoded_base_history.clone(),
            encoded_history,
        );
        attempt.inbox_dedup.push(inbox_record(request));
        attempt.outboxes.push(candidate_message.clone());
        attempt.outboxes.push(invitation_consume);
        self.repository
            .create(
                &attempt,
                Some(invitation_digest),
                Some(&encoded_base_history),
            )
            .await
            .map_err(map_repository_error)?;
        Ok(candidate_message)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn joiner_verify_and_prepare(
        &self,
        attempt_id: AdmissionAttemptId,
        candidate_message: &AdmissionOutboxMessageV1,
        candidate: DurableAdmissionCandidateV1,
        base_history: VersionedMembershipHistory,
        candidate_event: &MembershipEventV2,
        sponsor_commitment: &AdmissionSecurityCommitmentV1,
        target_access_state: &[u8],
        prepared_proof: &[u8],
        prepared_proof_signer: Option<&(dyn GroupAdmissionPort + Send + Sync)>,
        recipient: &[u8],
        payload: &[u8],
    ) -> Result<AdmissionOutboxMessageV1, WorkspaceConvergenceError> {
        if sponsor_commitment.attempt_id != *attempt_id.as_bytes() {
            return Err(inconsistent(
                "candidate security result is bound to another attempt",
            ));
        }
        let mut attempt = self.required_attempt(attempt_id).await?;
        if attempt.terminal_result == Some(AdmissionTerminalResultV1::SupersededByNewJoin) {
            let predecessor = candidate_message
                .predecessor_message_id
                .ok_or_else(|| inconsistent("candidate predecessor is missing"))?;
            if candidate_message.purpose != AdmissionOutboxPurposeV1::Candidate
                || !attempt
                    .outboxes
                    .iter()
                    .any(|message| message.message_id == predecessor)
            {
                return Err(inconsistent(
                    "candidate does not match the superseded local join",
                ));
            }
            let cleanup = attempt
                .outboxes
                .iter()
                .find(|message| {
                    message.purpose == AdmissionOutboxPurposeV1::CancelRequested
                        && !message.superseded
                })
                .cloned()
                .ok_or_else(|| inconsistent("superseded join cleanup is missing"))?;
            let record = inbox_record(candidate_message);
            if !attempt.inbox_dedup.contains(&record) {
                attempt.inbox_dedup.push(record);
                self.persist_advance(attempt).await?;
            }
            return Ok(cleanup);
        }
        let pending_state = attempt
            .joiner_pending_security_state
            .as_deref()
            .ok_or_else(|| inconsistent("joiner pending security state is missing"))?;
        let key_package = attempt
            .candidate_key_package
            .as_deref()
            .ok_or_else(|| inconsistent("joiner key package is missing"))?;
        if key_package != candidate.candidate_key_package {
            return Err(inconsistent(
                "candidate key package does not match the saved join material",
            ));
        }
        let transition_input = AdmissionSecurityTransitionInput {
            attempt_id: *attempt_id.as_bytes(),
            base_history_position: sponsor_commitment.base_history_position.clone(),
            candidate_core_digest: sponsor_commitment.candidate_core_digest,
            key_catalog_digest: sponsor_commitment.key_catalog_digest,
            admission_bundle_digest: sponsor_commitment.admission_bundle_digest,
        };
        let staged = self
            .security_transition
            .stage_joiner(
                pending_state,
                key_package,
                &sponsor_commitment.mls_group_id,
                &candidate.security_welcome,
                &candidate.security_commit,
                &transition_input,
            )
            .map_err(|error| inconsistent(error.to_string()))?;
        let encoded_base_history = base_history
            .encode_persisted_v2()
            .map_err(|error| inconsistent(error.to_string()))?;
        let target_lineage = base_history.lineage_id().to_owned();
        let current_history = self
            .repository
            .load_membership_history_v2()
            .await
            .map_err(map_repository_error)?;
        if let Some(current_history) = current_history.as_deref() {
            let current = VersionedMembershipHistory::decode_persisted_v2(
                current_history,
                self.history_verifier.as_ref(),
            )
            .map_err(|error| inconsistent(error.to_string()))?;
            if current.lineage_id() == target_lineage
                && current_history != encoded_base_history.as_slice()
                && !base_history.is_complete_extension_of(&current)
            {
                return Err(inconsistent(
                    "joiner current history is not a complete prefix of the candidate base history",
                ));
            }
        }
        let verified_history = verify_candidate_preparation(
            base_history,
            candidate_event,
            sponsor_commitment,
            &staged.public_commitment,
            self.history_verifier.as_ref(),
        )?;
        require_candidate_encoding(
            &candidate,
            candidate_event,
            sponsor_commitment,
            &verified_history,
        )?;
        let generated_prepared_proof;
        let prepared_proof = if let Some(signer) = prepared_proof_signer {
            let MembershipOperationV2::AddDevice { admission } = &candidate_event.operation else {
                return Err(inconsistent("admission candidate is not AddDevice"));
            };
            let mut proof = uc_core::membership::PreparedAdmissionProofV1::new(
                *attempt_id.as_bytes(),
                candidate_event.lineage_id.clone(),
                sponsor_commitment.base_history_position.clone(),
                candidate_event.event_id(),
                candidate_event.resulting_members_digest,
                sponsor_commitment.security_commitment_id,
                admission.facts.member_instance,
                admission.membership_credential.credential_id,
                Vec::new(),
            );
            let prepared_join =
                PreparedGroupJoin::new(key_package.to_vec(), pending_state.to_vec())
                    .with_member_instance(admission.facts.member_instance);
            proof.signature = signer
                .sign_prepared_join_payload(&prepared_join, &proof.signing_payload())
                .await
                .map_err(|error| admission_storage(error.to_string()))?;
            generated_prepared_proof = postcard::to_stdvec(&proof).map_err(admission_storage)?;
            generated_prepared_proof.as_slice()
        } else {
            prepared_proof
        };
        let prepared_payload = if prepared_proof_signer.is_some() {
            prepared_proof
        } else {
            payload
        };
        let encoded_history = verified_history
            .encode_persisted_v2()
            .map_err(|error| inconsistent(error.to_string()))?;
        let mut attempt = attempt;
        let prepared_message = outbound_message(
            attempt_id,
            AdmissionOutboxPurposeV1::Prepared,
            recipient,
            Some(candidate_message.message_id),
            prepared_payload,
        );
        if attempt.stage_rank().is_some_and(|rank| rank >= 3)
            && candidate_matches(&attempt, &candidate, false)
            && attempt.target_access_state.as_deref() == Some(target_access_state)
            && attempt.prepared_proof.as_deref() == Some(prepared_proof)
            && attempt.staged_security_state.as_deref() == Some(staged.staged_state.as_slice())
            && attempt.verified_membership_history.as_deref() == Some(encoded_history.as_slice())
            && attempt.base_membership_history.as_deref() == Some(encoded_base_history.as_slice())
            && attempt.outboxes.contains(&prepared_message)
        {
            return Ok(prepared_message);
        }
        require_joiner_stage(&attempt, JoinerAdmissionStageV1::Initiated)?;
        let request_id = active_outbox_id(&attempt, AdmissionOutboxPurposeV1::JoinRequest)?;
        require_message(
            attempt_id,
            candidate_message,
            AdmissionOutboxPurposeV1::Candidate,
            Some(request_id),
        )?;
        let transition = self
            .space_transition
            .prepare_if_needed(&AdmissionSpaceTransitionPreparationV2 {
                attempt_id,
                target_space_id: candidate.lineage_id.clone(),
                target_security_commitment: sponsor_commitment.clone(),
                target_membership_history: encoded_history.clone(),
                target_security_state: staged.staged_state.clone(),
                target_protection_group_id: candidate.target_protection_group_id.clone(),
                target_key_catalog: candidate.target_key_catalog.clone(),
                local_device_id: match &candidate_event.operation {
                    MembershipOperationV2::AddDevice { admission } => {
                        admission.facts.device_id.clone()
                    }
                    MembershipOperationV2::RemoveDevice { .. } => {
                        return Err(inconsistent("admission candidate is not AddDevice"));
                    }
                },
                target_relationships: candidate.target_relationships.clone(),
                relayed_group_updates: candidate
                    .existing_member_deliveries
                    .iter()
                    .map(|delivery| {
                        uc_core::membership::PendingGroupUpdate::for_admission(
                            *attempt_id.as_bytes(),
                            delivery.recipient.clone(),
                            delivery.payload.clone(),
                        )
                    })
                    .collect(),
                target_access_state: target_access_state.to_vec(),
                preserve_unreadable_history: attempt.preserve_unreadable_history,
            })
            .await
            .map_err(map_space_transition_error)?;
        if transition.attempt_id() != attempt_id
            || transition.target_space_id() != candidate.lineage_id
            || !transition.is_initial()
        {
            return Err(inconsistent(
                "prepared space transition does not match the admission candidate",
            ));
        }
        let encoded_transition = transition
            .encode()
            .ok_or_else(|| inconsistent("prepared space transition is invalid"))?;
        attempt.target_access_state = Some(target_access_state.to_vec());
        apply_candidate(&mut attempt, candidate);
        attempt.space_transition = Some(encoded_transition);
        attempt.staged_security_state = Some(staged.staged_state);
        attempt.base_membership_history = Some(encoded_base_history.clone());
        attempt.verified_membership_history = Some(encoded_history);
        attempt.prepared_proof = Some(prepared_proof.to_vec());
        attempt.role_state = AdmissionAttemptRoleStateV1::Joiner(JoinerAdmissionStateV1 {
            stage: JoinerAdmissionStageV1::Prepared,
        });
        accept_incoming(
            &mut attempt,
            candidate_message,
            &[AdmissionOutboxPurposeV1::JoinRequest],
        );
        attempt.outboxes.push(prepared_message.clone());
        self.persist_advance_with_history(
            attempt,
            current_history.as_deref(),
            &encoded_base_history,
        )
        .await?;
        Ok(prepared_message)
    }

    pub(crate) async fn record_superseded_protocol_contradiction(
        &self,
        attempt_id: AdmissionAttemptId,
        message: &AdmissionOutboxMessageV1,
    ) -> Result<(), WorkspaceConvergenceError> {
        let mut attempt = self.required_attempt(attempt_id).await?;
        if attempt.terminal_result != Some(AdmissionTerminalResultV1::SupersededByNewJoin) {
            return Err(inconsistent(
                "protocol contradiction does not target a superseded local join",
            ));
        }
        let valid = match message.purpose {
            AdmissionOutboxPurposeV1::Candidate => {
                message.predecessor_message_id.is_some_and(|predecessor| {
                    attempt.outboxes.iter().any(|outbox| {
                        outbox.purpose == AdmissionOutboxPurposeV1::JoinRequest
                            && outbox.message_id == predecessor
                    })
                })
            }
            AdmissionOutboxPurposeV1::Commit | AdmissionOutboxPurposeV1::Complete => true,
            _ => false,
        };
        if !valid || message.payload.is_empty() {
            return Err(inconsistent(
                "superseded join protocol contradiction is invalid",
            ));
        }
        let evidence = inbox_record(message);
        if !attempt
            .inbox_dedup
            .iter()
            .any(|record| record.message_id == evidence.message_id)
        {
            attempt.inbox_dedup.push(evidence);
            self.persist_advance(attempt).await?;
        }
        Ok(())
    }

    pub(crate) async fn sponsor_commit(
        &self,
        attempt_id: AdmissionAttemptId,
        prepared_message: &AdmissionOutboxMessageV1,
        prepared_proof: &[u8],
        recipient: &[u8],
        payload: &[u8],
    ) -> Result<AdmissionOutboxMessageV1, WorkspaceConvergenceError> {
        let mut attempt = self.required_attempt(attempt_id).await?;
        let commit_message = outbound_message(
            attempt_id,
            AdmissionOutboxPurposeV1::Commit,
            recipient,
            Some(prepared_message.message_id),
            payload,
        );
        if attempt.stage_rank().is_some_and(|rank| rank >= 4)
            && attempt.prepared_proof.as_deref() == Some(prepared_proof)
            && attempt.outboxes.contains(&commit_message)
        {
            return Ok(commit_message);
        }
        if matches!(
            attempt.role_state,
            AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 {
                stage: SponsorAdmissionStageV1::Rejected
            })
        ) && attempt.rejection_reason == Some(AdmissionRejectionReasonV1::BaseHistoryChanged)
        {
            return active_outbox(&attempt, AdmissionOutboxPurposeV1::Rejected);
        }
        require_sponsor_stage(&attempt, SponsorAdmissionStageV1::Candidate)?;
        let candidate_id = active_outbox_id(&attempt, AdmissionOutboxPurposeV1::Candidate)?;
        require_message(
            attempt_id,
            prepared_message,
            AdmissionOutboxPurposeV1::Prepared,
            Some(candidate_id),
        )?;
        let base_history = attempt
            .base_membership_history
            .clone()
            .ok_or_else(|| inconsistent("candidate base membership history is missing"))?;
        let current_history = self
            .repository
            .load_membership_history_v2()
            .await
            .map_err(map_repository_error)?;
        if current_history.as_deref() != Some(base_history.as_slice()) {
            return self
                .reject_base_history_changed(attempt, prepared_message, prepared_proof, recipient)
                .await;
        }
        attempt.prepared_proof = Some(prepared_proof.to_vec());
        attempt.role_state = AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 {
            stage: SponsorAdmissionStageV1::Committed,
        });
        accept_incoming(
            &mut attempt,
            prepared_message,
            &[AdmissionOutboxPurposeV1::Candidate],
        );
        attempt.outboxes.push(commit_message.clone());
        let history = attempt
            .verified_membership_history
            .clone()
            .ok_or_else(|| inconsistent("committed membership history is missing"))?;
        match self
            .persist_advance_with_history(attempt, Some(&base_history), &history)
            .await
        {
            Ok(()) => Ok(commit_message),
            Err(WorkspaceConvergenceError::AdmissionInProgress) => {
                let current_attempt = self.required_attempt(attempt_id).await?;
                if matches!(
                    current_attempt.role_state,
                    AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 {
                        stage: SponsorAdmissionStageV1::Rejected
                    })
                ) {
                    return active_outbox(&current_attempt, AdmissionOutboxPurposeV1::Rejected);
                }
                if current_attempt.stage_rank().is_some_and(|rank| rank >= 4)
                    && current_attempt.prepared_proof.as_deref() == Some(prepared_proof)
                {
                    return active_outbox(&current_attempt, AdmissionOutboxPurposeV1::Commit);
                }
                require_sponsor_stage(&current_attempt, SponsorAdmissionStageV1::Candidate)?;
                let current_history = self
                    .repository
                    .load_membership_history_v2()
                    .await
                    .map_err(map_repository_error)?;
                if current_history.as_deref() != Some(base_history.as_slice()) {
                    self.reject_base_history_changed(
                        current_attempt,
                        prepared_message,
                        prepared_proof,
                        recipient,
                    )
                    .await
                } else {
                    Err(WorkspaceConvergenceError::AdmissionInProgress)
                }
            }
            Err(error) => Err(error),
        }
    }

    pub(crate) async fn request_cancel(
        &self,
        attempt_id: AdmissionAttemptId,
        recipient: &[u8],
        payload: &[u8],
    ) -> Result<AdmissionOutboxMessageV1, WorkspaceConvergenceError> {
        let mut attempt = self.required_attempt(attempt_id).await?;
        if !attempt.is_joiner() || attempt.is_terminal() {
            return Err(inconsistent("only a pending local join can be cancelled"));
        }
        if let Some(existing) = attempt.outboxes.iter().find(|message| {
            message.purpose == AdmissionOutboxPurposeV1::CancelRequested && !message.superseded
        }) {
            return Ok(existing.clone());
        }
        let predecessor = attempt
            .outboxes
            .iter()
            .rev()
            .find(|message| !message.superseded)
            .map(|message| message.message_id);
        let cancel = outbound_message(
            attempt_id,
            AdmissionOutboxPurposeV1::CancelRequested,
            recipient,
            predecessor,
            payload,
        );
        attempt.cancel_request = Some(payload.to_vec());
        attempt.outboxes.push(cancel.clone());
        self.persist_advance(attempt).await?;
        Ok(cancel)
    }

    pub(crate) async fn sponsor_remove_pending_member(
        &self,
        attempt_id: AdmissionAttemptId,
        removal_event: &MembershipEventV2,
        recipient: &[u8],
        payload: &[u8],
    ) -> Result<PendingMemberRemovalOutcomeV1, WorkspaceConvergenceError> {
        let mut attempt = self.required_attempt(attempt_id).await?;
        let stage = match attempt.role_state {
            AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 { stage }) => stage,
            _ => return Err(inconsistent("only a sponsor can remove a pending member")),
        };
        if matches!(
            stage,
            SponsorAdmissionStageV1::Applied | SponsorAdmissionStageV1::Completed
        ) {
            return Ok(PendingMemberRemovalOutcomeV1::OrdinaryMemberRemovalRequired);
        }
        if stage == SponsorAdmissionStageV1::Rejected
            && attempt.rejection_reason == Some(AdmissionRejectionReasonV1::RemovedBeforeActivation)
        {
            return active_outbox(&attempt, AdmissionOutboxPurposeV1::Rejected)
                .map(PendingMemberRemovalOutcomeV1::AdmissionRejected);
        }

        let candidate_event: MembershipEventV2 = postcard::from_bytes(
            attempt
                .candidate_event
                .as_deref()
                .ok_or_else(|| inconsistent("pending candidate event is missing"))?,
        )
        .map_err(admission_storage)?;
        let MembershipOperationV2::AddDevice { admission } = &candidate_event.operation else {
            return Err(inconsistent("pending candidate is not an AddDevice"));
        };
        if removal_event.parent_event_id != Some(candidate_event.event_id())
            || !matches!(
                removal_event.operation,
                MembershipOperationV2::RemoveDevice { member }
                    if member == admission.facts.member_instance
            )
        {
            return Err(inconsistent(
                "pending-member removal does not match the admission candidate",
            ));
        }
        let encoded_candidate_history = attempt
            .verified_membership_history
            .clone()
            .ok_or_else(|| inconsistent("verified candidate history is missing"))?;
        let mut removed_history = VersionedMembershipHistory::decode_persisted_v2(
            &encoded_candidate_history,
            self.history_verifier.as_ref(),
        )
        .map_err(|error| inconsistent(error.to_string()))?;
        match removed_history
            .verify_and_receive_event(removal_event.clone(), self.history_verifier.as_ref())
            .map_err(|error| inconsistent(error.to_string()))?
        {
            MembershipHistoryV2ReceiveOutcome::Applied => {}
            MembershipHistoryV2ReceiveOutcome::AlreadyKnown
            | MembershipHistoryV2ReceiveOutcome::Diverged => {
                return Err(inconsistent(
                    "pending-member removal does not extend the candidate history",
                ));
            }
        }
        let encoded_removed_history = removed_history
            .encode_persisted_v2()
            .map_err(|error| inconsistent(error.to_string()))?;
        let predecessor = attempt
            .outboxes
            .iter()
            .rev()
            .find(|message| {
                !message.superseded
                    && matches!(
                        message.purpose,
                        AdmissionOutboxPurposeV1::Candidate | AdmissionOutboxPurposeV1::Commit
                    )
            })
            .map(|message| message.message_id)
            .ok_or_else(|| inconsistent("pending-member removal predecessor is missing"))?;
        for message in &mut attempt.outboxes {
            if matches!(
                message.purpose,
                AdmissionOutboxPurposeV1::Candidate
                    | AdmissionOutboxPurposeV1::Prepared
                    | AdmissionOutboxPurposeV1::Commit
                    | AdmissionOutboxPurposeV1::Applied
            ) {
                message.superseded = true;
            }
        }
        attempt.terminal_result = Some(AdmissionTerminalResultV1::Rejected);
        attempt.rejection_reason = Some(AdmissionRejectionReasonV1::RemovedBeforeActivation);
        attempt.role_state = AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 {
            stage: SponsorAdmissionStageV1::Rejected,
        });
        let rejected = outbound_message(
            attempt_id,
            AdmissionOutboxPurposeV1::Rejected,
            recipient,
            Some(predecessor),
            &encode_rejection_payload(
                AdmissionRejectionReasonV1::RemovedBeforeActivation,
                payload,
            )?,
        );
        attempt.outboxes.push(rejected.clone());

        if stage == SponsorAdmissionStageV1::Committed {
            self.persist_advance_with_history(
                attempt,
                Some(&encoded_candidate_history),
                &encoded_removed_history,
            )
            .await?;
        } else {
            self.persist_advance(attempt).await?;
        }
        Ok(PendingMemberRemovalOutcomeV1::AdmissionRejected(rejected))
    }

    pub(crate) async fn sponsor_reject_before_commit(
        &self,
        attempt_id: AdmissionAttemptId,
        reason: AdmissionRejectionReasonV1,
        recipient: &[u8],
    ) -> Result<AdmissionOutboxMessageV1, WorkspaceConvergenceError> {
        if !matches!(
            reason,
            AdmissionRejectionReasonV1::InvitationUnavailable
                | AdmissionRejectionReasonV1::AuthenticationRejected
                | AdmissionRejectionReasonV1::IdentityConflict
                | AdmissionRejectionReasonV1::JoinerHistoryAhead
                | AdmissionRejectionReasonV1::HistoryConflict
                | AdmissionRejectionReasonV1::PeerUpgradeRequired
        ) {
            return Err(inconsistent(
                "rejection reason requires its dedicated admission transition",
            ));
        }
        let mut attempt = self.required_attempt(attempt_id).await?;
        if matches!(
            attempt.role_state,
            AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 {
                stage: SponsorAdmissionStageV1::Rejected
            })
        ) && attempt.rejection_reason == Some(reason)
        {
            return active_outbox(&attempt, AdmissionOutboxPurposeV1::Rejected);
        }
        require_sponsor_stage(&attempt, SponsorAdmissionStageV1::Candidate)?;
        let predecessor = active_outbox_id(&attempt, AdmissionOutboxPurposeV1::Candidate)?;
        let payload = encode_rejection_payload(reason, &[])?;
        let rejected = outbound_message(
            attempt_id,
            AdmissionOutboxPurposeV1::Rejected,
            recipient,
            Some(predecessor),
            &payload,
        );
        for message in &mut attempt.outboxes {
            if matches!(
                message.purpose,
                AdmissionOutboxPurposeV1::Candidate | AdmissionOutboxPurposeV1::Prepared
            ) {
                message.superseded = true;
            }
        }
        attempt.terminal_result = Some(AdmissionTerminalResultV1::Rejected);
        attempt.rejection_reason = Some(reason);
        attempt.role_state = AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 {
            stage: SponsorAdmissionStageV1::Rejected,
        });
        attempt.outboxes.push(rejected.clone());
        self.persist_advance(attempt).await?;
        Ok(rejected)
    }

    pub(crate) async fn record_admission_unavailable(
        &self,
        attempt_id: AdmissionAttemptId,
        join_request: &AdmissionOutboxMessageV1,
    ) -> Result<AdmissionOutboxMessageV1, WorkspaceConvergenceError> {
        let attempt = self.required_attempt(attempt_id).await?;
        require_joiner_stage(&attempt, JoinerAdmissionStageV1::Initiated)?;
        let request = attempt
            .outboxes
            .iter()
            .find(|message| {
                message.purpose == AdmissionOutboxPurposeV1::JoinRequest && !message.superseded
            })
            .ok_or_else(|| inconsistent("pending join request outbox is missing"))?;
        if request != join_request {
            return Err(inconsistent(
                "admission unavailable does not match the pending join request",
            ));
        }
        Ok(request.clone())
    }

    pub(crate) async fn record_invitation_consume_result(
        &self,
        attempt_id: AdmissionAttemptId,
        result: InvitationConsumeResultV1,
    ) -> Result<(), WorkspaceConvergenceError> {
        let mut attempt = self.required_attempt(attempt_id).await?;
        let Some(index) = attempt.outboxes.iter().position(|message| {
            message.purpose == AdmissionOutboxPurposeV1::InvitationConsume && !message.superseded
        }) else {
            return Ok(());
        };
        match result {
            InvitationConsumeResultV1::Retryable => Ok(()),
            InvitationConsumeResultV1::Consumed
            | InvitationConsumeResultV1::NotFound
            | InvitationConsumeResultV1::Conflict => {
                attempt.outboxes[index].superseded = true;
                self.persist_advance(attempt).await
            }
        }
    }

    pub(crate) async fn acknowledge_delivery(
        &self,
        attempt_id: AdmissionAttemptId,
        acknowledgment: &AdmissionInboxRecordV1,
    ) -> Result<(), WorkspaceConvergenceError> {
        let mut attempt = self.required_attempt(attempt_id).await?;
        if attempt.terminal_result == Some(AdmissionTerminalResultV1::SupersededByNewJoin) {
            let index = attempt
                .outboxes
                .iter()
                .position(|message| admission_acknowledgment(message) == *acknowledgment)
                .ok_or_else(|| {
                    inconsistent("superseded delivery acknowledgment does not match an outbox")
                })?;
            if attempt.outboxes[index].purpose == AdmissionOutboxPurposeV1::CancelRequested {
                attempt.outboxes[index].superseded = true;
            }
            if !attempt.inbox_dedup.contains(acknowledgment) {
                attempt.inbox_dedup.push(acknowledgment.clone());
            }
            return self.persist_advance(attempt).await;
        }
        let index = attempt
            .outboxes
            .iter()
            .position(|message| {
                !message.superseded
                    && matches!(
                        message.purpose,
                        AdmissionOutboxPurposeV1::JoinRequest
                            | AdmissionOutboxPurposeV1::Candidate
                            | AdmissionOutboxPurposeV1::Prepared
                            | AdmissionOutboxPurposeV1::Commit
                            | AdmissionOutboxPurposeV1::Applied
                    )
                    && admission_acknowledgment(message) == *acknowledgment
            })
            .ok_or_else(|| inconsistent("delivery acknowledgment does not match an outbox"))?;
        attempt.outboxes[index].superseded = true;
        if !attempt.inbox_dedup.contains(acknowledgment) {
            attempt.inbox_dedup.push(acknowledgment.clone());
        }
        self.persist_advance(attempt).await
    }

    pub(crate) async fn enqueue_post_commit_delivery(
        &self,
        attempt_id: AdmissionAttemptId,
        purpose: AdmissionOutboxPurposeV1,
        recipient: &[u8],
        payload: &[u8],
    ) -> Result<AdmissionOutboxMessageV1, WorkspaceConvergenceError> {
        if !matches!(
            purpose,
            AdmissionOutboxPurposeV1::ExistingMemberSecurityUpdate
                | AdmissionOutboxPurposeV1::HistoryOrReceiptBatch
        ) {
            return Err(inconsistent("outbox is not a post-commit delivery"));
        }
        let mut attempt = self.required_attempt(attempt_id).await?;
        require_sponsor_stage(&attempt, SponsorAdmissionStageV1::Completed)?;
        let predecessor = active_outbox_id(&attempt, AdmissionOutboxPurposeV1::Complete)?;
        let message = outbound_message(attempt_id, purpose, recipient, Some(predecessor), payload);
        if attempt.outboxes.contains(&message) {
            return Ok(message);
        }
        attempt.outboxes.push(message.clone());
        self.persist_advance(attempt).await?;
        Ok(message)
    }

    pub(crate) async fn acknowledge_persisted_delivery(
        &self,
        attempt_id: AdmissionAttemptId,
        purpose: AdmissionOutboxPurposeV1,
        acknowledgment: &AdmissionInboxRecordV1,
    ) -> Result<(), WorkspaceConvergenceError> {
        if !matches!(
            purpose,
            AdmissionOutboxPurposeV1::ExistingMemberSecurityUpdate
                | AdmissionOutboxPurposeV1::HistoryOrReceiptBatch
        ) {
            return Err(inconsistent(
                "acknowledgment is not persisted-delivery evidence",
            ));
        }
        let mut attempt = self.required_attempt(attempt_id).await?;
        let index = attempt
            .outboxes
            .iter()
            .position(|message| {
                message.purpose == purpose
                    && !message.superseded
                    && admission_acknowledgment(message) == *acknowledgment
            })
            .ok_or_else(|| inconsistent("persisted-delivery evidence does not match an outbox"))?;
        attempt.outboxes[index].superseded = true;
        if !attempt.inbox_dedup.contains(acknowledgment) {
            attempt.inbox_dedup.push(acknowledgment.clone());
        }
        self.persist_advance(attempt).await
    }

    pub(crate) async fn sponsor_decide_cancel(
        &self,
        attempt_id: AdmissionAttemptId,
        cancel: &AdmissionOutboxMessageV1,
        recipient: &[u8],
        rejected_payload: &[u8],
    ) -> Result<AdmissionOutboxMessageV1, WorkspaceConvergenceError> {
        require_message(
            attempt_id,
            cancel,
            AdmissionOutboxPurposeV1::CancelRequested,
            cancel.predecessor_message_id,
        )?;
        if let Some(terminal) = self
            .repository
            .load_terminal(attempt_id)
            .await
            .map_err(map_repository_error)?
        {
            if terminal.terminal_result != AdmissionTerminalResultV1::Rejected
                || terminal.rejection_reason != Some(AdmissionRejectionReasonV1::Cancelled)
            {
                return Err(inconsistent("admission is already terminal"));
            }
            let rejected: AdmissionOutboxMessageV1 =
                postcard::from_bytes(&terminal.replay_result).map_err(admission_storage)?;
            if rejected.purpose != AdmissionOutboxPurposeV1::Rejected
                || rejected.predecessor_message_id != Some(cancel.message_id)
                || rejected.recipient != recipient
                || decode_rejection_reason(&rejected.payload)?
                    != AdmissionRejectionReasonV1::Cancelled
            {
                return Err(inconsistent("cancel rejection replay does not match"));
            }
            return Ok(rejected);
        }
        let mut attempt = self.required_attempt(attempt_id).await?;
        let stage = match attempt.role_state {
            AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 { stage }) => stage,
            _ => return Err(inconsistent("only a sponsor can decide cancellation")),
        };
        match stage {
            SponsorAdmissionStageV1::Accepted
            | SponsorAdmissionStageV1::Candidate
            | SponsorAdmissionStageV1::Prepared => {
                accept_incoming(
                    &mut attempt,
                    cancel,
                    &[
                        AdmissionOutboxPurposeV1::Candidate,
                        AdmissionOutboxPurposeV1::Prepared,
                    ],
                );
                attempt.cancel_request = Some(cancel.payload.clone());
                attempt.cancel_outcome = Some(b"cancelled".to_vec());
                attempt.terminal_result = Some(AdmissionTerminalResultV1::Rejected);
                attempt.rejection_reason = Some(AdmissionRejectionReasonV1::Cancelled);
                attempt.role_state =
                    AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 {
                        stage: SponsorAdmissionStageV1::Rejected,
                    });
                let rejected = outbound_message(
                    attempt_id,
                    AdmissionOutboxPurposeV1::Rejected,
                    recipient,
                    Some(cancel.message_id),
                    &encode_rejection_payload(
                        AdmissionRejectionReasonV1::Cancelled,
                        rejected_payload,
                    )?,
                );
                attempt.outboxes.push(rejected.clone());
                self.persist_advance(attempt).await?;
                Ok(rejected)
            }
            SponsorAdmissionStageV1::Committed | SponsorAdmissionStageV1::Applied => {
                accept_incoming(&mut attempt, cancel, &[]);
                attempt.cancel_request = Some(cancel.payload.clone());
                attempt.cancel_outcome = Some(b"too_late_committed".to_vec());
                let committed = attempt
                    .outboxes
                    .iter()
                    .find(|message| {
                        matches!(
                            message.purpose,
                            AdmissionOutboxPurposeV1::Commit | AdmissionOutboxPurposeV1::Complete
                        ) && !message.superseded
                    })
                    .cloned()
                    .ok_or_else(|| inconsistent("committed admission outbox is missing"))?;
                self.persist_advance(attempt).await?;
                Ok(committed)
            }
            SponsorAdmissionStageV1::Rejected
                if attempt.terminal_result == Some(AdmissionTerminalResultV1::Rejected)
                    && attempt.rejection_reason == Some(AdmissionRejectionReasonV1::Cancelled)
                    && attempt.cancel_request.as_deref() == Some(cancel.payload.as_slice())
                    && attempt.inbox_dedup.contains(&inbox_record(cancel)) =>
            {
                attempt
                    .outboxes
                    .iter()
                    .find(|message| {
                        message.purpose == AdmissionOutboxPurposeV1::Rejected
                            && message.predecessor_message_id == Some(cancel.message_id)
                    })
                    .cloned()
                    .map(|mut message| {
                        message.superseded = false;
                        message
                    })
                    .ok_or_else(|| inconsistent("cancel rejection outbox is missing"))
            }
            SponsorAdmissionStageV1::Completed | SponsorAdmissionStageV1::Rejected => {
                Err(inconsistent("admission is already terminal"))
            }
        }
    }

    pub(crate) async fn joiner_record_rejected(
        &self,
        attempt_id: AdmissionAttemptId,
        rejected: &AdmissionOutboxMessageV1,
    ) -> Result<AdmissionInboxRecordV1, WorkspaceConvergenceError> {
        let mut attempt = self.required_attempt(attempt_id).await?;
        let acknowledgment = admission_acknowledgment(rejected);
        if attempt.terminal_result == Some(AdmissionTerminalResultV1::SupersededByNewJoin) {
            let predecessor = rejected
                .predecessor_message_id
                .ok_or_else(|| inconsistent("cleanup confirmation predecessor is missing"))?;
            if rejected.purpose != AdmissionOutboxPurposeV1::Rejected {
                return Err(inconsistent(
                    "superseded join cleanup confirmation is invalid",
                ));
            }
            require_message(
                attempt_id,
                rejected,
                AdmissionOutboxPurposeV1::Rejected,
                Some(predecessor),
            )?;
            let cleanup = attempt
                .outboxes
                .iter_mut()
                .find(|message| {
                    message.purpose == AdmissionOutboxPurposeV1::CancelRequested
                        && message.message_id == predecessor
                        && !message.superseded
                })
                .ok_or_else(|| inconsistent("superseded join cleanup does not match"))?;
            if rejected.recipient != cleanup.recipient
                || decode_rejection_reason(&rejected.payload)?
                    != AdmissionRejectionReasonV1::Cancelled
            {
                return Err(inconsistent(
                    "superseded join cleanup confirmation is invalid",
                ));
            }
            cleanup.superseded = true;
            if !attempt.inbox_dedup.contains(&acknowledgment) {
                attempt.inbox_dedup.push(acknowledgment.clone());
            }
            self.persist_advance(attempt).await?;
            return Ok(acknowledgment);
        }
        if attempt.terminal_result == Some(AdmissionTerminalResultV1::Rejected)
            && attempt.inbox_dedup.contains(&acknowledgment)
        {
            return Ok(acknowledgment);
        }
        if !attempt.is_joiner() || attempt.is_terminal() {
            return Err(inconsistent("only a pending join can receive rejection"));
        }
        let predecessor = rejected
            .predecessor_message_id
            .ok_or_else(|| inconsistent("rejection predecessor is missing"))?;
        if !attempt
            .outboxes
            .iter()
            .any(|message| message.message_id == predecessor && !message.superseded)
        {
            return Err(inconsistent("rejection does not match pending join work"));
        }
        require_message(
            attempt_id,
            rejected,
            AdmissionOutboxPurposeV1::Rejected,
            Some(predecessor),
        )?;
        let rejection_reason = decode_rejection_reason(&rejected.payload)?;
        if let Some(encoded_transition) = attempt.space_transition.as_deref() {
            let transition = AdmissionSpaceTransitionV2::decode(encoded_transition)
                .ok_or_else(|| inconsistent("saved space transition is invalid"))?;
            if transition.phase_rank() >= transition.activation_started_rank() {
                return Err(inconsistent(
                    "committed space transition cannot be rejected or rolled back",
                ));
            }
            self.space_transition
                .discard_pre_activation(&transition)
                .await
                .map_err(map_space_transition_error)?;
            attempt.space_transition = None;
            attempt.target_access_state = None;
        }
        if let Some(staged) = attempt.staged_security_state.take() {
            self.security_transition.discard(staged);
        }
        if let Some(pending) = attempt.joiner_pending_security_state.take() {
            self.security_transition.discard(pending);
        }
        attempt.terminal_result = Some(AdmissionTerminalResultV1::Rejected);
        attempt.rejection_reason = Some(rejection_reason);
        attempt.role_state = AdmissionAttemptRoleStateV1::Joiner(JoinerAdmissionStateV1 {
            stage: JoinerAdmissionStageV1::Rejected,
        });
        for message in &mut attempt.outboxes {
            message.superseded = true;
        }
        if !attempt.inbox_dedup.contains(&acknowledgment) {
            attempt.inbox_dedup.push(acknowledgment.clone());
        }
        self.persist_advance(attempt).await?;
        Ok(acknowledgment)
    }

    pub(crate) async fn joiner_reject_before_candidate(
        &self,
        attempt_id: AdmissionAttemptId,
        reason: AdmissionRejectionReasonV1,
    ) -> Result<(), WorkspaceConvergenceError> {
        let mut attempt = self.required_attempt(attempt_id).await?;
        if attempt.terminal_result == Some(AdmissionTerminalResultV1::Rejected)
            && attempt.rejection_reason == Some(reason)
        {
            return Ok(());
        }
        require_joiner_stage(&attempt, JoinerAdmissionStageV1::Initiated)?;
        if let Some(pending) = attempt.joiner_pending_security_state.take() {
            self.security_transition.discard(pending);
        }
        attempt.terminal_result = Some(AdmissionTerminalResultV1::Rejected);
        attempt.rejection_reason = Some(reason);
        attempt.role_state = AdmissionAttemptRoleStateV1::Joiner(JoinerAdmissionStateV1 {
            stage: JoinerAdmissionStageV1::Rejected,
        });
        for message in &mut attempt.outboxes {
            message.superseded = true;
        }
        self.persist_advance(attempt).await
    }

    pub(crate) async fn sponsor_confirm_rejected(
        &self,
        attempt_id: AdmissionAttemptId,
        rejected_ack: &AdmissionInboxRecordV1,
    ) -> Result<(), WorkspaceConvergenceError> {
        if let Some(terminal) = self
            .repository
            .load_terminal(attempt_id)
            .await
            .map_err(map_repository_error)?
        {
            if terminal.terminal_result == AdmissionTerminalResultV1::Rejected
                && terminal.rejection_reason == Some(AdmissionRejectionReasonV1::Cancelled)
                && terminal.acknowledgment_rebuild.contains(rejected_ack)
            {
                return Ok(());
            }
            return Err(inconsistent("rejected acknowledgment does not match"));
        }
        let mut attempt = self.required_attempt(attempt_id).await?;
        if !matches!(
            attempt.role_state,
            AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 {
                stage: SponsorAdmissionStageV1::Rejected
            })
        ) || attempt.terminal_result != Some(AdmissionTerminalResultV1::Rejected)
        {
            return Err(inconsistent("sponsor admission is not rejected"));
        }
        if attempt.inbox_dedup.contains(rejected_ack) {
            return Ok(());
        }
        let rejected_index = attempt
            .outboxes
            .iter()
            .position(|message| {
                message.purpose == AdmissionOutboxPurposeV1::Rejected
                    && !message.superseded
                    && admission_acknowledgment(message) == *rejected_ack
            })
            .ok_or_else(|| inconsistent("rejected acknowledgment does not match"))?;
        attempt.outboxes[rejected_index].superseded = true;
        attempt.inbox_dedup.push(rejected_ack.clone());
        self.persist_advance(attempt).await
    }

    pub(crate) async fn joiner_apply(
        &self,
        attempt_id: AdmissionAttemptId,
        commit_message: &AdmissionOutboxMessageV1,
        activation_receipt: &AdmissionActivationReceipt,
        recipient: &[u8],
        payload: &[u8],
    ) -> Result<AdmissionOutboxMessageV1, WorkspaceConvergenceError> {
        if activation_receipt.attempt_id != *attempt_id.as_bytes() {
            return Err(inconsistent(
                "activation receipt is bound to another attempt",
            ));
        }
        let mut attempt = self.required_attempt(attempt_id).await?;
        if attempt.terminal_result == Some(AdmissionTerminalResultV1::SupersededByNewJoin) {
            if commit_message.purpose != AdmissionOutboxPurposeV1::Commit
                || commit_message.payload.is_empty()
            {
                return Err(inconsistent("superseded join commit evidence is invalid"));
            }
            let evidence = inbox_record(commit_message);
            if !attempt.inbox_dedup.contains(&evidence) {
                attempt.inbox_dedup.push(evidence);
                self.persist_advance(attempt).await?;
            }
            return Err(WorkspaceConvergenceError::RecoveryRequired);
        }
        let encoded_receipt = postcard::to_stdvec(activation_receipt).map_err(admission_storage)?;
        let applied_message = outbound_message(
            attempt_id,
            AdmissionOutboxPurposeV1::Applied,
            recipient,
            Some(commit_message.message_id),
            payload,
        );
        if attempt.stage_rank().is_some_and(|rank| rank >= 5)
            && attempt.activation_receipt.as_deref() == Some(encoded_receipt.as_slice())
            && attempt.outboxes.contains(&applied_message)
        {
            return Ok(applied_message);
        }
        require_joiner_stage(&attempt, JoinerAdmissionStageV1::Prepared)?;
        let prepared_id = active_outbox_id(&attempt, AdmissionOutboxPurposeV1::Prepared)?;
        require_message(
            attempt_id,
            commit_message,
            AdmissionOutboxPurposeV1::Commit,
            Some(prepared_id),
        )?;
        let staged_history = attempt
            .verified_membership_history
            .as_deref()
            .ok_or_else(|| inconsistent("verified joiner history is missing"))?;
        let encoded_history = record_activation_receipt(
            staged_history,
            activation_receipt,
            self.history_verifier.as_ref(),
        )?;
        attempt.activation_receipt = Some(encoded_receipt);
        attempt.verified_membership_history = Some(encoded_history.clone());
        attempt.role_state = AdmissionAttemptRoleStateV1::Joiner(JoinerAdmissionStateV1 {
            stage: JoinerAdmissionStageV1::Applied,
        });
        accept_incoming(
            &mut attempt,
            commit_message,
            &[
                AdmissionOutboxPurposeV1::Prepared,
                AdmissionOutboxPurposeV1::CancelRequested,
            ],
        );
        if attempt.cancel_request.is_some() {
            attempt.cancel_outcome = Some(b"too_late_committed".to_vec());
        }
        attempt.outboxes.push(applied_message.clone());
        let base_history = attempt
            .base_membership_history
            .clone()
            .ok_or_else(|| inconsistent("joiner base membership history is missing"))?;
        self.persist_advance_with_history(attempt, Some(&base_history), &encoded_history)
            .await?;
        Ok(applied_message)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn sponsor_prepare_security_activation(
        &self,
        attempt_id: AdmissionAttemptId,
        activation_receipt: &AdmissionActivationReceipt,
    ) -> Result<(), WorkspaceConvergenceError> {
        let mut attempt = self.required_attempt(attempt_id).await?;
        require_sponsor_stage(&attempt, SponsorAdmissionStageV1::Committed)?;
        let marker = postcard::to_stdvec(activation_receipt).map_err(admission_storage)?;
        if attempt.write_ahead_recovery.as_deref() == Some(marker.as_slice()) {
            return Ok(());
        }
        if attempt.write_ahead_recovery.is_some() {
            return Err(inconsistent("sponsor activation recovery marker conflicts"));
        }
        attempt.write_ahead_recovery = Some(marker);
        self.persist_advance(attempt).await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn sponsor_complete(
        &self,
        attempt_id: AdmissionAttemptId,
        applied_message: &AdmissionOutboxMessageV1,
        activation_receipt: &AdmissionActivationReceipt,
        completion: &[u8],
        recipient: &[u8],
        payload: &[u8],
    ) -> Result<AdmissionOutboxMessageV1, WorkspaceConvergenceError> {
        if activation_receipt.attempt_id != *attempt_id.as_bytes() {
            return Err(inconsistent(
                "activation receipt is bound to another attempt",
            ));
        }
        let mut attempt = self.required_attempt(attempt_id).await?;
        let encoded_receipt = postcard::to_stdvec(activation_receipt).map_err(admission_storage)?;
        let complete_message = outbound_message(
            attempt_id,
            AdmissionOutboxPurposeV1::Complete,
            recipient,
            Some(applied_message.message_id),
            payload,
        );
        if attempt.stage_rank().is_some_and(|rank| rank >= 5)
            && attempt.activation_receipt.as_deref() == Some(encoded_receipt.as_slice())
            && attempt.completion.as_deref() == Some(completion)
            && attempt.outboxes.contains(&complete_message)
        {
            return Ok(complete_message);
        }
        require_sponsor_stage(&attempt, SponsorAdmissionStageV1::Committed)?;
        let commit_id = active_outbox_id(&attempt, AdmissionOutboxPurposeV1::Commit)?;
        require_message(
            attempt_id,
            applied_message,
            AdmissionOutboxPurposeV1::Applied,
            Some(commit_id),
        )?;
        let committed_history = self
            .repository
            .load_membership_history_v2()
            .await
            .map_err(map_repository_error)?
            .ok_or_else(|| inconsistent("committed sponsor history is missing"))?;
        let encoded_history = record_activation_receipt(
            &committed_history,
            activation_receipt,
            self.history_verifier.as_ref(),
        )?;
        let expected_history = committed_history;
        attempt.activation_receipt = Some(encoded_receipt);
        attempt.verified_membership_history = Some(encoded_history.clone());
        attempt.completion = Some(completion.to_vec());
        attempt.write_ahead_recovery = None;
        attempt.terminal_result = Some(AdmissionTerminalResultV1::Completed);
        attempt.role_state = AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 {
            stage: SponsorAdmissionStageV1::Completed,
        });
        accept_incoming(
            &mut attempt,
            applied_message,
            &[AdmissionOutboxPurposeV1::Commit],
        );
        attempt.outboxes.push(complete_message.clone());
        self.persist_advance_with_history(attempt, Some(&expected_history), &encoded_history)
            .await?;
        Ok(complete_message)
    }

    pub(crate) async fn joiner_activate(
        &self,
        attempt_id: AdmissionAttemptId,
        complete_message: &AdmissionOutboxMessageV1,
        completion: &[u8],
    ) -> Result<JoinerActivationOutcomeV1, WorkspaceConvergenceError> {
        let acknowledgment = inbox_record(complete_message);
        if let Some(terminal) = self
            .repository
            .load_terminal(attempt_id)
            .await
            .map_err(map_repository_error)?
        {
            if terminal.terminal_result == AdmissionTerminalResultV1::SupersededByNewJoin {
                return Err(WorkspaceConvergenceError::RecoveryRequired);
            }
            if terminal.terminal_result == AdmissionTerminalResultV1::Active
                && terminal.replay_result == completion
                && terminal.acknowledgment_rebuild.contains(&acknowledgment)
            {
                return Ok(JoinerActivationOutcomeV1::Active(acknowledgment));
            }
            return Err(inconsistent(
                "complete replay does not match compacted admission result",
            ));
        }
        let mut attempt = self.required_attempt(attempt_id).await?;
        if attempt.terminal_result == Some(AdmissionTerminalResultV1::SupersededByNewJoin) {
            if complete_message.purpose != AdmissionOutboxPurposeV1::Complete
                || complete_message.payload.is_empty()
            {
                return Err(inconsistent(
                    "superseded join completion evidence is invalid",
                ));
            }
            if !attempt.inbox_dedup.contains(&acknowledgment) {
                attempt.inbox_dedup.push(acknowledgment);
                self.persist_advance(attempt).await?;
            }
            return Err(WorkspaceConvergenceError::RecoveryRequired);
        }
        if attempt.terminal_result == Some(AdmissionTerminalResultV1::Active)
            && attempt.completion.as_deref() == Some(completion)
            && attempt.inbox_dedup.contains(&acknowledgment)
        {
            return Ok(JoinerActivationOutcomeV1::Active(acknowledgment));
        }
        require_joiner_stage(&attempt, JoinerAdmissionStageV1::Applied)?;
        if attempt.completion.is_none() {
            let applied_id = active_outbox_id(&attempt, AdmissionOutboxPurposeV1::Applied)?;
            require_message(
                attempt_id,
                complete_message,
                AdmissionOutboxPurposeV1::Complete,
                Some(applied_id),
            )?;
            attempt.completion = Some(completion.to_vec());
            accept_incoming(
                &mut attempt,
                complete_message,
                &[AdmissionOutboxPurposeV1::Applied],
            );
            self.persist_advance(attempt).await?;
            attempt = self.required_attempt(attempt_id).await?;
        } else if attempt.completion.as_deref() != Some(completion)
            || !attempt.inbox_dedup.contains(&acknowledgment)
        {
            return Err(inconsistent("complete replay does not match saved state"));
        }
        if let Some(encoded_transition) = attempt.space_transition.as_deref() {
            AdmissionSpaceTransitionV2::decode(encoded_transition)
                .ok_or_else(|| inconsistent("saved space transition is invalid"))?;
            return Ok(JoinerActivationOutcomeV1::SpaceTransitionRequired);
        } else {
            attempt.terminal_result = Some(AdmissionTerminalResultV1::Active);
            attempt.role_state = AdmissionAttemptRoleStateV1::Joiner(JoinerAdmissionStateV1 {
                stage: JoinerAdmissionStageV1::Completed,
            });
            self.persist_advance(attempt).await?;
        }
        Ok(JoinerActivationOutcomeV1::Active(acknowledgment))
    }

    async fn resume_space_transition(
        &self,
        mut attempt: AdmissionAttemptV1,
    ) -> Result<AdmissionAttemptV1, WorkspaceConvergenceError> {
        if !attempt.is_joiner() || attempt.completion.is_none() {
            return Err(inconsistent(
                "space transition cannot run before joiner Complete is saved",
            ));
        }
        loop {
            let transition = AdmissionSpaceTransitionV2::decode(
                attempt
                    .space_transition
                    .as_deref()
                    .ok_or_else(|| inconsistent("space transition disappeared"))?,
            )
            .ok_or_else(|| inconsistent("saved space transition is invalid"))?;
            match self
                .space_transition
                .advance(&transition)
                .await
                .map_err(map_space_transition_error)?
            {
                AdmissionSpaceTransitionStepV2::Advanced(next) => {
                    if !transition.can_advance_to(&next) {
                        return Err(inconsistent(
                            "space transition adapter skipped or replaced a phase",
                        ));
                    }
                    attempt.space_transition = Some(
                        next.encode()
                            .ok_or_else(|| inconsistent("advanced space transition is invalid"))?,
                    );
                    self.persist_advance(attempt).await?;
                    attempt = self.required_attempt(transition.attempt_id()).await?;
                }
                AdmissionSpaceTransitionStepV2::Finished(result) => {
                    if !result.matches_cleanup_pending(&transition) {
                        return Err(inconsistent(
                            "space transition result does not match cleanup state",
                        ));
                    }
                    let verified_history =
                        attempt.verified_membership_history.clone().ok_or_else(|| {
                            inconsistent("space transition verified history is missing")
                        })?;
                    attempt.space_transition_result = Some(encode_transition_result(&result)?);
                    attempt.terminal_result = Some(AdmissionTerminalResultV1::Active);
                    attempt.role_state =
                        AdmissionAttemptRoleStateV1::Joiner(JoinerAdmissionStateV1 {
                            stage: JoinerAdmissionStageV1::Completed,
                        });
                    self.persist_advance_with_history(
                        attempt,
                        Some(&verified_history),
                        &verified_history,
                    )
                    .await?;
                    return self.required_attempt(transition.attempt_id()).await;
                }
            }
        }
    }

    pub(crate) async fn sponsor_confirm_active(
        &self,
        attempt_id: AdmissionAttemptId,
        complete_ack: &AdmissionInboxRecordV1,
    ) -> Result<(), WorkspaceConvergenceError> {
        if let Some(terminal) = self
            .repository
            .load_terminal(attempt_id)
            .await
            .map_err(map_repository_error)?
        {
            if terminal.terminal_result == AdmissionTerminalResultV1::Completed
                && terminal.acknowledgment_rebuild.contains(complete_ack)
            {
                return Ok(());
            }
            return Err(inconsistent(
                "complete acknowledgment does not match compacted admission result",
            ));
        }
        let mut attempt = self.required_attempt(attempt_id).await?;
        if attempt.terminal_result == Some(AdmissionTerminalResultV1::Completed)
            && attempt.inbox_dedup.contains(complete_ack)
        {
            return Ok(());
        }
        require_sponsor_stage(&attempt, SponsorAdmissionStageV1::Completed)?;
        let complete_index = attempt
            .outboxes
            .iter()
            .position(|message| {
                message.purpose == AdmissionOutboxPurposeV1::Complete && !message.superseded
            })
            .ok_or_else(|| inconsistent("complete outbox is missing"))?;
        if inbox_record(&attempt.outboxes[complete_index]) != *complete_ack {
            return Err(inconsistent("complete acknowledgment does not match"));
        }
        attempt.outboxes[complete_index].superseded = true;
        attempt.inbox_dedup.push(complete_ack.clone());
        attempt.terminal_result = Some(AdmissionTerminalResultV1::Completed);
        attempt.role_state = AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 {
            stage: SponsorAdmissionStageV1::Completed,
        });
        self.persist_advance(attempt).await
    }

    pub(crate) async fn recoverable(
        &self,
    ) -> Result<Vec<AdmissionAttemptV1>, WorkspaceConvergenceError> {
        self.repository
            .scan_recoverable()
            .await
            .map_err(map_repository_error)
    }

    pub(crate) async fn requires_session_transition(
        &self,
    ) -> Result<bool, WorkspaceConvergenceError> {
        Ok(self.recoverable().await?.into_iter().any(|attempt| {
            attempt.is_joiner()
                && attempt.completion.is_some()
                && attempt.space_transition.is_some()
                && attempt.space_transition_result.is_none()
        }))
    }

    pub(crate) async fn recover_space_transitions_after_session_drain(
        &self,
    ) -> Result<usize, WorkspaceConvergenceError> {
        let attempts = self.recoverable().await?;
        let mut finished = 0;
        for attempt in attempts {
            if attempt.is_joiner()
                && attempt.completion.is_some()
                && attempt.space_transition.is_some()
                && attempt.space_transition_result.is_none()
            {
                let attempt_id = attempt.attempt_id;
                self.resume_space_transition(attempt).await?;
                self.compact_if_settled(attempt_id).await?;
                finished += 1;
            }
        }
        Ok(finished)
    }

    pub(crate) async fn recover_with(
        &self,
        delivery: &(impl AdmissionOutboxDeliveryPort + ?Sized),
    ) -> Result<AdmissionRecoveryReportV1, WorkspaceConvergenceError> {
        let attempts = self.recoverable().await?;
        let mut report = AdmissionRecoveryReportV1::default();
        for attempt in attempts {
            for message in attempt
                .outboxes
                .iter()
                .filter(|message| !message.superseded)
            {
                report.deliveries_attempted += 1;
                let route = admission_outbox_delivery_route(&attempt, message)?;
                let Ok(outcome) = delivery
                    .deliver(attempt.attempt_id, message, route.as_ref())
                    .await
                else {
                    continue;
                };
                let confirmed = match outcome {
                    AdmissionOutboxDeliveryResultV1::Deferred => false,
                    AdmissionOutboxDeliveryResultV1::InvitationConsume(result) => {
                        if message.purpose != AdmissionOutboxPurposeV1::InvitationConsume {
                            return Err(inconsistent(
                                "invitation result does not match admission outbox purpose",
                            ));
                        }
                        let result = match result {
                            InvitationConsumeDeliveryResultV1::Consumed => {
                                InvitationConsumeResultV1::Consumed
                            }
                            InvitationConsumeDeliveryResultV1::NotFound => {
                                InvitationConsumeResultV1::NotFound
                            }
                            InvitationConsumeDeliveryResultV1::Conflict => {
                                InvitationConsumeResultV1::Conflict
                            }
                        };
                        self.record_invitation_consume_result(attempt.attempt_id, result)
                            .await?;
                        true
                    }
                    AdmissionOutboxDeliveryResultV1::Persisted(acknowledgment) => {
                        match message.purpose {
                            AdmissionOutboxPurposeV1::JoinRequest
                            | AdmissionOutboxPurposeV1::Candidate
                            | AdmissionOutboxPurposeV1::Prepared
                            | AdmissionOutboxPurposeV1::Commit
                            | AdmissionOutboxPurposeV1::Applied => {
                                self.acknowledge_delivery(attempt.attempt_id, &acknowledgment)
                                    .await?;
                            }
                            AdmissionOutboxPurposeV1::Rejected => {
                                self.sponsor_confirm_rejected(attempt.attempt_id, &acknowledgment)
                                    .await?;
                            }
                            AdmissionOutboxPurposeV1::Complete => {
                                self.sponsor_confirm_active(attempt.attempt_id, &acknowledgment)
                                    .await?;
                            }
                            AdmissionOutboxPurposeV1::ExistingMemberSecurityUpdate
                            | AdmissionOutboxPurposeV1::HistoryOrReceiptBatch => {
                                self.acknowledge_persisted_delivery(
                                    attempt.attempt_id,
                                    message.purpose,
                                    &acknowledgment,
                                )
                                .await?;
                            }
                            AdmissionOutboxPurposeV1::CancelRequested => {
                                self.acknowledge_delivery(attempt.attempt_id, &acknowledgment)
                                    .await?;
                            }
                            AdmissionOutboxPurposeV1::InvitationConsume => {
                                return Err(inconsistent(
                                    "persisted acknowledgment cannot clear this admission outbox",
                                ));
                            }
                        }
                        true
                    }
                    AdmissionOutboxDeliveryResultV1::Rejected(rejected) => {
                        if message.purpose != AdmissionOutboxPurposeV1::CancelRequested {
                            return Err(inconsistent(
                                "rejection does not match admission outbox purpose",
                            ));
                        }
                        self.joiner_record_rejected(attempt.attempt_id, &rejected)
                            .await?;
                        true
                    }
                };
                if confirmed {
                    report.deliveries_confirmed += 1;
                }
            }
            let Some(current) = self.load(attempt.attempt_id).await? else {
                continue;
            };
            if current.is_terminal()
                && current.outboxes.iter().all(|message| message.superseded)
                && current.write_ahead_recovery.is_none()
                && (current.space_transition.is_none() || current.space_transition_result.is_some())
                && !current.cleanup_pending
            {
                self.compact_if_settled(attempt.attempt_id).await?;
                report.attempts_compacted += 1;
            }
        }
        Ok(report)
    }

    pub(crate) async fn pending_inbound_member(
        &self,
        active_lineage_id: &str,
    ) -> Result<Option<PendingInboundMember>, WorkspaceConvergenceError> {
        let mut matching = self
            .repository
            .scan_recoverable()
            .await
            .map_err(map_repository_error)?
            .into_iter()
            .filter(|attempt| {
                !attempt.is_terminal()
                    && matches!(attempt.role_state, AdmissionAttemptRoleStateV1::Sponsor(_))
                    && attempt.lineage_id.as_deref() == Some(active_lineage_id)
            });
        let Some(attempt) = matching.next() else {
            return Ok(None);
        };
        if matching.next().is_some() {
            return Err(WorkspaceConvergenceError::RecoveryRequired);
        }
        let event: MembershipEventV2 = postcard::from_bytes(
            attempt
                .candidate_event
                .as_deref()
                .ok_or_else(|| inconsistent("pending inbound candidate event is missing"))?,
        )
        .map_err(admission_storage)?;
        let MembershipOperationV2::AddDevice { admission } = event.operation else {
            return Err(inconsistent(
                "pending inbound candidate is not an AddDevice",
            ));
        };
        Ok(Some(PendingInboundMember {
            device_id: admission.facts.device_id,
            display_name: admission.facts.device_name,
        }))
    }

    pub(crate) async fn current_local_join(
        &self,
    ) -> Result<Option<CurrentJoinStatus>, WorkspaceConvergenceError> {
        DurableAdmissionProjection::new(Arc::clone(&self.repository))
            .current_local_join()
            .await
    }

    pub(crate) async fn cancel_local_join(
        &self,
        join_id: [u8; 16],
    ) -> Result<CurrentJoinStatus, WorkspaceConvergenceError> {
        DurableAdmissionProjection::new(Arc::clone(&self.repository))
            .cancel_local_join(join_id)
            .await
    }

    pub(crate) async fn reset_join_projection_if_quiet(
        &self,
    ) -> Result<uc_core::membership::AdmissionProfileMetadataV1, WorkspaceConvergenceError> {
        DurableAdmissionProjection::new(Arc::clone(&self.repository))
            .reset_join_projection_if_quiet()
            .await
    }

    pub(crate) async fn compact_if_settled(
        &self,
        attempt_id: AdmissionAttemptId,
    ) -> Result<uc_core::membership::TerminalAdmissionAttemptV1, WorkspaceConvergenceError> {
        if let Some(terminal) = self
            .repository
            .load_terminal(attempt_id)
            .await
            .map_err(map_repository_error)?
        {
            return Ok(terminal);
        }
        let attempt = self.required_attempt(attempt_id).await?;
        if !attempt.is_terminal()
            || attempt.outboxes.iter().any(|message| !message.superseded)
            || attempt.write_ahead_recovery.is_some()
            || (attempt.space_transition.is_some() && attempt.space_transition_result.is_none())
            || attempt.cleanup_pending
        {
            return Err(WorkspaceConvergenceError::AdmissionInProgress);
        }
        self.repository
            .compact_terminal(attempt_id, attempt.record_version)
            .await
            .map_err(map_repository_error)
    }

    pub(crate) async fn load(
        &self,
        attempt_id: AdmissionAttemptId,
    ) -> Result<Option<AdmissionAttemptV1>, WorkspaceConvergenceError> {
        self.repository
            .load(attempt_id)
            .await
            .map_err(map_repository_error)
    }

    pub(crate) async fn is_compacted_superseded(
        &self,
        attempt_id: AdmissionAttemptId,
    ) -> Result<bool, WorkspaceConvergenceError> {
        Ok(self
            .repository
            .load_terminal(attempt_id)
            .await
            .map_err(map_repository_error)?
            .is_some_and(|terminal| {
                terminal.terminal_result == AdmissionTerminalResultV1::SupersededByNewJoin
            }))
    }

    pub(crate) async fn save_completion_recovery_challenge(
        &self,
        attempt_id: AdmissionAttemptId,
        challenge: &AdmissionCompletionRecoveryChallengeV1,
    ) -> Result<(), WorkspaceConvergenceError> {
        let encoded = postcard::to_stdvec(challenge).map_err(admission_storage)?;
        self.repository
            .save_completion_recovery_challenge(attempt_id, &encoded)
            .await
            .map_err(map_repository_error)?;
        Ok(())
    }

    pub(crate) async fn load_completion_recovery_challenge(
        &self,
        attempt_id: AdmissionAttemptId,
    ) -> Result<Option<AdmissionCompletionRecoveryChallengeV1>, WorkspaceConvergenceError> {
        self.repository
            .load_completion_recovery_challenge(attempt_id)
            .await
            .map_err(map_repository_error)?
            .map(|encoded| postcard::from_bytes(&encoded).map_err(admission_storage))
            .transpose()
    }

    pub(crate) async fn create_completion_helper(
        &self,
        attempt_id: AdmissionAttemptId,
        challenge: &AdmissionCompletionRecoveryChallengeV1,
        response: &AdmissionCompletionRecoveryResponseV1,
        lineage_id: &str,
        event_id: [u8; 32],
        target_members_digest: [u8; 32],
    ) -> Result<AdmissionAttemptV1, WorkspaceConvergenceError> {
        let challenge_bytes = postcard::to_stdvec(challenge).map_err(admission_storage)?;
        let response_bytes = postcard::to_stdvec(response).map_err(admission_storage)?;
        let mut attempt = AdmissionAttemptV1::new_completion_helper(attempt_id);
        attempt.lineage_id = Some(lineage_id.to_owned());
        attempt.base_history_position = Some(
            postcard::to_stdvec(&challenge.helper_history_position).map_err(admission_storage)?,
        );
        attempt.candidate_event = Some(response.bundle.candidate_event.clone());
        attempt.candidate_event_id = Some(event_id);
        attempt.candidate_key_package = Some(response.bundle.candidate_key_package.clone());
        attempt.target_members_digest = Some(target_members_digest);
        attempt.security_commitment = Some(response.bundle.security_commitment.clone());
        attempt.security_commit = Some(response.bundle.security_commit.clone());
        attempt.security_welcome = Some(response.bundle.security_welcome.clone());
        attempt.target_protection_group_id =
            Some(response.bundle.target_protection_group_id.clone());
        attempt.target_key_catalog = Some(response.bundle.target_key_catalog.clone());
        attempt.existing_member_security_deliveries =
            Some(response.bundle.existing_member_deliveries.clone());
        attempt.activation_receipt = Some(response.bundle.activation_receipt.clone());
        attempt.resume_public_key = Some(response.bundle.resume_public_key.clone());
        attempt.resume_peers.push(challenge_bytes.clone());
        attempt.completion_recovery_deliveries.push(response_bytes);
        self.repository
            .create_completion_helper(&attempt, &challenge_bytes)
            .await
            .map_err(map_repository_error)?;
        Ok(attempt)
    }

    pub(crate) async fn complete_as_helper(
        &self,
        mut attempt: AdmissionAttemptV1,
        completion: &[u8],
        recipient: &[u8],
        joiner_last_message_id: [u8; 32],
    ) -> Result<AdmissionOutboxMessageV1, WorkspaceConvergenceError> {
        if !matches!(
            attempt.role_state,
            AdmissionAttemptRoleStateV1::CompletionHelper(CompletionHelperAdmissionStateV1 {
                stage: CompletionHelperAdmissionStageV1::Applied,
            })
        ) {
            return Err(inconsistent("completion helper is not awaiting completion"));
        }
        let message = outbound_message(
            attempt.attempt_id,
            AdmissionOutboxPurposeV1::Complete,
            recipient,
            Some(joiner_last_message_id),
            completion,
        );
        attempt.completion = Some(completion.to_vec());
        attempt.outboxes.push(message.clone());
        attempt.terminal_result = Some(AdmissionTerminalResultV1::Completed);
        attempt.role_state =
            AdmissionAttemptRoleStateV1::CompletionHelper(CompletionHelperAdmissionStateV1 {
                stage: CompletionHelperAdmissionStageV1::Completed,
            });
        self.persist_advance(attempt).await?;
        Ok(message)
    }

    async fn required_attempt(
        &self,
        attempt_id: AdmissionAttemptId,
    ) -> Result<AdmissionAttemptV1, WorkspaceConvergenceError> {
        self.load(attempt_id)
            .await?
            .ok_or_else(|| inconsistent("admission attempt was not found"))
    }

    async fn reject_base_history_changed(
        &self,
        mut attempt: AdmissionAttemptV1,
        prepared_message: &AdmissionOutboxMessageV1,
        prepared_proof: &[u8],
        recipient: &[u8],
    ) -> Result<AdmissionOutboxMessageV1, WorkspaceConvergenceError> {
        let candidate_id = active_outbox_id(&attempt, AdmissionOutboxPurposeV1::Candidate)?;
        require_message(
            attempt.attempt_id,
            prepared_message,
            AdmissionOutboxPurposeV1::Prepared,
            Some(candidate_id),
        )?;
        accept_incoming(
            &mut attempt,
            prepared_message,
            &[AdmissionOutboxPurposeV1::Candidate],
        );
        attempt.prepared_proof = Some(prepared_proof.to_vec());
        attempt.terminal_result = Some(AdmissionTerminalResultV1::Rejected);
        attempt.rejection_reason = Some(AdmissionRejectionReasonV1::BaseHistoryChanged);
        attempt.role_state = AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 {
            stage: SponsorAdmissionStageV1::Rejected,
        });
        let rejected = outbound_message(
            attempt.attempt_id,
            AdmissionOutboxPurposeV1::Rejected,
            recipient,
            Some(prepared_message.message_id),
            &encode_rejection_payload(
                AdmissionRejectionReasonV1::BaseHistoryChanged,
                b"base_history_changed",
            )?,
        );
        attempt.outboxes.push(rejected.clone());
        self.persist_advance(attempt).await?;
        Ok(rejected)
    }

    async fn persist_advance(
        &self,
        mut attempt: AdmissionAttemptV1,
    ) -> Result<(), WorkspaceConvergenceError> {
        let expected = attempt.record_version;
        attempt.record_version = attempt
            .record_version
            .checked_add(1)
            .ok_or_else(|| inconsistent("admission record version overflow"))?;
        self.repository
            .compare_and_advance(attempt.attempt_id, expected, &attempt)
            .await
            .map_err(map_repository_error)?;
        Ok(())
    }

    async fn persist_advance_with_history(
        &self,
        mut attempt: AdmissionAttemptV1,
        expected_membership_history_v2: Option<&[u8]>,
        membership_history_v2: &[u8],
    ) -> Result<(), WorkspaceConvergenceError> {
        let expected = attempt.record_version;
        attempt.record_version = attempt
            .record_version
            .checked_add(1)
            .ok_or_else(|| inconsistent("admission record version overflow"))?;
        self.repository
            .compare_and_advance_with_membership_history_v2(
                attempt.attempt_id,
                expected,
                &attempt,
                expected_membership_history_v2,
                membership_history_v2,
            )
            .await
            .map_err(map_repository_error)?;
        Ok(())
    }

    fn match_existing_start(
        &self,
        existing: AdmissionAttemptV1,
        join_id: [u8; 16],
        sponsor: &[u8],
        request_payload: &[u8],
        pending_security_state: &[u8],
        candidate_key_package: &[u8],
        target_access_state: Option<&[u8]>,
        recovery_material: Option<&DurableJoinRecoveryMaterialV1>,
    ) -> Result<AdmissionAttemptV1, WorkspaceConvergenceError> {
        let expected = outbound_message(
            existing.attempt_id,
            AdmissionOutboxPurposeV1::JoinRequest,
            sponsor,
            None,
            request_payload,
        );
        let is_same_start = existing.is_joiner()
            && existing.join_id == Some(join_id)
            && existing.stage_rank() == Some(0)
            && existing.joiner_pending_security_state.as_deref() == Some(pending_security_state)
            && existing.candidate_key_package.as_deref() == Some(candidate_key_package)
            && existing.target_access_state.as_deref() == target_access_state
            && recovery_material.is_none_or(|material| {
                existing.joiner_member_instance == Some(material.member_instance)
                    && existing.resume_public_key.as_deref()
                        == Some(material.resume_public_key.as_slice())
                    && existing.resume_private_key.as_deref()
                        == Some(material.resume_private_key.as_slice())
            })
            && existing.outboxes.as_slice() == [expected];
        if is_same_start {
            Ok(existing)
        } else {
            Err(admission_storage(
                "attempt identity was reused with different join input",
            ))
        }
    }
}

fn validate_join_recovery_material(
    material: &DurableJoinRecoveryMaterialV1,
) -> Result<(), WorkspaceConvergenceError> {
    if material.pending_security_state.is_empty()
        || material.candidate_key_package.is_empty()
        || material.resume_public_key.len() != 32
        || material.resume_private_key.len() != 32
    {
        return Err(inconsistent("join recovery material is incomplete"));
    }
    Ok(())
}

fn sponsor_candidate_attempt(
    attempt_id: AdmissionAttemptId,
    invitation_digest: [u8; 32],
    candidate: DurableAdmissionCandidateV1,
    base_membership_history: Vec<u8>,
    verified_membership_history: Vec<u8>,
) -> AdmissionAttemptV1 {
    let mut attempt =
        AdmissionAttemptV1::new_joiner(attempt_id, [0; 16], JoinerAdmissionStageV1::Initiated);
    attempt.join_id = None;
    attempt.role_state = AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 {
        stage: SponsorAdmissionStageV1::Candidate,
    });
    apply_candidate(&mut attempt, candidate);
    attempt.base_membership_history = Some(base_membership_history);
    attempt.verified_membership_history = Some(verified_membership_history);
    attempt.invitation_claim = Some(invitation_digest.to_vec());
    attempt
}

fn require_candidate_encoding(
    candidate: &DurableAdmissionCandidateV1,
    candidate_event: &MembershipEventV2,
    sponsor_commitment: &AdmissionSecurityCommitmentV1,
    verified_history: &VersionedMembershipHistory,
) -> Result<(), WorkspaceConvergenceError> {
    let encoded_base_position = postcard::to_stdvec(&sponsor_commitment.base_history_position)
        .map_err(admission_storage)?;
    let encoded_candidate_event =
        postcard::to_stdvec(candidate_event).map_err(admission_storage)?;
    let encoded_commitment = postcard::to_stdvec(sponsor_commitment).map_err(admission_storage)?;
    if candidate.lineage_id != candidate_event.lineage_id
        || candidate.base_history_position != encoded_base_position
        || candidate.candidate_event != encoded_candidate_event
        || candidate.candidate_event_id != *candidate_event.event_id().as_bytes()
        || candidate.target_members_digest != candidate_event.resulting_members_digest
        || candidate.security_commitment != encoded_commitment
    {
        return Err(inconsistent(
            "persisted candidate does not match the verified V2 candidate",
        ));
    }
    let catalog = AdmissionContentKeyCatalogV1::decode(&candidate.target_key_catalog)
        .map_err(|error| inconsistent(error.to_string()))?;
    if catalog.target_epoch != sponsor_commitment.target_epoch
        || catalog.digest() != sponsor_commitment.key_catalog_digest
        || candidate.target_protection_group_id.is_empty()
        || candidate.target_protection_group_id.len() > 128
        || !candidate.target_protection_group_id.is_ascii()
    {
        return Err(inconsistent(
            "candidate content-key catalog does not match the security commitment",
        ));
    }
    let mut member_instances = std::collections::BTreeSet::new();
    let mut device_ids = std::collections::BTreeSet::new();
    for facts in &candidate.target_relationships {
        let credential = verified_history
            .credential_for(facts.member_instance)
            .ok_or_else(|| inconsistent("candidate relationship has no history credential"))?;
        if credential.member_instance_id(&facts.device_id) != facts.member_instance
            || !member_instances.insert(facts.member_instance)
            || !device_ids.insert(facts.device_id.clone())
        {
            return Err(inconsistent(
                "candidate relationship projection does not match verified history",
            ));
        }
    }
    if member_instances != verified_history.effective_members()
        || !matches!(
            &candidate_event.operation,
            MembershipOperationV2::AddDevice { admission }
                if candidate.target_relationships.contains(&admission.facts)
        )
    {
        return Err(inconsistent(
            "candidate relationship projection is incomplete",
        ));
    }
    let MembershipOperationV2::AddDevice { admission } = &candidate_event.operation else {
        return Err(inconsistent("admission candidate is not AddDevice"));
    };
    let mut delivery_devices = std::collections::BTreeSet::new();
    let mut delivery_credentials = std::collections::BTreeSet::new();
    for delivery in &candidate.existing_member_deliveries {
        let matching_relationships = candidate
            .target_relationships
            .iter()
            .filter(|facts| facts.device_id == delivery.recipient)
            .collect::<Vec<_>>();
        if delivery.payload.is_empty()
            || matching_relationships.len() != 1
            || matching_relationships[0].member_instance == admission.facts.member_instance
            || verified_history
                .credential_for(matching_relationships[0].member_instance)
                .is_none_or(|credential| credential.credential_id != delivery.credential_id)
            || !delivery_devices.insert(delivery.recipient.clone())
            || !delivery_credentials.insert(delivery.credential_id)
        {
            return Err(inconsistent(
                "candidate existing-member security delivery is invalid",
            ));
        }
    }
    AdmissionIdentityBindingV1::decode_and_validate(
        &candidate.identity_binding,
        &candidate_event.lineage_id,
        candidate_event.event_id(),
        candidate_event.author_member_instance_id,
        &admission.facts,
        &candidate.target_relationships,
    )
    .map_err(|error| inconsistent(error.to_string()))?;
    Ok(())
}

fn record_activation_receipt(
    encoded_history: &[u8],
    activation_receipt: &AdmissionActivationReceipt,
    verifier: &(impl HistoricalMembershipSignatureVerifier + ?Sized),
) -> Result<Vec<u8>, WorkspaceConvergenceError> {
    let mut history = VersionedMembershipHistory::decode_persisted_v2(encoded_history, verifier)
        .map_err(|error| inconsistent(error.to_string()))?;
    history
        .verify_and_record_activation_receipt(activation_receipt.clone(), verifier)
        .map_err(|error| inconsistent(error.to_string()))?;
    history
        .encode_persisted_v2()
        .map_err(|error| inconsistent(error.to_string()))
}

fn apply_candidate(attempt: &mut AdmissionAttemptV1, candidate: DurableAdmissionCandidateV1) {
    attempt.completion_recovery_routes = candidate
        .target_relationships
        .iter()
        .map(|facts| facts.transport_address_blob.clone())
        .collect();
    attempt.lineage_id = Some(candidate.lineage_id);
    attempt.base_history_position = Some(candidate.base_history_position);
    attempt.candidate_event = Some(candidate.candidate_event);
    attempt.candidate_event_id = Some(candidate.candidate_event_id);
    attempt.candidate_key_package = Some(candidate.candidate_key_package);
    attempt.resume_public_key = Some(candidate.resume_public_key);
    attempt.target_members_digest = Some(candidate.target_members_digest);
    attempt.security_commitment = Some(candidate.security_commitment);
    attempt.security_commit = Some(candidate.security_commit);
    attempt.security_welcome = Some(candidate.security_welcome);
    attempt.target_protection_group_id = Some(candidate.target_protection_group_id);
    attempt.target_key_catalog = Some(candidate.target_key_catalog);
    attempt.target_relationships = Some(candidate.target_relationships);
    attempt.existing_member_security_deliveries = Some(candidate.existing_member_deliveries);
    attempt.staged_security_state = Some(candidate.staged_security_state);
    attempt.identity_binding = Some(candidate.identity_binding);
}

fn candidate_matches(
    attempt: &AdmissionAttemptV1,
    candidate: &DurableAdmissionCandidateV1,
    compare_staged_state: bool,
) -> bool {
    attempt.lineage_id.as_deref() == Some(candidate.lineage_id.as_str())
        && attempt.base_history_position.as_deref()
            == Some(candidate.base_history_position.as_slice())
        && attempt.candidate_event.as_deref() == Some(candidate.candidate_event.as_slice())
        && attempt.candidate_event_id == Some(candidate.candidate_event_id)
        && attempt.candidate_key_package.as_deref()
            == Some(candidate.candidate_key_package.as_slice())
        && attempt.resume_public_key.as_deref() == Some(candidate.resume_public_key.as_slice())
        && attempt.target_members_digest == Some(candidate.target_members_digest)
        && attempt.security_commitment.as_deref() == Some(candidate.security_commitment.as_slice())
        && attempt.security_commit.as_deref() == Some(candidate.security_commit.as_slice())
        && attempt.security_welcome.as_deref() == Some(candidate.security_welcome.as_slice())
        && attempt.target_protection_group_id.as_deref()
            == Some(candidate.target_protection_group_id.as_str())
        && attempt.target_key_catalog.as_deref() == Some(candidate.target_key_catalog.as_slice())
        && attempt.target_relationships.as_deref()
            == Some(candidate.target_relationships.as_slice())
        && attempt.existing_member_security_deliveries.as_deref()
            == Some(candidate.existing_member_deliveries.as_slice())
        && (!compare_staged_state
            || attempt.staged_security_state.as_deref()
                == Some(candidate.staged_security_state.as_slice()))
        && attempt.identity_binding.as_deref() == Some(candidate.identity_binding.as_slice())
}

fn require_joiner_stage(
    attempt: &AdmissionAttemptV1,
    expected: JoinerAdmissionStageV1,
) -> Result<(), WorkspaceConvergenceError> {
    if matches!(
        attempt.role_state,
        AdmissionAttemptRoleStateV1::Joiner(JoinerAdmissionStateV1 { stage }) if stage == expected
    ) {
        Ok(())
    } else {
        Err(inconsistent("joiner admission message is out of order"))
    }
}

fn require_sponsor_stage(
    attempt: &AdmissionAttemptV1,
    expected: SponsorAdmissionStageV1,
) -> Result<(), WorkspaceConvergenceError> {
    if matches!(
        attempt.role_state,
        AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 { stage }) if stage == expected
    ) {
        Ok(())
    } else {
        Err(inconsistent("sponsor admission message is out of order"))
    }
}

fn require_message(
    attempt_id: AdmissionAttemptId,
    message: &AdmissionOutboxMessageV1,
    purpose: AdmissionOutboxPurposeV1,
    predecessor: Option<[u8; 32]>,
) -> Result<(), WorkspaceConvergenceError> {
    let expected = outbound_message(
        attempt_id,
        purpose,
        &message.recipient,
        predecessor,
        &message.payload,
    );
    if *message == expected {
        Ok(())
    } else {
        Err(inconsistent("admission message is out of order"))
    }
}

fn active_outbox_id(
    attempt: &AdmissionAttemptV1,
    purpose: AdmissionOutboxPurposeV1,
) -> Result<[u8; 32], WorkspaceConvergenceError> {
    attempt
        .outboxes
        .iter()
        .find(|message| message.purpose == purpose && !message.superseded)
        .map(|message| message.message_id)
        .ok_or_else(|| inconsistent("required admission outbox is missing"))
}

fn active_outbox(
    attempt: &AdmissionAttemptV1,
    purpose: AdmissionOutboxPurposeV1,
) -> Result<AdmissionOutboxMessageV1, WorkspaceConvergenceError> {
    attempt
        .outboxes
        .iter()
        .find(|message| message.purpose == purpose && !message.superseded)
        .cloned()
        .ok_or_else(|| inconsistent("required admission outbox is missing"))
}

fn accept_incoming(
    attempt: &mut AdmissionAttemptV1,
    incoming: &AdmissionOutboxMessageV1,
    superseded: &[AdmissionOutboxPurposeV1],
) {
    let record = inbox_record(incoming);
    if !attempt
        .inbox_dedup
        .iter()
        .any(|existing| existing.message_id == record.message_id)
    {
        attempt.inbox_dedup.push(record);
    }
    for message in &mut attempt.outboxes {
        if superseded.contains(&message.purpose) {
            message.superseded = true;
        }
    }
}

fn inbox_record(message: &AdmissionOutboxMessageV1) -> AdmissionInboxRecordV1 {
    let payload_digest: [u8; 32] = Sha256::digest(&message.payload).into();
    let mut hasher = Sha256::new();
    hasher.update(b"uniclipboard/admission-message-ack/v1\0");
    hasher.update(message.message_id);
    hasher.update(payload_digest);
    AdmissionInboxRecordV1 {
        message_id: message.message_id,
        payload_digest,
        acknowledgment_payload: hasher.finalize().to_vec(),
    }
}

pub(crate) fn admission_acknowledgment(
    message: &AdmissionOutboxMessageV1,
) -> AdmissionInboxRecordV1 {
    inbox_record(message)
}

pub(crate) fn durable_admission_message(
    attempt_id: AdmissionAttemptId,
    purpose: AdmissionOutboxPurposeV1,
    recipient: &[u8],
    predecessor_message_id: Option<[u8; 32]>,
    payload: &[u8],
) -> AdmissionOutboxMessageV1 {
    outbound_message(
        attempt_id,
        purpose,
        recipient,
        predecessor_message_id,
        payload,
    )
}

fn outbound_message(
    attempt_id: AdmissionAttemptId,
    purpose: AdmissionOutboxPurposeV1,
    recipient: &[u8],
    predecessor_message_id: Option<[u8; 32]>,
    payload: &[u8],
) -> AdmissionOutboxMessageV1 {
    let mut hasher = Sha256::new();
    hasher.update(b"uniclipboard/admission-message/v1\0");
    hasher.update(attempt_id.as_bytes());
    hasher.update([purpose as u8]);
    hasher.update(predecessor_message_id.unwrap_or([0; 32]));
    hasher.update((recipient.len() as u64).to_be_bytes());
    hasher.update(recipient);
    hasher.update((payload.len() as u64).to_be_bytes());
    hasher.update(payload);
    AdmissionOutboxMessageV1 {
        purpose,
        recipient: recipient.to_vec(),
        message_id: hasher.finalize().into(),
        predecessor_message_id,
        payload: payload.to_vec(),
        superseded: false,
    }
}

fn encode_rejection_payload(
    reason: AdmissionRejectionReasonV1,
    detail: &[u8],
) -> Result<Vec<u8>, WorkspaceConvergenceError> {
    postcard::to_stdvec(&(reason, detail.to_vec())).map_err(admission_storage)
}

fn decode_rejection_reason(
    payload: &[u8],
) -> Result<AdmissionRejectionReasonV1, WorkspaceConvergenceError> {
    postcard::from_bytes::<(AdmissionRejectionReasonV1, Vec<u8>)>(payload)
        .map(|(reason, _)| reason)
        .map_err(|_| inconsistent("rejection payload is invalid"))
}

fn encode_transition_result(
    result: &AdmissionSpaceTransitionResultV2,
) -> Result<Vec<u8>, WorkspaceConvergenceError> {
    result
        .encode()
        .ok_or_else(|| inconsistent("space transition result cannot be encoded"))
}

fn map_space_transition_error(error: AdmissionSpaceTransitionError) -> WorkspaceConvergenceError {
    match error {
        AdmissionSpaceTransitionError::UnreadableHistoryRequiresConfirmation => {
            WorkspaceConvergenceError::UnreadableHistoryRequiresConfirmation
        }
        AdmissionSpaceTransitionError::Locked | AdmissionSpaceTransitionError::Storage => {
            admission_storage(error)
        }
        AdmissionSpaceTransitionError::Unavailable => WorkspaceConvergenceError::Unavailable,
        AdmissionSpaceTransitionError::RecoveryRequired => {
            WorkspaceConvergenceError::RecoveryRequired
        }
        AdmissionSpaceTransitionError::Inconsistent => inconsistent(error.to_string()),
    }
}

pub(crate) fn map_repository_error(
    error: AdmissionAttemptRepositoryError,
) -> WorkspaceConvergenceError {
    match error {
        AdmissionAttemptRepositoryError::VersionConflict => {
            WorkspaceConvergenceError::AdmissionInProgress
        }
        AdmissionAttemptRepositoryError::PreviousJoinCannotBeSuperseded => {
            WorkspaceConvergenceError::PreviousJoinCannotBeSuperseded
        }
        other => admission_storage(other),
    }
}

fn admission_storage(error: impl std::fmt::Display) -> WorkspaceConvergenceError {
    WorkspaceConvergenceError::AdmissionStorage(error.to_string())
}

fn inconsistent(message: impl Into<String>) -> WorkspaceConvergenceError {
    WorkspaceConvergenceError::Inconsistent(message.into())
}

#[cfg(test)]
mod tests {
    use super::{CompletionRecoveryRouteV1, DurableAdmissionCommitPayloadV1};
    use uc_core::ids::DeviceId;
    use uc_core::membership::{AdmissionChangeFacts, MemberInstanceId};
    use uc_core::security::IdentityFingerprint;

    #[test]
    fn completion_recovery_route_excludes_unneeded_member_profile_data() {
        let facts = AdmissionChangeFacts {
            member_instance: MemberInstanceId::from_bytes([0x11; 32]),
            device_id: DeviceId::new("helper-device"),
            device_name: "private-device-name-sentinel".to_owned(),
            identity_fingerprint: IdentityFingerprint::from_display_string("ABCD-EFGH-IJKL-MNOP")
                .unwrap(),
            transport_public_key: vec![0x22; 32],
            transport_address_blob: vec![0x33; 64],
            identity_signature: b"private-signature-sentinel".to_vec(),
        };

        let route = CompletionRecoveryRouteV1::from(&facts);
        let encoded = postcard::to_stdvec(&route).unwrap();

        assert_eq!(route.member_instance, facts.member_instance);
        assert_eq!(route.device_id, facts.device_id);
        assert_eq!(route.transport_public_key, facts.transport_public_key);
        assert_eq!(route.transport_address_blob, facts.transport_address_blob);
        assert!(!encoded
            .windows(facts.device_name.len())
            .any(|window| window == facts.device_name.as_bytes()));
        assert!(!encoded
            .windows(facts.identity_signature.len())
            .any(|window| window == facts.identity_signature));
    }

    #[test]
    fn durable_commit_accepts_256_recovery_routes_and_rejects_257() {
        let route = CompletionRecoveryRouteV1 {
            member_instance: MemberInstanceId::from_bytes([0x44; 32]),
            device_id: DeviceId::new("helper-device"),
            transport_public_key: vec![0x55; 32],
            transport_address_blob: vec![0x66; 32],
        };
        let payload = |route_count| DurableAdmissionCommitPayloadV1 {
            format_version: DurableAdmissionCommitPayloadV1::FORMAT_V1,
            candidate_event_id: [0x77; 32],
            security_commitment_id: [0x88; 32],
            prepared_proof: vec![0x99],
            resume_public_key: vec![0xaa; 32],
            existing_member_deliveries: Vec::new(),
            completion_recovery_routes: vec![route.clone(); route_count],
        };

        let boundary = payload(256).encode().unwrap();
        assert!(DurableAdmissionCommitPayloadV1::decode(&boundary).is_ok());

        let over_limit = payload(257);
        assert!(over_limit.encode().is_err());
        let untrusted_over_limit = postcard::to_stdvec(&over_limit).unwrap();
        assert!(DurableAdmissionCommitPayloadV1::decode(&untrusted_over_limit).is_err());
    }
}
