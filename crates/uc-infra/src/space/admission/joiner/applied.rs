use std::sync::Arc;

use async_trait::async_trait;
use rand::RngCore;
use serde::Deserialize;
use uc_application::deps::{
    AdmissionSecurityTransitionInput, PrepareJoinerAppliedError, PrepareJoinerAppliedPort,
    PreparedJoinerAppliedMaterial,
};
use uc_core::membership::{
    AdmissionActivationReceipt, AdmissionAppliedV1, AdmissionRetryState,
    HistoricalMembershipSignatureVerifier, PendingAdmissionExchange, SpaceAdmissionBodyV1,
    SpaceAdmissionEnvelopeV1, SpaceAdmissionId, SpaceAdmissionMessageKind, SpaceAdmissionRoute,
    VersionedMembershipHistory,
};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::space::admission::security::AdmissionSecurityTransitionAdapter;
use crate::space::security::mls_group::{MlsClientState, MlsGroupEngine};

const JOINER_STAGED_TARGET_FORMAT_V2: u16 = 2;
const ACTIVATION_RECEIPT_FORMAT_V1: u16 = 1;

pub struct DefaultJoinerAppliedPreparation {
    history_verifier: Arc<dyn HistoricalMembershipSignatureVerifier>,
}

impl DefaultJoinerAppliedPreparation {
    pub fn new(history_verifier: Arc<dyn HistoricalMembershipSignatureVerifier>) -> Self {
        Self { history_verifier }
    }
}

#[derive(Deserialize, Zeroize, ZeroizeOnDrop)]
struct OwnedJoinerStagedTargetV1 {
    format_version: u16,
    mls_state: Vec<u8>,
    recovery_secret: [u8; 32],
    target_access: Vec<u8>,
}

#[async_trait]
impl PrepareJoinerAppliedPort for DefaultJoinerAppliedPreparation {
    async fn prepare(
        &self,
        admission_id: SpaceAdmissionId,
        preparation: uc_core::membership::JoinerAppliedPreparation<'_>,
    ) -> Result<PreparedJoinerAppliedMaterial, PrepareJoinerAppliedError> {
        let commit = match preparation.exact_commit().body() {
            SpaceAdmissionBodyV1::Commit(commit) => commit,
            _ => return Err(invalid("the saved Joiner message is not Commit")),
        };
        if preparation.exact_commit().header().admission_id() != admission_id {
            return Err(invalid("the Commit belongs to another admission"));
        }
        let candidate = commit.exact_candidate();
        let commitment = candidate.security_commitment();
        let mut staged: OwnedJoinerStagedTargetV1 =
            postcard::from_bytes(preparation.staged_target().as_bytes())
                .map_err(|error| PrepareJoinerAppliedError::invalid(anyhow::Error::new(error)))?;
        if staged.format_version != JOINER_STAGED_TARGET_FORMAT_V2 {
            return Err(invalid("the staged Joiner target format is unsupported"));
        }
        let transition_input = AdmissionSecurityTransitionInput {
            attempt_id: commitment.attempt_id,
            base_history_position: commitment.base_history_position.clone(),
            candidate_core_digest: commitment.candidate_core_digest,
            key_catalog_digest: commitment.key_catalog_digest,
            admission_bundle_digest: commitment.admission_bundle_digest,
        };
        let derived = AdmissionSecurityTransitionAdapter::derive_public_commitment(
            &staged.mls_state,
            candidate.mls_commit().as_bytes(),
            &transition_input,
        )
        .map_err(|error| PrepareJoinerAppliedError::invalid(anyhow::Error::new(error)))?;
        if derived != *commitment {
            return Err(invalid(
                "the staged Joiner security commitment differs from Commit",
            ));
        }
        let history = VersionedMembershipHistory::decode_persisted_v2(
            commit.target_membership_history().as_bytes(),
            self.history_verifier.as_ref(),
        )
        .map_err(|error| PrepareJoinerAppliedError::invalid(anyhow::Error::new(error)))?;
        let event_id = candidate.candidate_event().event_id();
        if history.event(event_id) != Some(candidate.candidate_event()) {
            return Err(invalid(
                "the target history does not contain the exact Candidate event",
            ));
        }
        let member_instance = match &candidate.candidate_event().operation {
            uc_core::membership::MembershipOperationV2::AddDevice { admission } => {
                admission.facts.member_instance
            }
            _ => return Err(invalid("the Candidate event is not AddDevice")),
        };
        let mut receipt = AdmissionActivationReceipt::new(
            ACTIVATION_RECEIPT_FORMAT_V1,
            *admission_id.as_bytes(),
            event_id,
            candidate.candidate_event().resulting_members_digest,
            commitment.security_commitment_id,
            member_instance,
            Vec::new(),
        );
        let mls_state = std::mem::take(&mut staged.mls_state);
        receipt.signature = MlsGroupEngine::sign_member_payload(
            &MlsClientState::from_bytes(mls_state),
            &receipt.signing_payload(),
        )
        .map_err(|error| PrepareJoinerAppliedError::unavailable(anyhow::Error::new(error)))?;
        let applied = SpaceAdmissionEnvelopeV1::new(
            admission_id,
            uc_core::membership::AdmissionRole::Joiner,
            2,
            mint_message_id(),
            Some(preparation.exact_commit().header().message_id()),
            SpaceAdmissionBodyV1::Applied(AdmissionAppliedV1::new(receipt)),
        )
        .map_err(|error| PrepareJoinerAppliedError::invalid(anyhow::Error::new(error)))?;
        let route =
            SpaceAdmissionRoute::from_bytes(candidate.continuation_route().as_bytes().to_vec())
                .map_err(|error| PrepareJoinerAppliedError::invalid(anyhow::Error::new(error)))?;
        let pending = PendingAdmissionExchange::new(
            route,
            applied,
            SpaceAdmissionMessageKind::Complete,
            AdmissionRetryState::new(0, 0)
                .map_err(|error| PrepareJoinerAppliedError::invalid(anyhow::Error::new(error)))?,
        )
        .map_err(|error| PrepareJoinerAppliedError::invalid(anyhow::Error::new(error)))?;
        Ok(PreparedJoinerAppliedMaterial::new(pending))
    }
}

fn invalid(message: &'static str) -> PrepareJoinerAppliedError {
    PrepareJoinerAppliedError::invalid(anyhow::anyhow!(message))
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
