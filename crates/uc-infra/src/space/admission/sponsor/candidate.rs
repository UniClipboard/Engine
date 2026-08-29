use std::sync::Arc;

use async_trait::async_trait;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uc_application::deps::{
    CurrentMemberSignaturePort, PrepareSponsorAdmissionSecurityPort, PrepareSponsorCandidateError,
    PrepareSponsorCandidatePort, PreparedSponsorCandidate, SponsorAdmissionSecurityRecipient,
    SponsorAdmissionSecurityRequest,
};
use uc_core::ids::{DeviceId, SpaceId};
use uc_core::membership::{
    AdmissionCandidateV1, AdmissionContinuationRoute, AdmissionMlsCommit, AdmissionMlsWelcome,
    AdmissionRole, AdmissionStagedSecurityState, HistoricalMembershipSignatureVerifier,
    MembershipOperationV2, SpaceAdmissionBodyV1, SpaceAdmissionEnvelopeV1, SpaceAdmissionId,
    SponsorAdmissionSecurityDelivery, SponsorCandidatePreparation, VersionedMembershipHistory,
};

use super::base_snapshot::decode_sponsor_base_snapshot;
use crate::space::admission::recovery_material::seal_recovery_material;

const SPONSOR_CANDIDATE_STAGED_FORMAT_V1: u16 = 1;

pub struct DefaultSponsorCandidatePreparation {
    local_device_id: DeviceId,
    continuation_route: Vec<u8>,
    signatures: Arc<dyn CurrentMemberSignaturePort>,
    history_verifier: Arc<dyn HistoricalMembershipSignatureVerifier>,
    security: Arc<dyn PrepareSponsorAdmissionSecurityPort>,
}

impl DefaultSponsorCandidatePreparation {
    pub fn new(
        local_device_id: DeviceId,
        continuation_route: Vec<u8>,
        signatures: Arc<dyn CurrentMemberSignaturePort>,
        history_verifier: Arc<dyn HistoricalMembershipSignatureVerifier>,
        security: Arc<dyn PrepareSponsorAdmissionSecurityPort>,
    ) -> Self {
        Self {
            local_device_id,
            continuation_route,
            signatures,
            history_verifier,
            security,
        }
    }
}

#[derive(Serialize, Deserialize)]
pub(super) struct SponsorCandidateStagedV1 {
    pub(super) format_version: u16,
    pub(super) staged_state: Vec<u8>,
    pub(super) target_protection_group_id: String,
    pub(super) target_key_catalog: uc_core::membership::AdmissionContentKeyCatalogV1,
    pub(super) existing_member_deliveries: Vec<SponsorAdmissionSecurityDelivery>,
    pub(super) sealed_recovery_material: Vec<u8>,
}

