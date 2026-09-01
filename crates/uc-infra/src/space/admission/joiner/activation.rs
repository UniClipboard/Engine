use std::sync::Arc;

use async_trait::async_trait;
use rand::RngCore;
use serde::Deserialize;
use uc_application::deps::{
    AdmissionSpaceTransitionPort, AdmissionSpaceTransitionPreparationV2,
    AdmissionSpaceTransitionStepV2, CompletedJoinerActivation, ExecuteJoinerActivationError,
    ExecuteJoinerActivationPort, JoinerActivationOutcome, PrepareJoinerActivationError,
    PrepareJoinerActivationPort, PreparedJoinerActivation,
};
use uc_core::membership::{
    AdmissionCompleteAckV1, AdmissionCompletionV1, AdmissionRetryState, AdmissionSpaceTransition,
    AdmissionSpaceTransitionResult, AdmissionSpaceTransitionV2,
    HistoricalMembershipSignatureVerifier, MembershipOperationV2, PendingAdmissionExchange,
    PendingGroupUpdate, SpaceAdmissionBodyV1, SpaceAdmissionEnvelopeV1, SpaceAdmissionId,
    SpaceAdmissionMessageKind, SpaceAdmissionRoute, VersionedMembershipHistory,
};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::space::admission::digest::completion_digest;
use crate::space::admission::recovery_material::open_recovery_material;

use super::super::sponsor::{activation_receipt_digest, SponsorCandidateStagedV1};

const JOINER_STAGED_TARGET_FORMAT_V2: u16 = 2;
const MAX_TRANSITION_ADVANCES: usize = 16;

pub struct DefaultJoinerActivationPreparation {
    history_verifier: Arc<dyn HistoricalMembershipSignatureVerifier>,
    transition: Arc<dyn AdmissionSpaceTransitionPort>,
}

impl DefaultJoinerActivationPreparation {
    pub fn new(
        history_verifier: Arc<dyn HistoricalMembershipSignatureVerifier>,
        transition: Arc<dyn AdmissionSpaceTransitionPort>,
    ) -> Self {
        Self {
            history_verifier,
            transition,
        }
    }
}

pub struct DefaultJoinerActivationExecutor {
    transition: Arc<dyn AdmissionSpaceTransitionPort>,
    history_verifier: Arc<dyn HistoricalMembershipSignatureVerifier>,
}

impl DefaultJoinerActivationExecutor {
    pub fn new(
        transition: Arc<dyn AdmissionSpaceTransitionPort>,
        history_verifier: Arc<dyn HistoricalMembershipSignatureVerifier>,
    ) -> Self {
        Self {
            transition,
            history_verifier,
        }
    }
}

#[derive(Deserialize, Zeroize, ZeroizeOnDrop)]
struct OwnedJoinerStagedTargetV2 {
    format_version: u16,
    mls_state: Vec<u8>,
    recovery_secret: [u8; 32],
    target_access: Vec<u8>,
    target_admission_credentials: Vec<u8>,
    preserve_unreadable_history: bool,
}

