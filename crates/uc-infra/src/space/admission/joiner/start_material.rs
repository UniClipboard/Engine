use async_trait::async_trait;
use rand::RngCore;
use serde::Serialize;
use uc_application::deps::{
    JoinerStartMaterial, JoinerStartMaterialError, JoinerStartMaterialPort,
};
use uc_application::facade::JoinSpaceInput;
use uc_core::ids::DeviceId;
use uc_core::membership::{
    AdmissionChangeFacts, AdmissionEncryptedPasswordEquivalent, AdmissionIdentitySignature,
    AdmissionJoinRequestV1, AdmissionJoinerPrivateState, AdmissionKeyPackage, AdmissionMessageId,
    AdmissionRecoveryPublicKey, AdmissionRole, JoinId, MembershipCredential, SpaceAdmissionBodyV1,
    SpaceAdmissionEnvelopeV1, SpaceAdmissionId, SpaceAdmissionRoute, UnreadableHistoryPolicy,
    ED25519_SIGNATURE_ALGORITHM_V1,
};
use uc_core::security::IdentityFingerprint;
use x25519_dalek::{PublicKey as RecoveryPublicKey, StaticSecret as RecoverySecret};
use zeroize::Zeroizing;

use crate::space::decode_invitation_entry;
use crate::space::security::mls_group::MlsGroupEngine;

const JOINER_PRIVATE_STATE_FORMAT_V1: u16 = 1;

/// Infra owns the complete, one-shot construction of a Joiner's initial admission material.
pub struct DefaultJoinerStartMaterial {
    device_id: DeviceId,
    device_name: String,
    identity_fingerprint: IdentityFingerprint,
    transport_public_key: Vec<u8>,
    transport_address_blob: Vec<u8>,
}

impl DefaultJoinerStartMaterial {
    pub fn new(
        device_id: DeviceId,
        device_name: String,
        identity_fingerprint: IdentityFingerprint,
        transport_public_key: Vec<u8>,
        transport_address_blob: Vec<u8>,
    ) -> Self {
        Self {
            device_id,
            device_name,
            identity_fingerprint,
            transport_public_key,
            transport_address_blob,
        }
    }
}

#[derive(Serialize)]
pub(super) struct JoinerPrivateStateV1<'a> {
    pub(super) format_version: u16,
    pub(super) mls_state: &'a [u8],
    pub(super) recovery_secret: &'a [u8; 32],
}

#[async_trait]
impl JoinerStartMaterialPort for DefaultJoinerStartMaterial {
    async fn create(
        &self,
        input: &JoinSpaceInput,
    ) -> Result<JoinerStartMaterial, JoinerStartMaterialError> {
        let decoded = decode_invitation_entry(
            input.invitation_code.as_str(),
            chrono::Utc::now().timestamp_millis(),
        )
        .map_err(|_| JoinerStartMaterialError::InvalidInvitation)?
        .ok_or(JoinerStartMaterialError::InvalidInvitation)?;

        let admission_id = mint_admission_id();
        let join_id = mint_join_id();
        let pending = MlsGroupEngine::prepare_join(self.device_id.as_str().as_bytes())
            .map_err(|error| JoinerStartMaterialError::unavailable(anyhow::Error::new(error)))?;
        let signing_public_key = MlsGroupEngine::signing_public_key(&pending.client_state)
            .map_err(|error| JoinerStartMaterialError::unavailable(anyhow::Error::new(error)))?;

        let mut recovery_secret_bytes = Zeroizing::new([0u8; 32]);
        rand::rng().fill_bytes(recovery_secret_bytes.as_mut());
        let recovery_secret = RecoverySecret::from(*recovery_secret_bytes);
        let recovery_public_bytes = RecoveryPublicKey::from(&recovery_secret).to_bytes();
        let recovery_public_key = AdmissionRecoveryPublicKey::from_bytes(recovery_public_bytes)
            .ok_or_else(|| {
                JoinerStartMaterialError::unavailable(anyhow::anyhow!(
                    "generated recovery public key is invalid"
                ))
            })?;

        let policy = if input.preserve_unreadable_history {
            UnreadableHistoryPolicy::Preserve
        } else {
            UnreadableHistoryPolicy::Discard
        };
        let credential =
            MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, signing_public_key);
        let mut identity_facts = AdmissionChangeFacts {
            member_instance: credential.member_instance_id(&self.device_id),
            device_id: self.device_id.clone(),
            device_name: self.device_name.clone(),
            identity_fingerprint: self.identity_fingerprint.clone(),
            transport_public_key: self.transport_public_key.clone(),
            transport_address_blob: self.transport_address_blob.clone(),
            identity_signature: Vec::new(),
        };
        let identity_signature = MlsGroupEngine::sign_pending_member_payload(
            &pending.client_state,
            &identity_facts.signing_payload(),
        )
        .map_err(|error| JoinerStartMaterialError::unavailable(anyhow::Error::new(error)))?;
        identity_facts.identity_signature = identity_signature.clone();

