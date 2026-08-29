use std::sync::Arc;

use async_trait::async_trait;
use rand::RngCore;
use uc_application::deps::{
    PrepareSponsorCommitError, PrepareSponsorCommitPort, PreparedSponsorCommit,
};
use uc_core::membership::{
    AdmissionCandidateV1, AdmissionCommitV1, AdmissionMlsCommit, AdmissionMlsWelcome,
    AdmissionRole, AdmissionSealedRecoveryMaterial, AdmissionSealedSecurityState,
    AdmissionSignedMembershipHistory, HistoricalMembershipSignatureVerifier, MembershipOperationV2,
    SpaceAdmissionBodyV1, SpaceAdmissionEnvelopeV1, SpaceAdmissionId, SponsorCommitPreparation,
    VersionedMembershipHistory,
};

use super::candidate::SponsorCandidateStagedV1;

pub struct DefaultSponsorCommitPreparation {
    history_verifier: Arc<dyn HistoricalMembershipSignatureVerifier>,
}

impl DefaultSponsorCommitPreparation {
    pub fn new(history_verifier: Arc<dyn HistoricalMembershipSignatureVerifier>) -> Self {
        Self { history_verifier }
    }
}

#[async_trait]
impl PrepareSponsorCommitPort for DefaultSponsorCommitPreparation {
    async fn prepare(
        &self,
        admission_id: SpaceAdmissionId,
        preparation: SponsorCommitPreparation<'_>,
        prepared: &SpaceAdmissionEnvelopeV1,
    ) -> Result<PreparedSponsorCommit, PrepareSponsorCommitError> {
        let candidate = candidate_body(preparation.candidate_reply())?;
        let proof = match prepared.body() {
            SpaceAdmissionBodyV1::Prepared(prepared) => prepared.proof(),
            _ => return Err(invalid("the Sponsor Commit input is not Prepared")),
        };
        if prepared.header().admission_id() != admission_id
            || prepared.header().predecessor_message_id()
                != Some(preparation.candidate_reply().header().message_id())
        {
            return Err(invalid("the Prepared envelope is not bound to Candidate"));
        }
        let commitment = candidate.security_commitment();
        let MembershipOperationV2::AddDevice { admission } = &candidate.candidate_event().operation
        else {
            return Err(invalid("the Candidate event is not AddDevice"));
        };
        if proof.proof_format_version != 1
            || proof.attempt_id != *admission_id.as_bytes()
            || proof.lineage_id != commitment.lineage_id
            || proof.base_history_position != commitment.base_history_position
            || proof.candidate_event_id != candidate.candidate_event().event_id()
            || proof.target_members_digest != candidate.candidate_event().resulting_members_digest
            || proof.security_commitment_id != commitment.security_commitment_id
            || proof.joiner_member_instance_id != admission.facts.member_instance
            || proof.joiner_credential_id != admission.membership_credential.credential_id
        {
            return Err(invalid("the Prepared proof differs from Candidate"));
        }
        let signature_valid = self
            .history_verifier
            .verify(
                admission.membership_credential.signature_algorithm_version,
                &admission.membership_credential.public_key,
                &proof.signing_payload(),
                &proof.signature,
            )
            .map_err(|error| PrepareSponsorCommitError::invalid(anyhow::Error::new(error)))?;
        if !signature_valid {
            return Err(invalid("the Prepared proof signature is invalid"));
        }

        let mut history = VersionedMembershipHistory::decode_persisted_v2(
            candidate.base_membership_history().as_bytes(),
            self.history_verifier.as_ref(),
        )
        .map_err(|error| PrepareSponsorCommitError::invalid(anyhow::Error::new(error)))?;
        if history
            .current_position()
            .map_err(|error| PrepareSponsorCommitError::invalid(anyhow::Error::new(error)))?
            != commitment.base_history_position
        {
            return Err(invalid(
                "the Candidate base history position is inconsistent",
            ));
        }
        history
            .verify_and_receive_event(
                candidate.candidate_event().clone(),
                self.history_verifier.as_ref(),
            )
            .map_err(|error| PrepareSponsorCommitError::invalid(anyhow::Error::new(error)))?;
        let committed_history =
            AdmissionSignedMembershipHistory::from_bytes(history.encode_persisted_v2().map_err(
                |error| PrepareSponsorCommitError::unavailable(anyhow::Error::new(error)),
            )?)
            .map_err(|error| PrepareSponsorCommitError::invalid(anyhow::Error::new(error)))?;

        let staged: SponsorCandidateStagedV1 =
            postcard::from_bytes(preparation.staged_security().as_bytes())
                .map_err(|error| PrepareSponsorCommitError::invalid(anyhow::Error::new(error)))?;
        if staged.format_version != 1 || staged.sealed_recovery_material.is_empty() {
            return Err(invalid("the staged Sponsor security state is invalid"));
        }
        let sealed_recovery =
            AdmissionSealedRecoveryMaterial::from_bytes(staged.sealed_recovery_material.clone())
                .map_err(|error| PrepareSponsorCommitError::invalid(anyhow::Error::new(error)))?;
        let sealed_security = AdmissionSealedSecurityState::from_bytes(
            preparation.staged_security().as_bytes().to_vec(),
        )
        .map_err(|error| PrepareSponsorCommitError::invalid(anyhow::Error::new(error)))?;
        let commit_body = AdmissionCommitV1::new(
            copy_candidate(candidate)?,
            AdmissionSignedMembershipHistory::from_bytes(committed_history.as_bytes().to_vec())
                .map_err(|error| PrepareSponsorCommitError::invalid(anyhow::Error::new(error)))?,
            sealed_recovery,
        );
        let commit_reply = SpaceAdmissionEnvelopeV1::new(
            admission_id,
            AdmissionRole::Sponsor,
            1,
            mint_message_id(),
            Some(prepared.header().message_id()),
            SpaceAdmissionBodyV1::Commit(commit_body),
        )
        .map_err(|error| PrepareSponsorCommitError::invalid(anyhow::Error::new(error)))?;
        Ok(PreparedSponsorCommit::new(
            committed_history,
            sealed_security,
            commit_reply,
        ))
    }
}