#[async_trait]
impl PrepareJoinerActivationPort for DefaultJoinerActivationPreparation {
    async fn prepare(
        &self,
        admission_id: SpaceAdmissionId,
        preparation: uc_core::membership::JoinerCompletePreparation<'_>,
        complete: &SpaceAdmissionEnvelopeV1,
    ) -> Result<PreparedJoinerActivation, PrepareJoinerActivationError> {
        let commit = match preparation.exact_commit().body() {
            SpaceAdmissionBodyV1::Commit(commit) => commit,
            _ => return Err(invalid_plan("the saved Joiner message is not Commit")),
        };
        let applied = match preparation.applied_request().body() {
            SpaceAdmissionBodyV1::Applied(applied) => applied,
            _ => return Err(invalid_plan("the saved Joiner request is not Applied")),
        };
        let completion = match complete.body() {
            SpaceAdmissionBodyV1::Complete(complete) => complete.completion(),
            _ => return Err(invalid_plan("the Joiner activation input is not Complete")),
        };
        if complete.header().admission_id() != admission_id
            || complete.header().predecessor_message_id()
                != Some(preparation.applied_request().header().message_id())
        {
            return Err(invalid_plan(
                "the Complete envelope is not bound to Applied",
            ));
        }
        let receipt = applied.activation_receipt();
        validate_completion(admission_id, completion, receipt, commit.exact_candidate())?;
        let mut history = VersionedMembershipHistory::decode_persisted_v2(
            commit.target_membership_history().as_bytes(),
            self.history_verifier.as_ref(),
        )
        .map_err(|error| PrepareJoinerActivationError::invalid(anyhow::Error::new(error)))?;
        history
            .verify_and_record_activation_receipt(receipt.clone(), self.history_verifier.as_ref())
            .map_err(|error| PrepareJoinerActivationError::invalid(anyhow::Error::new(error)))?;
        if history
            .current_position()
            .map_err(|error| PrepareJoinerActivationError::invalid(anyhow::Error::new(error)))?
            != completion.completed_history_position
        {
            return Err(invalid_plan("the Complete history position is invalid"));
        }
        let sponsor_credential = history
            .credential_for(completion.completed_by_member_instance_id)
            .ok_or_else(|| invalid_plan("the Complete signer is not in membership history"))?;
        if sponsor_credential.credential_id != completion.completed_by_credential_id
            || !self
                .history_verifier
                .verify(
                    sponsor_credential.signature_algorithm_version,
                    &sponsor_credential.public_key,
                    &completion.signing_payload(),
                    &completion.signature,
                )
                .map_err(|error| PrepareJoinerActivationError::invalid(anyhow::Error::new(error)))?
        {
            return Err(invalid_plan("the Complete signature is invalid"));
        }

        let mut staged: OwnedJoinerStagedTargetV2 =
            postcard::from_bytes(preparation.staged_target().as_bytes()).map_err(|error| {
                PrepareJoinerActivationError::invalid(anyhow::Error::new(error))
            })?;
        if staged.format_version != JOINER_STAGED_TARGET_FORMAT_V2 {
            return Err(invalid_plan(
                "the staged Joiner target format is unsupported",
            ));
        }
        let recovery = open_recovery_material(
            admission_id.as_bytes(),
            &staged.recovery_secret,
            commit.sealed_recovery_material().as_bytes(),
        )
        .map_err(|error| PrepareJoinerActivationError::invalid(anyhow::Error::new(error)))?;
        let sponsor: SponsorCandidateStagedV1 = postcard::from_bytes(&recovery)
            .map_err(|error| PrepareJoinerActivationError::invalid(anyhow::Error::new(error)))?;
        if sponsor.format_version != 1 {
            return Err(invalid_plan(
                "the recovered Sponsor security format is unsupported",
            ));
        }
        let candidate = commit.exact_candidate();
        let (local_device_id, _) = match &candidate.candidate_event().operation {
            MembershipOperationV2::AddDevice { admission } => (
                admission.facts.device_id.clone(),
                admission.facts.member_instance,
            ),
            _ => return Err(invalid_plan("the Candidate event is not AddDevice")),
        };
        let target_relationships = history
            .active_members()
            .into_iter()
            .filter_map(|member| history.admission_facts_for(member).cloned())
            .collect();
        let relayed_group_updates = sponsor
            .existing_member_deliveries
            .iter()
            .map(|delivery| {
                PendingGroupUpdate::for_admission(
                    *admission_id.as_bytes(),
                    delivery.recipient.clone(),
                    delivery.payload.clone(),
                )
            })
            .collect();
        let target_catalog = postcard::to_stdvec(&sponsor.target_key_catalog).map_err(|error| {
            PrepareJoinerActivationError::unavailable(anyhow::Error::new(error))
        })?;
        let transition = self
            .transition
            .prepare_if_needed(&AdmissionSpaceTransitionPreparationV2 {
                attempt_id: admission_id,
                target_space_id: candidate.security_commitment().lineage_id.clone(),
                target_security_commitment: candidate.security_commitment().clone(),
                target_membership_history: history.encode_persisted_v2().map_err(|error| {
                    PrepareJoinerActivationError::unavailable(anyhow::Error::new(error))
                })?,
                target_security_state: std::mem::take(&mut staged.mls_state),
                target_protection_group_id: sponsor.target_protection_group_id,
                target_key_catalog: target_catalog,
                local_device_id,
                target_relationships,
                relayed_group_updates,
                target_access_state: std::mem::take(&mut staged.target_access),
                target_admission_credentials: std::mem::take(
                    &mut staged.target_admission_credentials,
                ),
                preserve_unreadable_history: staged.preserve_unreadable_history,
            })
            .await
            .map_err(|error| {
                PrepareJoinerActivationError::unavailable(anyhow::Error::new(error))
            })?;
        if transition.attempt_id().as_bytes() != admission_id.as_bytes()
            || transition.target_space_id() != candidate.security_commitment().lineage_id
            || !transition.is_initial()
        {
            return Err(invalid_plan(
                "the prepared Space transition is inconsistent",
            ));
        }
        let encoded = transition
            .encode()
            .ok_or_else(|| invalid_plan("the prepared Space transition cannot be encoded"))?;
        let transition = AdmissionSpaceTransition::from_bytes(encoded)
            .map_err(|error| PrepareJoinerActivationError::invalid(anyhow::Error::new(error)))?;
        Ok(PreparedJoinerActivation::new(transition))
    }
}