#[async_trait]
impl PrepareSponsorCandidatePort for DefaultSponsorCandidatePreparation {
    async fn prepare(
        &self,
        admission_id: SpaceAdmissionId,
        preparation: SponsorCandidatePreparation<'_>,
    ) -> Result<PreparedSponsorCandidate, PrepareSponsorCandidateError> {
        let snapshot = decode_sponsor_base_snapshot(preparation.base_snapshot())
            .map_err(|error| PrepareSponsorCandidateError::invalid(anyhow::Error::new(error)))?;
        let history = VersionedMembershipHistory::decode_persisted_v2(
            &snapshot.membership_history,
            self.history_verifier.as_ref(),
        )
        .map_err(|error| PrepareSponsorCandidateError::invalid(anyhow::Error::new(error)))?;
        if history.lineage_id() != snapshot.lineage_id {
            return Err(PrepareSponsorCandidateError::invalid(anyhow::anyhow!(
                "the Sponsor base snapshot lineage is inconsistent"
            )));
        }
        let request = match preparation.join_request().body() {
            SpaceAdmissionBodyV1::JoinRequest(request) => request,
            _ => {
                return Err(PrepareSponsorCandidateError::invalid(anyhow::anyhow!(
                    "the Sponsor Candidate input is not a JoinRequest"
                )))
            }
        };
        let identity_is_valid = self
            .history_verifier
            .verify(
                request.membership_credential().signature_algorithm_version,
                &request.membership_credential().public_key,
                &request.identity_facts().signing_payload(),
                request.identity_signature().as_bytes(),
            )
            .map_err(|error| PrepareSponsorCandidateError::invalid(anyhow::Error::new(error)))?;
        if !identity_is_valid {
            return Err(PrepareSponsorCandidateError::invalid(anyhow::anyhow!(
                "the JoinRequest identity signature is invalid"
            )));
        }

        let author = self
            .signatures
            .current_member_instance(&self.local_device_id)
            .await
            .map_err(|error| {
                PrepareSponsorCandidateError::unavailable(anyhow::Error::new(error))
            })?;
        let author_credential = self
            .signatures
            .current_membership_credential(&self.local_device_id)
            .await
            .map_err(|error| {
                PrepareSponsorCandidateError::unavailable(anyhow::Error::new(error))
            })?;
        let mut operation_id = [0u8; 16];
        rand::rng().fill_bytes(&mut operation_id);
        let draft = history
            .create_unsigned_local_admission_event(
                author,
                &author_credential,
                request.identity_facts().clone(),
                request.membership_credential().clone(),
                Sha256::digest(request.recovery_public_key().as_bytes()).into(),
                operation_id,
            )
            .map_err(|error| PrepareSponsorCandidateError::invalid(anyhow::Error::new(error)))?;
        let candidate_core_digest = draft
            .admission_candidate_core_digest(
                *admission_id.as_bytes(),
                request.key_package().as_bytes(),
            )
            .map_err(|error| PrepareSponsorCandidateError::invalid(anyhow::Error::new(error)))?;
        let mut existing_recipients = Vec::new();
        for member in history.active_members() {
            let facts = history.admission_facts_for(member).ok_or_else(|| {
                PrepareSponsorCandidateError::invalid(anyhow::anyhow!(
                    "an active Sponsor member has no signed identity facts"
                ))
            })?;
            let credential = history.credential_for(member).ok_or_else(|| {
                PrepareSponsorCandidateError::invalid(anyhow::anyhow!(
                    "an active Sponsor member has no historical credential"
                ))
            })?;
            existing_recipients.push(SponsorAdmissionSecurityRecipient {
                device_id: facts.device_id.clone(),
                credential_id: credential.credential_id,
            });
        }
        let security = self
            .security
            .prepare_sponsor_admission_security(SponsorAdmissionSecurityRequest {
                space_id: SpaceId::from_str(&snapshot.lineage_id),
                attempt_id: *admission_id.as_bytes(),
                base_history_position: history.current_position().map_err(|error| {
                    PrepareSponsorCandidateError::invalid(anyhow::Error::new(error))
                })?,
                candidate_core_digest,
                candidate_identity: request.device_id().as_str().as_bytes().to_vec(),
                candidate_key_package: request.key_package().as_bytes().to_vec(),
                existing_recipients,
            })
            .await
            .map_err(|error| PrepareSponsorCandidateError::invalid(anyhow::Error::new(error)))?;
        let mut event = history
            .finalize_unsigned_local_admission_event(
                draft,
                request.key_package().as_bytes(),
                &security.public_commitment,
            )
            .map_err(|error| PrepareSponsorCandidateError::invalid(anyhow::Error::new(error)))?;
        event.signature = self
            .signatures
            .sign_current_member_payload(&event.signing_payload())
            .await
            .map_err(|error| {
                PrepareSponsorCandidateError::unavailable(anyhow::Error::new(error))
            })?;
        let candidate = AdmissionCandidateV1::new(
            uc_core::membership::AdmissionSignedMembershipHistory::from_bytes(
                snapshot.membership_history,
            )
            .map_err(|error| PrepareSponsorCandidateError::invalid(anyhow::Error::new(error)))?,
            event,
            security.public_commitment,
            AdmissionMlsCommit::from_bytes(security.commit).map_err(|error| {
                PrepareSponsorCandidateError::invalid(anyhow::Error::new(error))
            })?,
            AdmissionMlsWelcome::from_bytes(security.welcome).map_err(|error| {
                PrepareSponsorCandidateError::invalid(anyhow::Error::new(error))
            })?,
            AdmissionContinuationRoute::from_bytes(self.continuation_route.clone()).map_err(
                |error| PrepareSponsorCandidateError::invalid(anyhow::Error::new(error)),
            )?,
        )
        .map_err(|error| PrepareSponsorCandidateError::invalid(anyhow::Error::new(error)))?;
        let candidate_reply = SpaceAdmissionEnvelopeV1::new(
            admission_id,
            AdmissionRole::Sponsor,
            0,
            mint_message_id(),
            Some(preparation.join_request().header().message_id()),
            SpaceAdmissionBodyV1::Candidate(candidate),
        )
        .map_err(|error| PrepareSponsorCandidateError::invalid(anyhow::Error::new(error)))?;
        let recovery_plaintext = postcard::to_stdvec(&SponsorCandidateStagedV1 {
            format_version: SPONSOR_CANDIDATE_STAGED_FORMAT_V1,
            staged_state: security.staged_state.clone(),
            target_protection_group_id: security.target_protection_group_id.clone(),
            target_key_catalog: security.target_key_catalog.clone(),
            existing_member_deliveries: security.existing_member_deliveries.clone(),
            sealed_recovery_material: Vec::new(),
        })
        .map_err(|error| PrepareSponsorCandidateError::unavailable(anyhow::Error::new(error)))?;
        let sealed_recovery_material = seal_recovery_material(
            admission_id.as_bytes(),
            request.recovery_public_key().as_bytes(),
            &recovery_plaintext,
        )
        .map_err(|error| PrepareSponsorCandidateError::unavailable(anyhow::Error::new(error)))?;
        let staged = postcard::to_stdvec(&SponsorCandidateStagedV1 {
            format_version: SPONSOR_CANDIDATE_STAGED_FORMAT_V1,
            staged_state: security.staged_state,
            target_protection_group_id: security.target_protection_group_id,
            target_key_catalog: security.target_key_catalog,
            existing_member_deliveries: security.existing_member_deliveries,
            sealed_recovery_material,
        })
        .map_err(|error| PrepareSponsorCandidateError::unavailable(anyhow::Error::new(error)))?;
        let staged_security = AdmissionStagedSecurityState::from_bytes(staged)
            .map_err(|error| PrepareSponsorCandidateError::invalid(anyhow::Error::new(error)))?;
        Ok(PreparedSponsorCandidate::new(
            candidate_reply,
            staged_security,
        ))
    }
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

#[cfg(test)]
mod tests {
    use uc_application::deps::{
        AdmissionSecurityTransitionError, CurrentMemberSignatureError, PrepareSponsorCommitPort,
        PrepareSponsorCompletePort, SponsorPreparedAdmissionSecurity,
    };
    use uc_core::membership::{
        AdmissionActivationReceipt, AdmissionAppliedV1, AdmissionBaseSnapshot,
        AdmissionChangeFacts, AdmissionChannelPeerId, AdmissionContentKeyCatalogV1,
        AdmissionContentKeyEntryV1, AdmissionContinuationCredential, AdmissionIdentitySignature,
        AdmissionInvitationClaim, AdmissionJoinRequestV1, AdmissionKeyPackage, AdmissionMessageId,
        AdmissionPeerBinding, AdmissionPreparedV1, AdmissionRecoveryPublicKey,
        AdmissionSignedMembershipHistory, HistoricalMembershipSignatureError, MembershipCredential,
        PreparedAdmissionProofV1, SpaceAdmissionMessageKind, UnreadableHistoryPolicy,
        ADMISSION_SECURITY_COMMITMENT_FORMAT_V1, ED25519_SIGNATURE_ALGORITHM_V1,
    };
    use uc_core::security::IdentityFingerprint;

