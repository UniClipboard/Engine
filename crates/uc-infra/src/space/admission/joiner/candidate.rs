use std::sync::Arc;

use async_trait::async_trait;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uc_application::deps::{
    AdmissionSecurityTransitionInput, PrepareJoinerCandidateError, PrepareJoinerCandidatePort,
    PreparedJoinerCandidateMaterial,
};
use uc_core::crypto::domain::Passphrase;
use uc_core::ids::SpaceId;
use uc_core::membership::{
    AdmissionPreparedV1, AdmissionRetryState, AdmissionSignedMembershipHistory,
    AdmissionStagedTarget, AdmissionStagedTargetInput, HistoricalMembershipSignatureVerifier,
    MembershipOperationV2, PendingAdmissionExchange, PreparedAdmissionProofV1,
    SpaceAdmissionBodyV1, SpaceAdmissionEnvelopeV1, SpaceAdmissionMessageKind,
    VersionedMembershipHistory,
};
use uc_core::ports::space::PrepareAdmissionTargetAccessPort;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::space::admission::security::AdmissionSecurityTransitionAdapter;
use crate::space::security::mls_group::{MlsClientState, MlsGroupEngine};

const JOINER_STAGED_INPUT_FORMAT_V1: u16 = 1;
const JOINER_STAGED_TARGET_FORMAT_V2: u16 = 2;

pub struct DefaultJoinerCandidatePreparation {
    history_verifier: Arc<dyn HistoricalMembershipSignatureVerifier>,
    target_access: Arc<dyn PrepareAdmissionTargetAccessPort>,
}

impl DefaultJoinerCandidatePreparation {
    pub fn new(
        history_verifier: Arc<dyn HistoricalMembershipSignatureVerifier>,
        target_access: Arc<dyn PrepareAdmissionTargetAccessPort>,
    ) -> Self {
        Self {
            history_verifier,
            target_access,
        }
    }
}

#[derive(Serialize)]
struct JoinerStagedInputV1<'a> {
    format_version: u16,
    recovery_secret: &'a [u8; 32],
    candidate_digest: [u8; 32],
}

#[derive(Serialize)]
struct JoinerStagedTargetV1<'a> {
    format_version: u16,
    mls_state: &'a [u8],
    recovery_secret: &'a [u8; 32],
    target_access: &'a [u8],
    target_admission_credentials: &'a [u8],
    preserve_unreadable_history: bool,
}

#[derive(Deserialize, Zeroize, ZeroizeOnDrop)]
struct OwnedJoinerPrivateStateV1 {
    format_version: u16,
    mls_state: Vec<u8>,
    recovery_secret: [u8; 32],
    passphrase: Vec<u8>,
}