        let request = AdmissionJoinRequestV1::new(
            decoded.invitation_id(),
            self.device_id.clone(),
            identity_facts,
            credential,
            AdmissionKeyPackage::from_bytes(pending.key_package.clone()).map_err(|error| {
                JoinerStartMaterialError::unavailable(anyhow::Error::new(error))
            })?,
            recovery_public_key,
            AdmissionIdentitySignature::from_bytes(identity_signature).map_err(|error| {
                JoinerStartMaterialError::unavailable(anyhow::Error::new(error))
            })?,
            policy,
        )
        .map_err(|error| JoinerStartMaterialError::unavailable(anyhow::Error::new(error)))?;
        let join_request = SpaceAdmissionEnvelopeV1::new(
            admission_id,
            AdmissionRole::Joiner,
            0,
            mint_message_id(),
            None,
            SpaceAdmissionBodyV1::JoinRequest(request),
        )
        .map_err(|error| JoinerStartMaterialError::unavailable(anyhow::Error::new(error)))?;

        let private_state = postcard::to_stdvec(&JoinerPrivateStateV1 {
            format_version: JOINER_PRIVATE_STATE_FORMAT_V1,
            mls_state: pending.client_state.as_bytes(),
            recovery_secret: &recovery_secret_bytes,
        })
        .map_err(|error| JoinerStartMaterialError::unavailable(anyhow::Error::new(error)))?;
        let private_state = AdmissionJoinerPrivateState::from_bytes(private_state)
            .map_err(|error| JoinerStartMaterialError::unavailable(anyhow::Error::new(error)))?;

        // OPAQUE already binds the transcript to the invitation id. The
        // password input must remain the same value used by the Sponsor's
        // Space-scoped registration created during initialize/unlock.
        let password_equivalent = AdmissionEncryptedPasswordEquivalent::from_bytes(
            input.passphrase.expose().as_bytes().to_vec(),
        )
        .map_err(|error| JoinerStartMaterialError::unavailable(anyhow::Error::new(error)))?;
        let route = crate::network::iroh::space_admission::encode_space_admission_route_bytes(
            decoded.route(),
            Some(decoded.invitation_id()),
        )
        .and_then(|route| {
            SpaceAdmissionRoute::from_bytes(route)
                .map_err(|_| uc_application::deps::SpaceAdmissionTransportError::Unavailable)
        })
        .map_err(|_| JoinerStartMaterialError::InvalidInvitation)?;

        Ok(JoinerStartMaterial::new(
            admission_id,
            join_id,
            route,
            join_request,
            private_state,
            password_equivalent,
        ))
    }
}

fn mint_admission_id() -> SpaceAdmissionId {
    loop {
        let mut bytes = [0u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        if let Some(id) = SpaceAdmissionId::from_bytes(bytes) {
            return id;
        }
    }
}

fn mint_join_id() -> JoinId {
    loop {
        let mut bytes = [0u8; 16];
        rand::rng().fill_bytes(&mut bytes);
        if let Some(id) = JoinId::from_bytes(bytes) {
            return id;
        }
    }
}

fn mint_message_id() -> AdmissionMessageId {
    loop {
        let mut bytes = [0u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        if let Some(id) = AdmissionMessageId::from_bytes(bytes) {
            return id;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use uc_core::crypto::domain::Passphrase;
    use uc_core::pairing::InvitationCode;

    use super::*;
    use crate::space::encode_full_invitation;

    #[tokio::test]
    async fn complete_joiner_start_material_is_created_from_a_full_invitation() {
        let invitation_id = uc_core::membership::InvitationId::from_bytes([0x61; 32])
            .expect("valid invitation id fixture");
        let invitation =
            encode_full_invitation(invitation_id, b"opaque-sponsor-route", 1_900_000_000_000)
                .expect("valid full invitation fixture");
        let adapter = adapter();

        let material = adapter
            .create(&JoinSpaceInput {
                invitation_code: InvitationCode::new(invitation.as_str()),
                device_name: None,
                passphrase: Passphrase::new("correct horse battery staple"),
                preserve_unreadable_history: true,
            })
            .await;

        assert!(material.is_ok());
    }

    #[tokio::test]
    async fn invalid_full_invitation_is_rejected_without_a_dependency_error() {
        let adapter = adapter();
        let error = adapter
            .create(&JoinSpaceInput {
                invitation_code: InvitationCode::new("ucspace1_invalid"),
                device_name: None,
                passphrase: Passphrase::new("secret"),
                preserve_unreadable_history: false,
            })
            .await
            .err()
            .expect("invalid full invitation must fail");

        assert!(matches!(error, JoinerStartMaterialError::InvalidInvitation));
        assert!(error.source().is_none());
    }

    fn adapter() -> DefaultJoinerStartMaterial {
        DefaultJoinerStartMaterial::new(
            DeviceId::new("joining-device"),
            "Joining device".to_owned(),
            IdentityFingerprint::from_display_string("ABCD-EFGH-IJKL-MNOP")
                .expect("valid fingerprint fixture"),
            vec![0x71; 32],
            vec![0x72; 32],
        )
    }
}