    use super::*;
    use crate::space::admission::sponsor::base_snapshot::PersistedSponsorBaseSnapshotV1;
    use crate::space::admission::sponsor::{
        DefaultSponsorCommitPreparation, DefaultSponsorCompletePreparation,
    };

    struct DeterministicSignatures {
        device_id: DeviceId,
        credential: MembershipCredential,
    }

    impl DeterministicSignatures {
        fn sign(credential: &MembershipCredential, payload: &[u8]) -> Vec<u8> {
            let mut hasher = Sha256::new();
            hasher.update(b"candidate-test-signature");
            hasher.update(&credential.public_key);
            hasher.update(payload);
            hasher.finalize().to_vec()
        }
    }

    #[async_trait]
    impl CurrentMemberSignaturePort for DeterministicSignatures {
        async fn current_member_epoch(&self) -> Result<u64, CurrentMemberSignatureError> {
            Ok(0)
        }

        async fn current_membership_credential(
            &self,
            device_id: &DeviceId,
        ) -> Result<MembershipCredential, CurrentMemberSignatureError> {
            (device_id == &self.device_id)
                .then(|| self.credential.clone())
                .ok_or(CurrentMemberSignatureError::InvalidState)
        }

        async fn current_member_instance(
            &self,
            device_id: &DeviceId,
        ) -> Result<uc_core::membership::MemberInstanceId, CurrentMemberSignatureError> {
            Ok(self.credential.member_instance_id(device_id))
        }

