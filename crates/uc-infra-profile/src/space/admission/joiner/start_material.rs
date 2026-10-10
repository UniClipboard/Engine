use std::sync::Arc;

use async_trait::async_trait;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use uc_application::deps::{
    JoinerStartMaterial, JoinerStartMaterialError, JoinerStartMaterialPort,
    JoinerTransportMaterialPort,
};
use uc_application::facade::JoinSpaceInput;
use uc_core::crypto::domain::Passphrase;
use uc_core::ids::DeviceId;
use uc_core::membership::{
    AdmissionChangeFacts, AdmissionEncryptedPasswordEquivalent, AdmissionIdentitySignature,
    AdmissionJoinRequestV1, AdmissionJoinerPrivateState, AdmissionKeyPackage, AdmissionMessageId,
    AdmissionRecoveryPublicKey, AdmissionRole, JoinId, MembershipCredential, SpaceAdmissionBodyV1,
    SpaceAdmissionEnvelopeV1, SpaceAdmissionId, SpaceAdmissionProtocolVersion, SpaceAdmissionRoute,
    UnreadableHistoryPolicy, ED25519_SIGNATURE_ALGORITHM_V1,
};
use uc_core::pairing::InvitationCode;
use uc_core::ports::SettingsPort;
use uc_observability_contract::diagnostics::connectivity::{observe_local_result, LocalWorkStep};
use uc_sync_protocol::full_invitation::decode_invitation_entry;
use x25519_dalek::{PublicKey as RecoveryPublicKey, StaticSecret as RecoverySecret};
use zeroize::Zeroizing;

use uc_infra_crypto::mls_group::MlsGroupEngine;

const JOINER_PRIVATE_STATE_FORMAT_V2: u16 = 2;

/// Infra 一次性生成 Joiner 准入材料，并在签名前采样当前传输地址。
pub struct DefaultJoinerStartMaterial {
    device_id: DeviceId,
    settings: Arc<dyn SettingsPort>,
    transport_material: Arc<dyn JoinerTransportMaterialPort>,
}

impl DefaultJoinerStartMaterial {
    /// 注入当前材料采样能力；完整准入材料仍由本 adapter 生成。
    pub fn new(
        device_id: DeviceId,
        settings: Arc<dyn SettingsPort>,
        transport_material: Arc<dyn JoinerTransportMaterialPort>,
    ) -> Self {
        Self {
            device_id,
            settings,
            transport_material,
        }
    }
}

#[derive(Serialize)]
pub(super) struct JoinerPrivateStateV1<'a> {
    pub(super) format_version: u16,
    pub(super) mls_state: &'a [u8],
    pub(super) recovery_secret: &'a [u8; 32],
    pub(super) passphrase: &'a [u8],
}