#[async_trait]
impl ExecuteJoinerActivationPort for DefaultJoinerActivationExecutor {
    async fn execute(
        &self,
        admission_id: SpaceAdmissionId,
        preparation: uc_core::membership::JoinerActivationPreparation<'_>,
    ) -> Result<CompletedJoinerActivation, ExecuteJoinerActivationError> {
        let mut transition =
            AdmissionSpaceTransitionV2::decode(preparation.space_transition().as_bytes())
                .ok_or_else(|| invalid_execution("the saved Space transition is invalid"))?;
        if transition.attempt_id().as_bytes() != admission_id.as_bytes() {
            return Err(invalid_execution(
                "the Space transition belongs to another admission",
            ));
        }
        let result = {
            let mut completed = None;
            for _ in 0..MAX_TRANSITION_ADVANCES {
                match self
                    .transition
                    .advance(&transition)
                    .await
                    .map_err(|error| {
                        ExecuteJoinerActivationError::unavailable(anyhow::Error::new(error))
                    })? {
                    AdmissionSpaceTransitionStepV2::Advanced(next) => transition = next,
                    AdmissionSpaceTransitionStepV2::Finished(result) => {
                        completed = Some(result);
                        break;
                    }
                }
            }
            completed.ok_or_else(|| invalid_execution("the Space transition exceeded its bound"))?
        };
        let outcome = activation_outcome(
            preparation.join_id(),
            preparation.exact_commit(),
            &result,
            self.history_verifier.as_ref(),
        )?;
        let encoded = result
            .encode()
            .ok_or_else(|| invalid_execution("the Space transition result cannot be encoded"))?;
        let transition_result = AdmissionSpaceTransitionResult::from_bytes(encoded)
            .map_err(|error| ExecuteJoinerActivationError::invalid(anyhow::Error::new(error)))?;
        let completion = match preparation.completion().body() {
            SpaceAdmissionBodyV1::Complete(complete) => complete.completion(),
            _ => return Err(invalid_execution("the saved completion is invalid")),
        };
        let acknowledgment = AdmissionCompleteAckV1::new(completion_digest(completion))
            .ok_or_else(|| invalid_execution("the completion digest is invalid"))?;
        let request = SpaceAdmissionEnvelopeV1::new(
            admission_id,
            uc_core::membership::AdmissionRole::Joiner,
            3,
            mint_message_id(),
            Some(preparation.completion().header().message_id()),
            SpaceAdmissionBodyV1::CompleteAck(acknowledgment),
        )
        .map_err(|error| ExecuteJoinerActivationError::invalid(anyhow::Error::new(error)))?;
        let candidate = match preparation.exact_commit().body() {
            SpaceAdmissionBodyV1::Commit(commit) => commit.exact_candidate(),
            _ => return Err(invalid_execution("the saved Commit is invalid")),
        };
        let pending = PendingAdmissionExchange::new(
            SpaceAdmissionRoute::from_bytes(candidate.continuation_route().as_bytes().to_vec())
                .map_err(|error| {
                    ExecuteJoinerActivationError::invalid(anyhow::Error::new(error))
                })?,
            request,
            SpaceAdmissionMessageKind::Settled,
            AdmissionRetryState::new(0, 0).map_err(|error| {
                ExecuteJoinerActivationError::invalid(anyhow::Error::new(error))
            })?,
        )
        .map_err(|error| ExecuteJoinerActivationError::invalid(anyhow::Error::new(error)))?;
        Ok(CompletedJoinerActivation::new(
            transition_result,
            pending,
            outcome,
        ))
    }
}