        async fn sign_current_member_payload(
            &self,
            payload: &[u8],
        ) -> Result<Vec<u8>, CurrentMemberSignatureError> {
            Ok(Self::sign(&self.credential, payload))
        }

        async fn verify_current_member_payload(
            &self,
            _member: &DeviceId,
            payload: &[u8],
            signature: &[u8],
        ) -> Result<bool, CurrentMemberSignatureError> {
            Ok(Self::sign(&self.credential, payload) == signature)
        }
    }

    impl HistoricalMembershipSignatureVerifier for DeterministicSignatures {
        fn verify(
            &self,
            signature_algorithm_version: u16,
            public_key: &[u8],
            payload: &[u8],
            signature: &[u8],
        ) -> Result<bool, HistoricalMembershipSignatureError> {
            if signature_algorithm_version != ED25519_SIGNATURE_ALGORITHM_V1 {
                return Err(HistoricalMembershipSignatureError::UnsupportedAlgorithm);
            }
            Ok(Self::sign(
                &MembershipCredential::new(signature_algorithm_version, public_key.to_vec()),
                payload,
            ) == signature)
        }
    }

    struct FixedSecurity;

    #[async_trait]
    impl PrepareSponsorAdmissionSecurityPort for FixedSecurity {
        async fn prepare_sponsor_admission_security(
            &self,
            request: SponsorAdmissionSecurityRequest,
        ) -> Result<SponsorPreparedAdmissionSecurity, AdmissionSecurityTransitionError> {
            let commitment = uc_core::membership::AdmissionSecurityCommitmentV1::new(
                ADMISSION_SECURITY_COMMITMENT_FORMAT_V1,
                request.space_id.as_ref().to_owned(),
                request.space_id.as_ref().as_bytes().to_vec(),
                request.attempt_id,
                request.base_history_position,
                request.candidate_core_digest,
                1,
                0,
                1,
                [0x81; 32],
                [0x82; 32],
                [0x83; 32],
                [0x84; 32],
                [0x85; 32],
            )
            .map_err(|_| AdmissionSecurityTransitionError::InvalidState)?;
            let catalog = AdmissionContentKeyCatalogV1::new(
                "content-key",
                1,
                vec![
                    AdmissionContentKeyEntryV1::new("legacy-v1", 0, vec![0x80; 32])
                        .map_err(|_| AdmissionSecurityTransitionError::InvalidState)?,
                    AdmissionContentKeyEntryV1::new("content-key", 1, vec![0x86; 32])
                        .map_err(|_| AdmissionSecurityTransitionError::InvalidState)?,
                ],
            )
            .map_err(|_| AdmissionSecurityTransitionError::InvalidState)?;
            Ok(SponsorPreparedAdmissionSecurity {
                staged_state: vec![0x87; 128],
                commit: vec![0x88; 64],
                welcome: vec![0x89; 64],
                public_commitment: commitment,
                target_protection_group_id: "group".to_owned(),
                target_key_catalog: catalog,
                existing_member_deliveries: Vec::new(),
            })
        }
    }