impl DefaultJoinerStartMaterial {
    /// 在签名前取得当前材料；恢复调用使用已解析的准入标识。
    async fn create_with_ids(
        &self,
        input: &JoinSpaceInput,
        admission_id: SpaceAdmissionId,
        join_id: JoinId,
    ) -> Result<JoinerStartMaterial, JoinerStartMaterialError> {
        observe_local_result(LocalWorkStep::JoinerPrepareStart, async {
            let decoded = decode_invitation_entry(
                input.invitation_code.as_str(),
                chrono::Utc::now().timestamp_millis(),
            )
            .map_err(JoinerStartMaterialError::invalid_invitation_from)?
            .ok_or_else(JoinerStartMaterialError::invalid_invitation)?;

            let settings = self.settings.load().await.map_err(|error| {
                JoinerStartMaterialError::unavailable(
                    error.context("load the local device name for Space admission"),
                )
            })?;
            let device_name = settings
                .general
                .device_name
                .filter(|name| !name.trim().is_empty())
                .ok_or_else(|| {
                    JoinerStartMaterialError::unavailable(anyhow::anyhow!(
                        "the local device name is unavailable for Space admission"
                    ))
                })?;
            let pending = MlsGroupEngine::prepare_join(self.device_id.as_str().as_bytes())
                .map_err(|error| {
                    JoinerStartMaterialError::unavailable(anyhow::Error::new(error))
                })?;
            let signing_public_key = MlsGroupEngine::signing_public_key(&pending.client_state)
                .map_err(|error| {
                    JoinerStartMaterialError::unavailable(anyhow::Error::new(error))
                })?;

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
            // 同一次快照提供身份公钥和当前地址；签名后由准入恢复流程重放原材料。
            let transport = self.transport_material.prepare()?;
            let mut identity_facts = AdmissionChangeFacts {
                member_instance: credential.member_instance_id(&self.device_id),
                device_id: self.device_id.clone(),
                device_name,
                identity_fingerprint: transport.identity_fingerprint,
                transport_public_key: transport.transport_public_key,
                transport_address_blob: transport.transport_address_blob,
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
            let join_request = SpaceAdmissionEnvelopeV1::new_with_version(
                SpaceAdmissionProtocolVersion::CURRENT,
                admission_id,
                AdmissionRole::Joiner,
                0,
                mint_message_id(),
                None,
                SpaceAdmissionBodyV1::JoinRequest(request),
            )
            .map_err(|error| JoinerStartMaterialError::unavailable(anyhow::Error::new(error)))?;

            let private_state = postcard::to_stdvec(&JoinerPrivateStateV1 {
                format_version: JOINER_PRIVATE_STATE_FORMAT_V2,
                mls_state: pending.client_state.as_bytes(),
                recovery_secret: &recovery_secret_bytes,
                passphrase: input.passphrase.expose().as_bytes(),
            })
            .map_err(|error| JoinerStartMaterialError::unavailable(anyhow::Error::new(error)))?;
            let private_state =
                AdmissionJoinerPrivateState::from_bytes(private_state).map_err(|error| {
                    JoinerStartMaterialError::unavailable(anyhow::Error::new(error))
                })?;

            // OPAQUE already binds the transcript to the invitation id. The
            // password input must remain the same value used by the Sponsor's
            // Space-scoped registration created during initialize/unlock.
            let password_equivalent = AdmissionEncryptedPasswordEquivalent::from_bytes(
                input.passphrase.expose().as_bytes().to_vec(),
            )
            .map_err(|error| JoinerStartMaterialError::unavailable(anyhow::Error::new(error)))?;
            let route = preserve_admission_route(decoded.route())?;

            Ok(JoinerStartMaterial::new(
                admission_id,
                join_id,
                route,
                join_request,
                private_state,
                password_equivalent,
            ))
        })
        .await
    }
}

#[derive(Deserialize)]
struct OwnedJoinerStartContextV1 {
    format_version: u16,
    passphrase: Vec<u8>,
    preserve_unreadable_history: bool,
}

#[async_trait]
impl JoinerStartMaterialPort for DefaultJoinerStartMaterial {
    async fn create(
        &self,
        input: &JoinSpaceInput,
    ) -> Result<JoinerStartMaterial, JoinerStartMaterialError> {
        self.create_with_ids(input, mint_admission_id(), mint_join_id())
            .await
    }