#[async_trait]
impl PrepareJoinerCandidatePort for DefaultJoinerCandidatePreparation {
    async fn prepare(
        &self,
        preparation: uc_core::membership::JoinerCandidatePreparation<'_>,
        candidate_envelope: &SpaceAdmissionEnvelopeV1,
    ) -> Result<PreparedJoinerCandidateMaterial, PrepareJoinerCandidateError> {
        let original_request = match preparation.join_request().body() {
            SpaceAdmissionBodyV1::JoinRequest(request) => request,
            _ => return Err(PrepareJoinerCandidateError::Invalid),
        };
        let candidate = match candidate_envelope.body() {
            SpaceAdmissionBodyV1::Candidate(candidate) => candidate,
            _ => return Err(PrepareJoinerCandidateError::Invalid),
        };
        if candidate_envelope.header().admission_id()
            != preparation.join_request().header().admission_id()
            || candidate_envelope.header().predecessor_message_id()
                != Some(preparation.join_request().header().message_id())
        {
            return Err(PrepareJoinerCandidateError::Invalid);
        }
        let MembershipOperationV2::AddDevice { admission } = &candidate.candidate_event().operation
        else {
            return Err(PrepareJoinerCandidateError::Invalid);
        };
        let expected_resume_digest: [u8; 32] =
            Sha256::digest(original_request.recovery_public_key().as_bytes()).into();
        if admission.facts != *original_request.identity_facts()
            || admission.membership_credential != *original_request.membership_credential()
            || admission.resume_public_key_digest != expected_resume_digest
            || admission.security_commitment_id
                != candidate.security_commitment().security_commitment_id
        {
            return Err(PrepareJoinerCandidateError::Invalid);
        }
        let mut history = VersionedMembershipHistory::decode_persisted_v2(
            candidate.base_membership_history().as_bytes(),
            self.history_verifier.as_ref(),
        )
        .map_err(PrepareJoinerCandidateError::invalid)?;
        let commitment = candidate.security_commitment();
        if commitment.attempt_id != *candidate_envelope.header().admission_id().as_bytes()
            || commitment.base_history_position
                != history
                    .current_position()
                    .map_err(PrepareJoinerCandidateError::invalid)?
            || commitment.candidate_core_digest
                != candidate
                    .candidate_event()
                    .admission_candidate_core_digest(
                        commitment.attempt_id,
                        original_request.key_package().as_bytes(),
                    )
                    .map_err(PrepareJoinerCandidateError::invalid)?
        {
            return Err(PrepareJoinerCandidateError::Invalid);
        }
        history
            .verify_and_receive_event(
                candidate.candidate_event().clone(),
                self.history_verifier.as_ref(),
            )
            .map_err(PrepareJoinerCandidateError::invalid)?;

        let private: OwnedJoinerPrivateStateV1 =
            postcard::from_bytes(preparation.private_state().as_bytes())
                .map_err(PrepareJoinerCandidateError::invalid)?;
        if private.format_version != 2 {
            return Err(PrepareJoinerCandidateError::Invalid);
        }
        let transition_input = AdmissionSecurityTransitionInput {
            attempt_id: commitment.attempt_id,
            base_history_position: commitment.base_history_position.clone(),
            candidate_core_digest: commitment.candidate_core_digest,
            key_catalog_digest: commitment.key_catalog_digest,
            admission_bundle_digest: commitment.admission_bundle_digest,
        };
        let staged = AdmissionSecurityTransitionAdapter::stage_joiner(
            &private.mls_state,
            original_request.key_package().as_bytes(),
            commitment.lineage_id.as_bytes(),
            candidate.mls_welcome().as_bytes(),
            candidate.mls_commit().as_bytes(),
            &transition_input,
        )
        .map_err(PrepareJoinerCandidateError::invalid)?;
        if staged.public_commitment != *commitment {
            return Err(PrepareJoinerCandidateError::Invalid);
        }
        let passphrase = std::str::from_utf8(&private.passphrase)
            .map_err(PrepareJoinerCandidateError::invalid)?;
        let target_access = self
            .target_access
            .prepare_target_access(
                &SpaceId::from_str(&commitment.lineage_id),
                &Passphrase::new(passphrase),
            )
            .await
            .map_err(PrepareJoinerCandidateError::unavailable)?;
        let target_admission_credentials =
            crate::space::admission::credentials::prepare_registration(&Passphrase::new(
                passphrase,
            ))
            .map_err(PrepareJoinerCandidateError::unavailable)?;

        let mut proof = PreparedAdmissionProofV1::new(
            commitment.attempt_id,
            commitment.lineage_id.clone(),
            commitment.base_history_position.clone(),
            candidate.candidate_event().event_id(),
            candidate.candidate_event().resulting_members_digest,
            commitment.security_commitment_id,
            admission.facts.member_instance,
            admission.membership_credential.credential_id,
            Vec::new(),
        );
        proof.signature = MlsGroupEngine::sign_member_payload(
            &MlsClientState::from_bytes(staged.staged_state.clone()),
            &proof.signing_payload(),
        )
        .map_err(PrepareJoinerCandidateError::invalid)?;
        let prepared = SpaceAdmissionEnvelopeV1::new(
            candidate_envelope.header().admission_id(),
            uc_core::membership::AdmissionRole::Joiner,
            1,
            mint_message_id(),
            Some(candidate_envelope.header().message_id()),
            SpaceAdmissionBodyV1::Prepared(AdmissionPreparedV1::new(proof)),
        )
        .map_err(PrepareJoinerCandidateError::invalid)?;
        let prepared_exchange = PendingAdmissionExchange::new(
            uc_core::membership::SpaceAdmissionRoute::from_bytes(
                candidate.continuation_route().as_bytes().to_vec(),
            )
            .map_err(PrepareJoinerCandidateError::invalid)?,
            prepared,
            SpaceAdmissionMessageKind::Commit,
            AdmissionRetryState::new(0, 0).map_err(PrepareJoinerCandidateError::invalid)?,
        )
        .map_err(PrepareJoinerCandidateError::invalid)?;
        let staged_input = AdmissionStagedTargetInput::from_bytes(
            postcard::to_stdvec(&JoinerStagedInputV1 {
                format_version: JOINER_STAGED_INPUT_FORMAT_V1,
                recovery_secret: &private.recovery_secret,
                candidate_digest: candidate_digest(candidate),
            })
            .map_err(PrepareJoinerCandidateError::unavailable)?,
        )
        .map_err(PrepareJoinerCandidateError::invalid)?;
        let staged_target = AdmissionStagedTarget::from_bytes(
            postcard::to_stdvec(&JoinerStagedTargetV1 {
                format_version: JOINER_STAGED_TARGET_FORMAT_V2,
                mls_state: &staged.staged_state,
                recovery_secret: &private.recovery_secret,
                target_access: target_access.as_bytes(),
                target_admission_credentials: &target_admission_credentials,
                preserve_unreadable_history: matches!(
                    original_request.unreadable_history_policy(),
                    uc_core::membership::UnreadableHistoryPolicy::Preserve
                ),
            })
            .map_err(PrepareJoinerCandidateError::unavailable)?,
        )
        .map_err(PrepareJoinerCandidateError::invalid)?;
        let verified_history = AdmissionSignedMembershipHistory::from_bytes(
            history
                .encode_persisted_v2()
                .map_err(PrepareJoinerCandidateError::invalid)?,
        )
        .map_err(PrepareJoinerCandidateError::invalid)?;

        Ok(PreparedJoinerCandidateMaterial::new(
            staged_input,
            verified_history,
            staged_target,
            prepared_exchange,
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

fn candidate_digest(candidate: &uc_core::membership::AdmissionCandidateV1) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"uc-space-admission-candidate-state-v1");
    hasher.update(candidate.candidate_event().event_id().as_bytes());
    hasher.update(candidate.security_commitment().security_commitment_id);
    hasher.update(candidate.mls_commit().as_bytes());
    hasher.update(candidate.mls_welcome().as_bytes());
    hasher.finalize().into()
}

#[cfg(test)]
mod tests {
    use uc_application::deps::PrepareJoinerAppliedPort;
    use uc_core::ids::DeviceId;
    use uc_core::membership::{
        AdmissionCandidateV1, AdmissionChangeFacts, AdmissionCommitV1,
        AdmissionContinuationCredential, AdmissionContinuationRoute,
        AdmissionEncryptedPasswordEquivalent, AdmissionIdentitySignature, AdmissionJoinRequestV1,
        AdmissionJoinerPrivateState, AdmissionKeyPackage, AdmissionMessageId, AdmissionMlsCommit,
        AdmissionMlsWelcome, AdmissionPeerBinding, AdmissionRecoveryPublicKey, AdmissionRole,
        AdmissionSealedRecoveryMaterial, AdmissionSourceSnapshot, BaseMembershipHistoryPosition,
        InvitationId, JoinId, JoinerAdmission, MembershipCredential, PendingAdmissionExchange,
        SpaceAdmissionId, SpaceAdmissionRoute, UnreadableHistoryPolicy,
        ED25519_SIGNATURE_ALGORITHM_V1,
    };
    use uc_core::ports::space::SpaceAccessError;
    use uc_core::security::IdentityFingerprint;
    use uc_core::space_access::PreparedAdmissionTargetAccess;

    use super::*;
    use crate::space::admission::joiner::DefaultJoinerAppliedPreparation;
    use crate::space::security::mls_group::MlsGroupEngine;
    use crate::space::OpenMlsHistoricalSignatureVerifier;

    struct FixedTargetAccess;

    #[async_trait]
    impl PrepareAdmissionTargetAccessPort for FixedTargetAccess {
        async fn prepare_target_access(
            &self,
            _target_space_id: &SpaceId,
            _passphrase: &Passphrase,
        ) -> Result<PreparedAdmissionTargetAccess, SpaceAccessError> {
            Ok(PreparedAdmissionTargetAccess::from_bytes(vec![0x40; 96]))
        }
    }

    #[tokio::test]
    async fn production_joiner_candidate_validates_real_openmls_and_prepares_reply() {
        let sponsor_state = MlsGroupEngine::create_sponsor(b"space-a", b"sponsor-device")
            .expect("Sponsor MLS state");
        let sponsor_public =
            MlsGroupEngine::signing_public_key(&sponsor_state).expect("Sponsor signing public key");
        let sponsor_credential =
            MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, sponsor_public);
        let sponsor_device = DeviceId::new("sponsor-device");
        let mut sponsor_facts =
            identity_facts(sponsor_device.clone(), &sponsor_credential, Vec::new());
        sponsor_facts.identity_signature =
            MlsGroupEngine::sign_member_payload(&sponsor_state, &sponsor_facts.signing_payload())
                .expect("Sponsor signs identity facts");
        let history = VersionedMembershipHistory::new_single_member_root(
            "space-a".to_owned(),
            sponsor_facts.clone(),
            sponsor_credential.clone(),
        )
        .expect("valid base history");

        let pending = MlsGroupEngine::prepare_join(b"joiner-device").expect("Joiner MLS state");
        let joiner_public = MlsGroupEngine::signing_public_key(&pending.client_state)
            .expect("Joiner signing public key");
        let joiner_credential =
            MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, joiner_public);
        let joiner_device = DeviceId::new("joiner-device");
        let mut joiner_facts =
            identity_facts(joiner_device.clone(), &joiner_credential, Vec::new());
        joiner_facts.identity_signature = MlsGroupEngine::sign_pending_member_payload(
            &pending.client_state,
            &joiner_facts.signing_payload(),
        )
        .expect("Joiner signs identity facts");

        let admission_id = SpaceAdmissionId::from_bytes([0x41; 32]).expect("valid admission id");
        let invitation_id = InvitationId::from_bytes([0x42; 32]).expect("valid invitation id");
        let recovery_public = [0x43; 32];
        let request = join_request(
            admission_id,
            invitation_id,
            joiner_device,
            joiner_facts.clone(),
            joiner_credential.clone(),
            pending.key_package.clone(),
            recovery_public,
        );
        let draft = history
            .create_unsigned_local_admission_event(
                sponsor_facts.member_instance,
                &sponsor_credential,
                joiner_facts,
                joiner_credential,
                Sha256::digest(recovery_public).into(),
                [0x44; 16],
            )
            .expect("candidate draft");
        let candidate_core_digest = draft
            .admission_candidate_core_digest(*admission_id.as_bytes(), &pending.key_package)
            .expect("candidate core digest");
        let input = AdmissionSecurityTransitionInput {
            attempt_id: *admission_id.as_bytes(),
            base_history_position: history.current_position().expect("history position"),
            candidate_core_digest,
            key_catalog_digest: [0x45; 32],
            admission_bundle_digest: [0x46; 32],
        };
        let prepared_security = AdmissionSecurityTransitionAdapter::prepare_sponsor(
            sponsor_state.as_bytes(),
            b"joiner-device",
            &pending.key_package,
            &input,
        )
        .expect("real OpenMLS Candidate security");
        let mut candidate_event = history
            .finalize_unsigned_local_admission_event(
                draft,
                &pending.key_package,
                &prepared_security.public_commitment,
            )
            .expect("bind Candidate security");
        candidate_event.signature =
            MlsGroupEngine::sign_member_payload(&sponsor_state, &candidate_event.signing_payload())
                .expect("Sponsor signs Candidate event");
        let candidate = AdmissionCandidateV1::new(
            AdmissionSignedMembershipHistory::from_bytes(
                history.encode_persisted_v2().expect("history encodes"),
            )
            .expect("history artifact"),
            candidate_event,
            prepared_security.public_commitment,
            AdmissionMlsCommit::from_bytes(prepared_security.commit).expect("MLS commit"),
            AdmissionMlsWelcome::from_bytes(prepared_security.welcome).expect("MLS welcome"),
            AdmissionContinuationRoute::from_bytes(b"continuation-route".to_vec())
                .expect("continuation route"),
        )
        .expect("valid Candidate");
        let candidate_envelope = SpaceAdmissionEnvelopeV1::new(
            admission_id,
            AdmissionRole::Sponsor,
            0,
            AdmissionMessageId::from_bytes([0x47; 32]).expect("Candidate message id"),
            Some(request.header().message_id()),
            SpaceAdmissionBodyV1::Candidate(candidate),
        )
        .expect("Candidate envelope");
        let private_state = AdmissionJoinerPrivateState::from_bytes(
            postcard::to_stdvec(&super::super::start_material::JoinerPrivateStateV1 {
                format_version: 2,
                mls_state: pending.client_state.as_bytes(),
                recovery_secret: &[0x48; 32],
                passphrase: b"target passphrase",
            })
            .expect("private state encodes"),
        )
        .expect("private state artifact");
        let joiner = JoinerAdmission::start_join(
            admission_id,
            JoinId::from_bytes([0x49; 16]).expect("join id"),
            1,
            AdmissionSourceSnapshot::from_bytes(vec![0x4a; 32]).expect("source snapshot"),
            private_state,
            AdmissionEncryptedPasswordEquivalent::from_bytes(vec![0x4b; 64])
                .expect("password equivalent"),
            PendingAdmissionExchange::new(
                SpaceAdmissionRoute::from_bytes(b"sponsor-route".to_vec()).expect("route"),
                request,
                SpaceAdmissionMessageKind::Candidate,
                AdmissionRetryState::new(0, 0).expect("retry state"),
            )
            .expect("pending exchange"),
        )
        .expect("Joiner starts")
        .into_replacement()
        .with_authenticated_channel(
            AdmissionPeerBinding::new(
                uc_core::membership::AdmissionChannelPeerId::from_bytes([0x4c; 32])
                    .expect("local peer"),
                uc_core::membership::AdmissionChannelPeerId::from_bytes([0x4d; 32])
                    .expect("remote peer"),
            )
            .expect("peer binding"),
            AdmissionContinuationCredential::from_bytes(vec![0x4e; 64])
                .expect("continuation credential"),
        )
        .expect("authenticated channel")
        .into_replacement();
        let adapter = DefaultJoinerCandidatePreparation::new(
            Arc::new(OpenMlsHistoricalSignatureVerifier),
            Arc::new(FixedTargetAccess),
        );

        let prepared_material = adapter
            .prepare(
                joiner
                    .joiner_candidate_preparation()
                    .expect("authenticated Joiner preparation"),
                &candidate_envelope,
            )
            .await
            .expect("production Joiner prepares Candidate");
        let (staged_input, verified_history, staged_target, prepared_exchange) =
            prepared_material.into_parts();
        let prepared_message_id = prepared_exchange.request_envelope().header().message_id();
        let exact_candidate = copy_candidate(match candidate_envelope.body() {
            SpaceAdmissionBodyV1::Candidate(candidate) => candidate,
            _ => panic!("fixture must be Candidate"),
        });
        let mut target_history = VersionedMembershipHistory::decode_persisted_v2(
            exact_candidate.base_membership_history().as_bytes(),
            &OpenMlsHistoricalSignatureVerifier,
        )
        .expect("base history verifies");
        target_history
            .verify_and_receive_event(
                exact_candidate.candidate_event().clone(),
                &OpenMlsHistoricalSignatureVerifier,
            )
            .expect("Candidate event extends history");
        let commit = SpaceAdmissionEnvelopeV1::new(
            admission_id,
            AdmissionRole::Sponsor,
            1,
            AdmissionMessageId::from_bytes([0x54; 32]).expect("Commit message id"),
            Some(prepared_message_id),
            SpaceAdmissionBodyV1::Commit(AdmissionCommitV1::new(
                exact_candidate,
                AdmissionSignedMembershipHistory::from_bytes(
                    target_history
                        .encode_persisted_v2()
                        .expect("history encodes"),
                )
                .expect("target history artifact"),
                AdmissionSealedRecoveryMaterial::from_bytes(vec![0x55; 64])
                    .expect("sealed recovery fixture"),
            )),
        )
        .expect("valid Commit");
        let prepared_joiner = joiner
            .accept_candidate(candidate_envelope, [0x56; 32], staged_input)
            .expect("Joiner accepts Candidate")
            .into_replacement()
            .prepare_candidate(verified_history, staged_target, prepared_exchange)
            .expect("Joiner saves Prepared")
            .into_replacement();
        let committed_joiner = prepared_joiner
            .accept_commit(commit, [0x57; 32])
            .expect("Joiner accepts Commit")
            .into_replacement();
        let applied =
            DefaultJoinerAppliedPreparation::new(Arc::new(OpenMlsHistoricalSignatureVerifier))
                .prepare(
                    admission_id,
                    committed_joiner
                        .joiner_applied_preparation()
                        .expect("Committed Joiner exposes Applied preparation"),
                )
                .await
                .expect("production Joiner prepares Applied")
                .into_pending_exchange();
        assert_eq!(
            applied.request_envelope().kind(),
            SpaceAdmissionMessageKind::Applied
        );
        assert_eq!(
            applied.exact_expected_reply_kind(),
            SpaceAdmissionMessageKind::Complete
        );
    }