    #[tokio::test]
    async fn production_sponsor_candidate_prepares_one_complete_reply() {
        let sponsor_device = DeviceId::new("sponsor-device");
        let sponsor_credential = MembershipCredential::new(1, vec![0x61; 32]);
        let signatures = Arc::new(DeterministicSignatures {
            device_id: sponsor_device.clone(),
            credential: sponsor_credential.clone(),
        });
        let mut sponsor_facts =
            identity_facts(sponsor_device.clone(), &sponsor_credential, Vec::new());
        sponsor_facts.identity_signature =
            DeterministicSignatures::sign(&sponsor_credential, &sponsor_facts.signing_payload());
        let history = VersionedMembershipHistory::new_single_member_root(
            "space-a".to_owned(),
            sponsor_facts,
            sponsor_credential,
        )
        .expect("valid Sponsor history");
        let history_bytes = history.encode_persisted_v2().expect("history encodes");
        let snapshot = AdmissionBaseSnapshot::from_bytes(
            postcard::to_stdvec(&PersistedSponsorBaseSnapshotV1 {
                format_version: 1,
                ledger_revision: 3,
                lineage_id: "space-a".to_owned(),
                membership_history: history_bytes,
            })
            .expect("snapshot encodes"),
        )
        .expect("valid snapshot");
        let admission_id = SpaceAdmissionId::from_bytes([0x62; 32]).expect("valid admission id");
        let join_request = join_request(admission_id);
        let evidence = join_request.evidence([0x63; 32]).expect("valid evidence");
        let accepted = uc_core::membership::SponsorAdmission::accept_join_request(
            admission_id,
            AdmissionInvitationClaim::from_bytes(vec![0x64; 32]).expect("valid claim"),
            join_request,
            evidence,
            snapshot,
            AdmissionPeerBinding::new(
                AdmissionChannelPeerId::from_bytes([0x65; 32]).expect("valid local peer"),
                AdmissionChannelPeerId::from_bytes([0x66; 32]).expect("valid remote peer"),
            )
            .expect("distinct peers"),
            AdmissionContinuationCredential::from_bytes(vec![0x67; 64])
                .expect("valid continuation"),
        )
        .expect("Sponsor accepts request")
        .into_replacement();
        let adapter = DefaultSponsorCandidatePreparation::new(
            sponsor_device,
            b"continuation-route".to_vec(),
            signatures.clone(),
            signatures.clone(),
            Arc::new(FixedSecurity),
        );

        let candidate_material = adapter
            .prepare(
                admission_id,
                accepted
                    .sponsor_candidate_preparation()
                    .expect("Accepted exposes Candidate preparation"),
            )
            .await
            .unwrap_or_else(|error| panic!("Sponsor Candidate preparation failed: {error:?}"));
        let (candidate_reply, staged_security) = candidate_material.into_parts();
        let candidate_message_id = candidate_reply.header().message_id();
        let candidate = match candidate_reply.body() {
            SpaceAdmissionBodyV1::Candidate(candidate) => candidate,
            _ => panic!("Sponsor reply must be Candidate"),
        };
        let MembershipOperationV2::AddDevice { admission } = &candidate.candidate_event().operation
        else {
            panic!("Candidate must add one member");
        };
        let commitment = candidate.security_commitment();
        let mut proof = PreparedAdmissionProofV1::new(
            *admission_id.as_bytes(),
            commitment.lineage_id.clone(),
            commitment.base_history_position.clone(),
            candidate.candidate_event().event_id(),
            candidate.candidate_event().resulting_members_digest,
            commitment.security_commitment_id,
            admission.facts.member_instance,
            admission.membership_credential.credential_id,
            Vec::new(),
        );
        proof.signature = DeterministicSignatures::sign(
            &admission.membership_credential,
            &proof.signing_payload(),
        );
        let prepared_request = SpaceAdmissionEnvelopeV1::new(
            admission_id,
            AdmissionRole::Joiner,
            1,
            AdmissionMessageId::from_bytes([0x90; 32]).expect("valid Prepared id"),
            Some(candidate_message_id),
            SpaceAdmissionBodyV1::Prepared(AdmissionPreparedV1::new(proof)),
        )
        .expect("valid Prepared request");
        let candidate_state = accepted
            .fix_candidate(candidate_reply, staged_security)
            .expect("Sponsor fixes Candidate")
            .into_replacement();
        let commit = DefaultSponsorCommitPreparation::new(signatures.clone())
            .prepare(
                admission_id,
                candidate_state
                    .sponsor_commit_preparation()
                    .expect("Candidate exposes Commit preparation"),
                &prepared_request,
            )
            .await
            .unwrap_or_else(|error| panic!("Sponsor Commit preparation failed: {error:?}"));
        let (committed_history, sealed_security, commit_reply) = commit.into_parts();
        assert!(!committed_history.as_bytes().is_empty());
        assert!(!sealed_security.as_bytes().is_empty());
        assert_eq!(commit_reply.kind(), SpaceAdmissionMessageKind::Commit);
        assert_eq!(
            commit_reply.header().predecessor_message_id(),
            Some(prepared_request.header().message_id())
        );
        let commit_message_id = commit_reply.header().message_id();
        let (candidate_event, joiner_credential, security_commitment_id) = match commit_reply.body()
        {
            SpaceAdmissionBodyV1::Commit(commit) => {
                let candidate = commit.exact_candidate();
                let MembershipOperationV2::AddDevice { admission } =
                    &candidate.candidate_event().operation
                else {
                    panic!("Commit Candidate must add one member");
                };
                (
                    candidate.candidate_event().clone(),
                    admission.membership_credential.clone(),
                    candidate.security_commitment().security_commitment_id,
                )
            }
            _ => panic!("Sponsor reply must be Commit"),
        };
        let mut receipt = AdmissionActivationReceipt::new(
            1,
            *admission_id.as_bytes(),
            candidate_event.event_id(),
            candidate_event.resulting_members_digest,
            security_commitment_id,
            match &candidate_event.operation {
                MembershipOperationV2::AddDevice { admission } => admission.facts.member_instance,
                _ => panic!("Candidate must add one member"),
            },
            Vec::new(),
        );
        receipt.signature =
            DeterministicSignatures::sign(&joiner_credential, &receipt.signing_payload());
        let applied = SpaceAdmissionEnvelopeV1::new(
            admission_id,
            AdmissionRole::Joiner,
            2,
            AdmissionMessageId::from_bytes([0x91; 32]).expect("valid Applied id"),
            Some(commit_message_id),
            SpaceAdmissionBodyV1::Applied(AdmissionAppliedV1::new(receipt)),
        )
        .expect("valid Applied request");
        let committed_state = candidate_state
            .commit_prepared(
                prepared_request,
                [0x92; 32],
                AdmissionSignedMembershipHistory::from_bytes(committed_history.as_bytes().to_vec())
                    .expect("committed history copy"),
                sealed_security,
                commit_reply,
            )
            .expect("Sponsor commits Prepared")
            .into_replacement();
        DefaultSponsorCompletePreparation::new(
            DeviceId::new("sponsor-device"),
            signatures.clone(),
            signatures,
        )
        .prepare(
            admission_id,
            committed_state
                .sponsor_complete_preparation()
                .expect("Committed Sponsor exposes Complete preparation"),
            &applied,
        )
        .await
        .unwrap_or_else(|error| panic!("Sponsor Complete preparation failed: {error:?}"));
    }

