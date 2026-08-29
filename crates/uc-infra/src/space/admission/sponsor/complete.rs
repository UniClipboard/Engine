use std::sync::Arc;

use async_trait::async_trait;
use rand::RngCore;
use serde::Serialize;
use sha2::{Digest, Sha256};
use uc_application::deps::{
    CurrentMemberSignaturePort, PrepareSponsorCompleteError, PrepareSponsorCompletePort,
    PreparedSponsorComplete,
};
use uc_core::ids::DeviceId;
use uc_core::membership::{
    AdmissionActivatedSecurityState, AdmissionActivationReceipt, AdmissionCompleteV1,
    AdmissionCompletionV1, HistoricalMembershipSignatureVerifier, SpaceAdmissionBodyV1,
    SpaceAdmissionEnvelopeV1, SpaceAdmissionId, SponsorCompletePreparation,
    VersionedMembershipHistory,
};

use super::candidate::SponsorCandidateStagedV1;

const SPONSOR_ACTIVATED_SECURITY_FORMAT_V1: u16 = 1;

pub struct DefaultSponsorCompletePreparation {
    local_device_id: DeviceId,
    signatures: Arc<dyn CurrentMemberSignaturePort>,
    history_verifier: Arc<dyn HistoricalMembershipSignatureVerifier>,
}

impl DefaultSponsorCompletePreparation {
    pub fn new(
        local_device_id: DeviceId,
        signatures: Arc<dyn CurrentMemberSignaturePort>,
        history_verifier: Arc<dyn HistoricalMembershipSignatureVerifier>,
    ) -> Self {
        Self {
            local_device_id,
            signatures,
            history_verifier,
        }
    }
}

#[derive(Serialize)]
struct SponsorActivatedSecurityV1<'a> {
    format_version: u16,
    space_id: &'a str,
    staged_state: &'a [u8],
    commit: &'a [u8],
    expected_commitment: &'a uc_core::membership::AdmissionSecurityCommitmentV1,
    committed_history: &'a [u8],
    security_commitment_id: [u8; 32],
}

#[async_trait]
impl PrepareSponsorCompletePort for DefaultSponsorCompletePreparation {
    async fn prepare(
        &self,
        admission_id: SpaceAdmissionId,
        preparation: SponsorCompletePreparation<'_>,
        applied: &SpaceAdmissionEnvelopeV1,
    ) -> Result<PreparedSponsorComplete, PrepareSponsorCompleteError> {
        let commit = match preparation.commit_reply().body() {
            SpaceAdmissionBodyV1::Commit(commit) => commit,
            _ => return Err(invalid("the saved Sponsor message is not Commit")),
        };
        let receipt = match applied.body() {
            SpaceAdmissionBodyV1::Applied(applied) => applied.activation_receipt(),
            _ => return Err(invalid("the Sponsor Complete input is not Applied")),
        };
        if applied.header().admission_id() != admission_id
            || applied.header().predecessor_message_id()
                != Some(preparation.commit_reply().header().message_id())
        {
            return Err(invalid("the Applied envelope is not bound to Commit"));
        }
        let candidate = commit.exact_candidate();
        if receipt.attempt_id != *admission_id.as_bytes()
            || receipt.event_id != candidate.candidate_event().event_id()
            || receipt.installed_security_commitment_id
                != candidate.security_commitment().security_commitment_id
        {
            return Err(invalid("the Applied receipt differs from Commit"));
        }
        let mut history = VersionedMembershipHistory::decode_persisted_v2(
            preparation.committed_history().as_bytes(),
            self.history_verifier.as_ref(),
        )
        .map_err(|error| PrepareSponsorCompleteError::invalid(anyhow::Error::new(error)))?;
        history
            .verify_and_record_activation_receipt(receipt.clone(), self.history_verifier.as_ref())
            .map_err(|error| PrepareSponsorCompleteError::invalid(anyhow::Error::new(error)))?;
        let committed_history = history
            .encode_persisted_v2()
            .map_err(|error| PrepareSponsorCompleteError::unavailable(anyhow::Error::new(error)))?;
        let completed_position = history
            .current_position()
            .map_err(|error| PrepareSponsorCompleteError::invalid(anyhow::Error::new(error)))?;

        let staged: SponsorCandidateStagedV1 =
            postcard::from_bytes(preparation.sealed_security().as_bytes())
                .map_err(|error| PrepareSponsorCompleteError::invalid(anyhow::Error::new(error)))?;
        if staged.format_version != 1 {
            return Err(invalid("the sealed Sponsor security format is unsupported"));
        }
        let member_instance = self
            .signatures
            .current_member_instance(&self.local_device_id)
            .await
            .map_err(|error| PrepareSponsorCompleteError::unavailable(anyhow::Error::new(error)))?;
        let credential = self
            .signatures
            .current_membership_credential(&self.local_device_id)
            .await
            .map_err(|error| PrepareSponsorCompleteError::unavailable(anyhow::Error::new(error)))?;
        let mut completion = AdmissionCompletionV1::new(
            *admission_id.as_bytes(),
            receipt.event_id,
            activation_receipt_digest(receipt),
            receipt.installed_security_commitment_id,
            member_instance,
            credential.credential_id,
            completed_position,
            Vec::new(),
        );
        completion.signature = self
            .signatures
            .sign_current_member_payload(&completion.signing_payload())
            .await
            .map_err(|error| PrepareSponsorCompleteError::unavailable(anyhow::Error::new(error)))?;
        let complete_reply = SpaceAdmissionEnvelopeV1::new(
            admission_id,
            uc_core::membership::AdmissionRole::Sponsor,
            2,
            mint_message_id(),
            Some(applied.header().message_id()),
            SpaceAdmissionBodyV1::Complete(AdmissionCompleteV1::new(completion)),
        )
        .map_err(|error| PrepareSponsorCompleteError::invalid(anyhow::Error::new(error)))?;
        let activated_security = AdmissionActivatedSecurityState::from_bytes(
            postcard::to_stdvec(&SponsorActivatedSecurityV1 {
                format_version: SPONSOR_ACTIVATED_SECURITY_FORMAT_V1,
                space_id: &candidate.security_commitment().lineage_id,
                staged_state: &staged.staged_state,
                commit: candidate.mls_commit().as_bytes(),
                expected_commitment: candidate.security_commitment(),
                committed_history: &committed_history,
                security_commitment_id: receipt.installed_security_commitment_id,
            })
            .map_err(|error| PrepareSponsorCompleteError::unavailable(anyhow::Error::new(error)))?,
        )
        .map_err(|error| PrepareSponsorCompleteError::invalid(anyhow::Error::new(error)))?;
        Ok(PreparedSponsorComplete::new(
            activated_security,
            complete_reply,
        ))
    }
}

pub(crate) fn activation_receipt_digest(receipt: &AdmissionActivationReceipt) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"uniclipboard/admission-activation-receipt-digest/v1\0");
    hasher.update(receipt.signing_payload());
    hasher.update((receipt.signature.len() as u64).to_be_bytes());
    hasher.update(&receipt.signature);
    hasher.finalize().into()
}

fn invalid(message: &'static str) -> PrepareSponsorCompleteError {
    PrepareSponsorCompleteError::invalid(anyhow::anyhow!(message))
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