fn activation_outcome(
    join_id: uc_core::membership::JoinId,
    exact_commit: &SpaceAdmissionEnvelopeV1,
    result: &uc_core::membership::AdmissionSpaceTransitionResultV2,
    history_verifier: &dyn HistoricalMembershipSignatureVerifier,
) -> Result<JoinerActivationOutcome, ExecuteJoinerActivationError> {
    let commit = match exact_commit.body() {
        SpaceAdmissionBodyV1::Commit(commit) => commit,
        _ => return Err(invalid_execution("the saved Commit is invalid")),
    };
    let candidate = commit.exact_candidate();
    let local_facts = match &candidate.candidate_event().operation {
        MembershipOperationV2::AddDevice { admission } => &admission.facts,
        _ => return Err(invalid_execution("the Candidate event is not AddDevice")),
    };
    let history = VersionedMembershipHistory::decode_persisted_v2(
        commit.target_membership_history().as_bytes(),
        history_verifier,
    )
    .map_err(|error| ExecuteJoinerActivationError::invalid(anyhow::Error::new(error)))?;
    let sponsor_facts = history
        .admission_facts_for(candidate.candidate_event().author_member_instance_id)
        .ok_or_else(|| invalid_execution("the Candidate author has no admission facts"))?;
    let (migrated_records, preserved_unreadable_records) = match result {
        uc_core::membership::AdmissionSpaceTransitionResultV2::Fresh { .. } => (None, None),
        uc_core::membership::AdmissionSpaceTransitionResultV2::SameSpace { .. } => {
            (Some(0), Some(0))
        }
        uc_core::membership::AdmissionSpaceTransitionResultV2::CrossSpace(result) => (
            Some(result.migrated_records),
            Some(result.preserved_unreadable_records),
        ),
        uc_core::membership::AdmissionSpaceTransitionResultV2::CrossSpaceControl(_) => {
            (Some(0), Some(0))
        }
    };
    Ok(JoinerActivationOutcome {
        join_id: *join_id.as_bytes(),
        sponsor_device_id: sponsor_facts.device_id.clone(),
        sponsor_identity_fingerprint: sponsor_facts.identity_fingerprint.clone(),
        space_id: candidate.security_commitment().lineage_id.clone(),
        self_device_id: local_facts.device_id.clone(),
        self_identity_fingerprint: local_facts.identity_fingerprint.clone(),
        migrated_records,
        preserved_unreadable_records,
    })
}

fn validate_completion(
    admission_id: SpaceAdmissionId,
    completion: &AdmissionCompletionV1,
    receipt: &uc_core::membership::AdmissionActivationReceipt,
    candidate: &uc_core::membership::AdmissionCandidateV1,
) -> Result<(), PrepareJoinerActivationError> {
    if completion.attempt_id != *admission_id.as_bytes()
        || completion.event_id != receipt.event_id
        || completion.activation_receipt_digest != activation_receipt_digest(receipt)
        || completion.security_commitment_id
            != candidate.security_commitment().security_commitment_id
    {
        return Err(invalid_plan(
            "the Complete facts differ from Applied and Commit",
        ));
    }
    Ok(())
}

fn invalid_plan(message: &'static str) -> PrepareJoinerActivationError {
    PrepareJoinerActivationError::invalid(anyhow::anyhow!(message))
}

fn invalid_execution(message: &'static str) -> ExecuteJoinerActivationError {
    ExecuteJoinerActivationError::invalid(anyhow::anyhow!(message))
}

fn mint_message_id() -> uc_core::membership::AdmissionMessageId {
    loop {
        let mut bytes = [0u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        if let Some(id) = uc_core::membership::AdmissionMessageId::from_bytes(bytes) {
            return id;
        }
    }
}