    fn join_request(admission_id: SpaceAdmissionId) -> SpaceAdmissionEnvelopeV1 {
        let device_id = DeviceId::new("joiner-device");
        let credential = MembershipCredential::new(1, vec![0x71; 32]);
        let mut facts = identity_facts(device_id.clone(), &credential, Vec::new());
        facts.identity_signature =
            DeterministicSignatures::sign(&credential, &facts.signing_payload());
        let signature = facts.identity_signature.clone();
        SpaceAdmissionEnvelopeV1::new(
            admission_id,
            AdmissionRole::Joiner,
            0,
            AdmissionMessageId::from_bytes([0x72; 32]).expect("valid message id"),
            None,
            SpaceAdmissionBodyV1::JoinRequest(
                AdmissionJoinRequestV1::new(
                    uc_core::membership::InvitationId::from_bytes([0x73; 32])
                        .expect("valid invitation id"),
                    device_id,
                    facts,
                    credential,
                    AdmissionKeyPackage::from_bytes(vec![0x74; 48]).expect("valid key package"),
                    AdmissionRecoveryPublicKey::from_bytes([0x75; 32]).expect("valid recovery key"),
                    AdmissionIdentitySignature::from_bytes(signature)
                        .expect("valid identity signature"),
                    UnreadableHistoryPolicy::Discard,
                )
                .expect("valid JoinRequest"),
            ),
        )
        .expect("valid JoinRequest envelope")
    }

    fn identity_facts(
        device_id: DeviceId,
        credential: &MembershipCredential,
        identity_signature: Vec<u8>,
    ) -> AdmissionChangeFacts {
        AdmissionChangeFacts {
            member_instance: credential.member_instance_id(&device_id),
            device_id,
            device_name: "Device".to_owned(),
            identity_fingerprint: IdentityFingerprint::from_display_string("ABCD-EFGH-IJKL-MNOP")
                .expect("valid fingerprint"),
            transport_public_key: vec![0x76; 32],
            transport_address_blob: vec![0x77; 32],
            identity_signature,
        }
    }
}
