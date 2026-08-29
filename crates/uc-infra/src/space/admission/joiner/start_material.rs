use async_trait::async_trait;
use rand::RngCore;
use serde::Serialize;
use uc_application::deps::{
    JoinerStartMaterial, JoinerStartMaterialError, JoinerStartMaterialPort,
};
use uc_application::facade::JoinSpaceInput;
use uc_core::ids::DeviceId;
use uc_core::membership::{
    AdmissionEncryptedPasswordEquivalent, AdmissionIdentitySignature, AdmissionJoinRequestV1,
    AdmissionJoinerPrivateState, AdmissionKeyPackage, AdmissionMessageId,
    AdmissionRecoveryPublicKey, AdmissionRole, JoinId, MembershipCredential, SpaceAdmissionBodyV1,
    SpaceAdmissionEnvelopeV1, SpaceAdmissionId, SpaceAdmissionRoute, UnreadableHistoryPolicy,
    ED25519_SIGNATURE_ALGORITHM_V1,
};
use x25519_dalek::{PublicKey as RecoveryPublicKey, StaticSecret as RecoverySecret};
use zeroize::Zeroizing;

use crate::security::SpaceAdmissionAuth;
use crate::space::decode_invitation_entry;
use crate::space::security::mls_group::MlsGroupEngine;

const JOINER_PRIVATE_STATE_FORMAT_V1: u16 = 1;
const IDENTITY_SIGNATURE_CONTEXT_V1: &[u8] = b"uc-space-admission-join-request-v1";

/// Infra owns the complete, one-shot construction of a Joiner's initial admission material.
pub struct DefaultJoinerStartMaterial {
    device_id: DeviceId,
}

impl DefaultJoinerStartMaterial {
    pub fn new(device_id: DeviceId) -> Self {
        Self { device_id }
    }
}

#[derive(Serialize)]
struct JoinerPrivateStateV1<'a> {
    format_version: u16,
    mls_state: &'a [u8],
    recovery_secret: &'a [u8; 32],
}

#[derive(Serialize)]
struct IdentitySignaturePayloadV1<'a> {
    context: &'static [u8],
    admission_id: &'a [u8; 32],
    invitation_id: &'a [u8; 32],
    device_id: &'a str,
    key_package: &'a [u8],
    recovery_public_key: &'a [u8; 32],
    preserve_unreadable_history: bool,
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
        let signature_payload = postcard::to_stdvec(&IdentitySignaturePayloadV1 {
            context: IDENTITY_SIGNATURE_CONTEXT_V1,
            admission_id: admission_id.as_bytes(),
            invitation_id: decoded.invitation_id().as_bytes(),
            device_id: self.device_id.as_str(),
            key_package: &pending.key_package,
            recovery_public_key: &recovery_public_bytes,
            preserve_unreadable_history: input.preserve_unreadable_history,
        })
        .map_err(|error| JoinerStartMaterialError::unavailable(anyhow::Error::new(error)))?;
        let identity_signature =
            MlsGroupEngine::sign_pending_member_payload(&pending.client_state, &signature_payload)
                .map_err(|error| {
                    JoinerStartMaterialError::unavailable(anyhow::Error::new(error))
                })?;

        let request = AdmissionJoinRequestV1::new(
            decoded.invitation_id(),
            self.device_id.clone(),
            MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, signing_public_key),
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

        let derived_password = SpaceAdmissionAuth::derive_password_equivalent(
            input.passphrase.expose().as_bytes(),
            decoded.invitation_id(),
        );
        let password_equivalent =
            AdmissionEncryptedPasswordEquivalent::from_bytes(derived_password.as_bytes().to_vec())
                .map_err(|error| {
                    JoinerStartMaterialError::unavailable(anyhow::Error::new(error))
                })?;
        let route = SpaceAdmissionRoute::from_bytes(decoded.route().to_vec())
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
        let adapter = DefaultJoinerStartMaterial::new(DeviceId::new("joining-device"));

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
        let adapter = DefaultJoinerStartMaterial::new(DeviceId::new("joining-device"));
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
}