    fn copy_candidate(candidate: &AdmissionCandidateV1) -> AdmissionCandidateV1 {
        AdmissionCandidateV1::new(
            AdmissionSignedMembershipHistory::from_bytes(
                candidate.base_membership_history().as_bytes().to_vec(),
            )
            .expect("base history copy"),
            candidate.candidate_event().clone(),
            candidate.security_commitment().clone(),
            AdmissionMlsCommit::from_bytes(candidate.mls_commit().as_bytes().to_vec())
                .expect("MLS commit copy"),
            AdmissionMlsWelcome::from_bytes(candidate.mls_welcome().as_bytes().to_vec())
                .expect("MLS welcome copy"),
            AdmissionContinuationRoute::from_bytes(
                candidate.continuation_route().as_bytes().to_vec(),
            )
            .expect("continuation route copy"),
        )
        .expect("Candidate copy")
    }

    #[allow(clippy::too_many_arguments)]
    fn join_request(
        admission_id: SpaceAdmissionId,
        invitation_id: InvitationId,
        device_id: DeviceId,
        facts: AdmissionChangeFacts,
        credential: MembershipCredential,
        key_package: Vec<u8>,
        recovery_public: [u8; 32],
    ) -> SpaceAdmissionEnvelopeV1 {
        let signature = facts.identity_signature.clone();
        SpaceAdmissionEnvelopeV1::new(
            admission_id,
            AdmissionRole::Joiner,
            0,
            AdmissionMessageId::from_bytes([0x51; 32]).expect("JoinRequest message id"),
            None,
            SpaceAdmissionBodyV1::JoinRequest(
                AdmissionJoinRequestV1::new(
                    invitation_id,
                    device_id,
                    facts,
                    credential,
                    AdmissionKeyPackage::from_bytes(key_package).expect("key package"),
                    AdmissionRecoveryPublicKey::from_bytes(recovery_public)
                        .expect("recovery public key"),
                    AdmissionIdentitySignature::from_bytes(signature).expect("identity signature"),
                    UnreadableHistoryPolicy::Discard,
                )
                .expect("valid JoinRequest"),
            ),
        )
        .expect("JoinRequest envelope")
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
                .expect("fingerprint"),
            transport_public_key: vec![0x52; 32],
            transport_address_blob: vec![0x53; 32],
            identity_signature,
        }
    }
}