    /// 为已解析邀请生成签名材料，沿用已有准入标识。
    async fn create_resolved(
        &self,
        admission_id: SpaceAdmissionId,
        join_id: JoinId,
        invitation: &uc_core::pairing::invitation::FullInvitation,
        start_context: &uc_core::membership::AdmissionJoinerStartContext,
    ) -> Result<JoinerStartMaterial, JoinerStartMaterialError> {
        let context: OwnedJoinerStartContextV1 = postcard::from_bytes(start_context.as_bytes())
            .map_err(|error| JoinerStartMaterialError::unavailable(anyhow::Error::new(error)))?;
        if context.format_version != 1 {
            return Err(JoinerStartMaterialError::invalid_invitation());
        }
        let passphrase = String::from_utf8(context.passphrase)
            .map_err(|error| JoinerStartMaterialError::unavailable(anyhow::Error::new(error)))?;
        let input = JoinSpaceInput {
            invitation_code: InvitationCode::new(invitation.as_str()),
            device_name: None,
            passphrase: Passphrase::new(passphrase),
            preserve_unreadable_history: context.preserve_unreadable_history,
        };
        self.create_with_ids(&input, admission_id, join_id).await
    }
}

/// 保留邀请携带的路由并通过领域校验。
fn preserve_admission_route(
    encoded_route: &[u8],
) -> Result<SpaceAdmissionRoute, JoinerStartMaterialError> {
    SpaceAdmissionRoute::from_bytes(encoded_route.to_vec())
        .map_err(JoinerStartMaterialError::invalid_invitation_from)
}

/// 生成有效的随机准入标识。
fn mint_admission_id() -> SpaceAdmissionId {
    loop {
        let mut bytes = [0u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        if let Some(id) = SpaceAdmissionId::from_bytes(bytes) {
            return id;
        }
    }
}

/// 生成有效的随机加入标识。
fn mint_join_id() -> JoinId {
    loop {
        let mut bytes = [0u8; 16];
        rand::rng().fill_bytes(&mut bytes);
        if let Some(id) = JoinId::from_bytes(bytes) {
            return id;
        }
    }
}

/// 生成有效的随机消息标识。
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

    use async_trait::async_trait;
    use uc_application::deps::JoinerTransportMaterial;
    use uc_core::crypto::domain::Passphrase;
    use uc_core::pairing::InvitationCode;
    use uc_core::security::IdentityFingerprint;
    use uc_core::settings::model::Settings;

    use super::*;
    use uc_sync_protocol::full_invitation::encode_full_invitation;

    /// 验证完整邀请生成完整准入材料。
    #[tokio::test]
    async fn complete_joiner_start_material_is_created_from_a_full_invitation() {
        let invitation_id = uc_core::membership::InvitationId::from_bytes([0x61; 32])
            .expect("valid invitation id fixture");
        let encoded_route = b"opaque-sponsor-route";
        let invitation = encode_full_invitation(invitation_id, encoded_route, 1_900_000_000_000)
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
        let preserved = preserve_admission_route(encoded_route).expect("preserve admission route");
        assert_eq!(preserved.as_bytes(), encoded_route);
    }

    /// 验证非法邀请保持原有错误分类与来源。
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

        // 无效输入归为 InvalidInvitation，来源只是解码错误本身，而不是依赖故障。
        let JoinerStartMaterialError::InvalidInvitation {
            source: Some(source),
        } = &error
        else {
            panic!("expected InvalidInvitation with its decode source, got {error:?}");
        };
        assert!(source
            .downcast_ref::<uc_sync_protocol::full_invitation::FullInvitationCodecError>()
            .is_some());
    }

    /// 为已有材料测试提供确定性的传输材料。
    fn adapter() -> DefaultJoinerStartMaterial {
        let mut settings = Settings::default();
        settings.general.device_name = Some("Joining device".to_owned());
        DefaultJoinerStartMaterial::new(
            DeviceId::new("joining-device"),
            Arc::new(FixedSettings(settings)),
            Arc::new(FixedTransportMaterial),
        )
    }

    struct FixedTransportMaterial;

    impl JoinerTransportMaterialPort for FixedTransportMaterial {
        /// 提供已有 fixture 使用的固定传输身份与地址。
        fn prepare(&self) -> Result<JoinerTransportMaterial, JoinerStartMaterialError> {
            Ok(JoinerTransportMaterial {
                identity_fingerprint: IdentityFingerprint::from_display_string(
                    "ABCD-EFGH-IJKL-MNOP",
                )
                .expect("valid fingerprint"),
                transport_public_key: vec![0x11; 32],
                transport_address_blob: vec![0x12; 32],
            })
        }
    }

    struct FixedSettings(Settings);

    #[async_trait]
    impl SettingsPort for FixedSettings {
        async fn load(&self) -> anyhow::Result<Settings> {
            Ok(self.0.clone())
        }

        async fn save(&self, _settings: &Settings) -> anyhow::Result<()> {
            Ok(())
        }
    }
}