fn candidate_body(
    envelope: &SpaceAdmissionEnvelopeV1,
) -> Result<&AdmissionCandidateV1, PrepareSponsorCommitError> {
    match envelope.body() {
        SpaceAdmissionBodyV1::Candidate(candidate) => Ok(candidate),
        _ => Err(invalid("the saved Sponsor reply is not Candidate")),
    }
}

fn copy_candidate(
    candidate: &AdmissionCandidateV1,
) -> Result<AdmissionCandidateV1, PrepareSponsorCommitError> {
    AdmissionCandidateV1::new(
        AdmissionSignedMembershipHistory::from_bytes(
            candidate.base_membership_history().as_bytes().to_vec(),
        )
        .map_err(|error| PrepareSponsorCommitError::invalid(anyhow::Error::new(error)))?,
        candidate.candidate_event().clone(),
        candidate.security_commitment().clone(),
        AdmissionMlsCommit::from_bytes(candidate.mls_commit().as_bytes().to_vec())
            .map_err(|error| PrepareSponsorCommitError::invalid(anyhow::Error::new(error)))?,
        AdmissionMlsWelcome::from_bytes(candidate.mls_welcome().as_bytes().to_vec())
            .map_err(|error| PrepareSponsorCommitError::invalid(anyhow::Error::new(error)))?,
        uc_core::membership::AdmissionContinuationRoute::from_bytes(
            candidate.continuation_route().as_bytes().to_vec(),
        )
        .map_err(|error| PrepareSponsorCommitError::invalid(anyhow::Error::new(error)))?,
    )
    .map_err(|error| PrepareSponsorCommitError::invalid(anyhow::Error::new(error)))
}

fn invalid(message: &'static str) -> PrepareSponsorCommitError {
    PrepareSponsorCommitError::invalid(anyhow::anyhow!(message))
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
