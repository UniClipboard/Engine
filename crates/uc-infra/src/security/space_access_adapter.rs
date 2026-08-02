//! 空间访问的基础设施适配器。
//!
//! Slice 3 - C8 起完全独立运行: 不再依赖任何已删除的 port trait
//! (EncryptionPort / EncryptionSessionPort / KeyMaterialPort),
//! 改用 uc-infra 内部具体类型 `KeyMaterialStore` + `InMemorySession`,
//! AEAD 算法走 `super::v1_aead` helper。
//!
//! 该 adapter 实现内层聚合 trait `SpaceAccessStore`,并把每个窄意图 port
//! (`InitializeSpacePort` / `UnlockSpacePort` / … )经 UFCS 委托给它
//! (ports.md §8.3);全部方法签名保持稳定。字节级行为与历史
//! `EncryptionRepository` 一致——V1 加密协议 (Argon2id KDF +
//! XChaCha20-Poly1305 wrap/unwrap) ironclad 保留。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use hkdf::Hkdf;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use tracing::{debug, error, info, info_span, warn, Instrument};

use uc_core::crypto::domain::{ActiveSpace, Passphrase as DomainPassphrase};
use uc_core::crypto::model::{EncryptionError, Passphrase as LegacyPassphrase};

use super::crypto_model::{EncryptedBlob, KeyScope, KeySlot, WrappedMasterKey};
use super::secrets::{Kek, MasterKey};
use uc_core::ids::{DeviceId, ProfileId, SessionId, SpaceId};
use uc_core::membership::{
    AdmissionReplayId, BeginRevocationOutcome, BootstrapError, BootstrapId,
    CurrentMemberSignatureError, CurrentMemberSignaturePort, GroupBootstrapPort,
    GroupBootstrapResult, GroupEpoch, GroupRevocationPort, GroupRevocationResult, KeyEpochError,
    LegacyBootstrapProgress, LegacyBootstrapRecord, LegacyBootstrapRepositoryPort,
    LegacyBootstrapStage, LegacyBootstrapStatus, MemberProtection, MemberProtectionStatus,
    PendingGroupUpdate, ProtectionGroupAdmission, ProtectionGroupId, RevocationId,
    RevocationOutboxMessage, RevocationRecord, RevocationRepositoryPort, RevocationStage,
    RevocationStatus, SpaceKeyMaterial, SpaceKeyState, SpaceProtectionError, SpaceProtectionMode,
    SpaceProtectionSnapshot, SpaceProtectionStatusPort, SpaceSecurityMode,
};
use uc_core::pairing::InvitationCode;
use uc_core::ports::security::current_profile::CurrentProfilePort;
use uc_core::ports::space::{SpaceAccessError, SpaceAccessStore};
use uc_core::space_access::{
    AdmissionOffer, GroupAdmission, JoinOffer, PreparedAdmissionOffer, PreparedGroupJoin,
    ProofDerivedKey,
};

use super::key_material::KeyMaterialStore;
use super::mls_group::{MlsClientState, MlsGroupEngine, PendingMlsJoin};
use super::scope_identifier::scope_identifier;
use super::session::InMemorySession;
use super::v1_aead;

const MAX_STALLED_REVOCATION_ITERATIONS: usize = 3;

/// `SpaceAccessStore` 默认实现(同时提供全部窄意图 port)。
pub struct DefaultSpaceAccessAdapter {
    key_material: Arc<KeyMaterialStore>,
    current_profile: Arc<dyn CurrentProfilePort>,
    pub(super) session: Arc<InMemorySession>,
    pub(super) key_epoch_repository: Option<Arc<dyn RevocationRepositoryPort>>,
    legacy_bootstrap_repository: Option<Arc<dyn LegacyBootstrapRepositoryPort>>,
    /// 本进程内是否已经确认 keychain 中存在与本机 keyslot 匹配的 KEK。
    ///
    /// 一旦置位（`do_first_time_init` / `try_resume_session` /
    /// `derive_master_key_for_proof` 成功，或 `unlock` 完成首次刷新写入后），
    /// 后续的 `verify_keychain_access` 直接返回 `Ok(true)`，`unlock` 路径上
    /// 的"刷新写入"也跳过——避免在 macOS 上重复触发 keychain 授权弹窗
    /// （首次使用场景下原本会因 `try_resume_session` →
    /// `verify_keychain_access` → `unlock.store_kek refresh` 三次独立访问
    /// 而连弹三次）。
    ///
    /// `factory_reset` 删除 KEK 后必须复位为 `false`。
    kek_observed: AtomicBool,
}

impl DefaultSpaceAccessAdapter {
    pub fn new(
        key_material: Arc<KeyMaterialStore>,
        current_profile: Arc<dyn CurrentProfilePort>,
        session: Arc<InMemorySession>,
    ) -> Self {
        Self {
            key_material,
            current_profile,
            session,
            key_epoch_repository: None,
            legacy_bootstrap_repository: None,
            kek_observed: AtomicBool::new(false),
        }
    }

    pub fn new_with_key_epoch_repository(
        key_material: Arc<KeyMaterialStore>,
        current_profile: Arc<dyn CurrentProfilePort>,
        session: Arc<InMemorySession>,
        key_epoch_repository: Arc<dyn RevocationRepositoryPort>,
    ) -> Self {
        Self {
            key_material,
            current_profile,
            session,
            key_epoch_repository: Some(key_epoch_repository),
            legacy_bootstrap_repository: None,
            kek_observed: AtomicBool::new(false),
        }
    }

    pub fn new_with_security_repositories(
        key_material: Arc<KeyMaterialStore>,
        current_profile: Arc<dyn CurrentProfilePort>,
        session: Arc<InMemorySession>,
        key_epoch_repository: Arc<dyn RevocationRepositoryPort>,
        legacy_bootstrap_repository: Arc<dyn LegacyBootstrapRepositoryPort>,
    ) -> Self {
        Self {
            key_material,
            current_profile,
            session,
            key_epoch_repository: Some(key_epoch_repository),
            legacy_bootstrap_repository: Some(legacy_bootstrap_repository),
            kek_observed: AtomicBool::new(false),
        }
    }
}

/// Helper: 把端口返回的 `ProfileId` 包装成 key_material 使用的 `KeyScope`。
///
/// Slice 7 (U7) 过渡期间 `KeyScope` 仍是 uc-core 类型(磁盘 `KeySlotFile.scope`
/// 字段依赖);Slice 7 Commit 2 搬到 uc-infra 后这个 helper 可简化或消失。
fn key_scope_from_profile(profile: &ProfileId) -> KeyScope {
    KeyScope {
        profile_id: profile.as_ref().to_string(),
    }
}

fn map_encryption_error(err: EncryptionError) -> SpaceAccessError {
    match err {
        EncryptionError::WrongPassphrase => SpaceAccessError::WrongPassphrase,
        EncryptionError::CorruptedKeySlot
        | EncryptionError::CorruptedBlob
        | EncryptionError::UnsupportedKeySlotVersion
        | EncryptionError::UnsupportedBlobVersion => SpaceAccessError::CorruptedKeyMaterial,
        other => SpaceAccessError::Internal(other.to_string()),
    }
}

fn map_aead_error_for_unwrap(err: v1_aead::AeadError) -> SpaceAccessError {
    match err {
        v1_aead::AeadError::DecryptFailed => SpaceAccessError::WrongPassphrase,
        other => SpaceAccessError::Internal(other.to_string()),
    }
}

/// 把 master-key unwrap 阶段的 AEAD 失败按"业务输入错 vs 系统级故障"分级
/// 落 tracing event,再走 `map_aead_error_for_unwrap` 翻译到上层错误。
///
/// 三条调用路径语义统一:
/// - `unlock`: KEK 由当前 passphrase 现派生,unwrap 失败 ⇒ 用户输错口令。
/// - `try_resume_session`: KEK 直接读 keyring,unwrap 失败 ⇒ keyring 中
///   KEK 与磁盘 keyslot 漂移(典型场景:在另一台设备上改了口令,本机
///   keyring 没同步刷新)。仍是用户可恢复的输入路径——`load_kek` 拿到
///   的 KEK 不是密码学库故障,只是不对的字节。
/// - `derive_master_key_for_proof`: joiner 用本端 passphrase 派生 KEK 解
///   sponsor 包来的 wrapped master key,unwrap 失败 ⇒ 双方口令不一致。
///
/// 三种场景的共同点是 `AeadError::DecryptFailed` 永远代表"业务输入侧
/// 失败,UI 引导用户重输",不应作为 `error!` 级告警污染 Sentry 面板。
/// 其余变体 (`InvalidKey` / `EncryptFailed` / 长度异常等) 才是密码学库
/// 不该发生的故障,保留 `error!` 让 Sentry 抓到。
fn map_and_log_unwrap_aead_error(err: v1_aead::AeadError, path: &'static str) -> SpaceAccessError {
    match &err {
        v1_aead::AeadError::DecryptFailed => {
            warn!(
                path,
                "unwrap_master_key rejected: KEK does not match wrapped master key (passphrase mismatch or keyring/keyslot drift)"
            );
        }
        other => {
            error!(
                path,
                error = ?other,
                "unwrap_master_key failed: unexpected AEAD failure"
            );
        }
    }
    map_aead_error_for_unwrap(err)
}

/// KDF (Argon2id) 失败属于密码学库底层故障——参数已由 keyslot 固定,
/// 输入 passphrase 字节合法,这一步不该失败。走 `error!` + `Internal`。
fn map_and_log_kdf_error(err: String, path: &'static str) -> SpaceAccessError {
    error!(path, error = %err, "derive_kek_argon2id failed: unexpected KDF failure");
    SpaceAccessError::Internal(err)
}

/// `wrap_master_key_xchacha` / `MasterKey::generate` 等"本地新建密钥物料"
/// 路径上的失败同样属于密码学库底层故障。走 `error!` + `Internal`。
fn map_and_log_local_crypto_error(
    err: String,
    path: &'static str,
    op: &'static str,
) -> SpaceAccessError {
    error!(path, op, error = %err, "local crypto operation failed");
    SpaceAccessError::Internal(err)
}

#[derive(Debug, Serialize, Deserialize)]
struct AdmissionKdfOffer {
    version: String,
    kdf: super::crypto_model::KdfParams,
    salt: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
struct PortableKeyCatalog {
    version: u8,
    state: SpaceKeyState,
    key_catalog: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
struct GroupEpochUpdate {
    version: u8,
    group_epoch: u64,
    commit: Vec<u8>,
    encrypted_key_catalog: Vec<u8>,
}

fn group_catalog_aad(space_id: &SpaceId, epoch: u64) -> Vec<u8> {
    format!(
        "uniclipboard-group-key-catalog/v1|{}|{}",
        space_id.as_ref(),
        epoch
    )
    .into_bytes()
}

fn map_group_aead_error(error: v1_aead::AeadError) -> EncryptionError {
    match error {
        v1_aead::AeadError::DecryptFailed => EncryptionError::KeyMaterialCorrupt,
        v1_aead::AeadError::InvalidKey | v1_aead::AeadError::EncryptFailed => {
            EncryptionError::CryptoFailure
        }
    }
}

fn seal_group_catalog(
    wrapping_key: &MasterKey,
    material: &SpaceKeyMaterial,
) -> Result<Vec<u8>, EncryptionError> {
    let portable = PortableKeyCatalog {
        version: 1,
        state: material.state().clone(),
        key_catalog: material.key_catalog().to_vec(),
    };
    let plaintext =
        serde_json::to_vec(&portable).map_err(|_| EncryptionError::KeyMaterialCorrupt)?;
    let encrypted = v1_aead::encrypt_blob_xchacha(
        wrapping_key,
        &plaintext,
        &group_catalog_aad(
            material.state().space_id(),
            material.state().epoch().value(),
        ),
    )
    .map_err(map_group_aead_error)?;
    serde_json::to_vec(&encrypted).map_err(|_| EncryptionError::KeyMaterialCorrupt)
}

fn open_group_catalog(
    wrapping_key: &MasterKey,
    space_id: &SpaceId,
    epoch: u64,
    ciphertext: &[u8],
) -> Result<PortableKeyCatalog, EncryptionError> {
    let encrypted: EncryptedBlob =
        serde_json::from_slice(ciphertext).map_err(|_| EncryptionError::KeyMaterialCorrupt)?;
    let plaintext = v1_aead::decrypt_blob_xchacha(
        wrapping_key,
        &encrypted.nonce,
        &encrypted.ciphertext,
        &group_catalog_aad(space_id, epoch),
    )
    .map_err(map_group_aead_error)?;
    let portable: PortableKeyCatalog =
        serde_json::from_slice(&plaintext).map_err(|_| EncryptionError::KeyMaterialCorrupt)?;
    if portable.version != 1
        || portable.state.space_id() != space_id
        || portable.state.epoch() != GroupEpoch::new(epoch)
    {
        return Err(EncryptionError::KeyMaterialCorrupt);
    }
    Ok(portable)
}

fn serialize_admission_kdf_offer(keyslot: &KeySlot) -> Result<Vec<u8>, EncryptionError> {
    serde_json::to_vec(&AdmissionKdfOffer {
        version: "V1".to_string(),
        kdf: keyslot.kdf.clone(),
        salt: keyslot.salt.clone(),
    })
    .map_err(|_| EncryptionError::KeyMaterialCorrupt)
}

fn derive_admission_proof_key(
    kek: &Kek,
    invitation: &InvitationCode,
    session: &SessionId,
    space_id: &SpaceId,
) -> Result<ProofDerivedKey, EncryptionError> {
    let hkdf = Hkdf::<Sha256>::new(Some(invitation.as_str().as_bytes()), kek.as_bytes());
    let mut info = Vec::with_capacity(session.as_str().len() + space_id.as_ref().len() + 48);
    info.extend_from_slice(b"uniclipboard-admission-proof/v1");
    info.extend_from_slice(&(session.as_str().len() as u32).to_be_bytes());
    info.extend_from_slice(session.as_str().as_bytes());
    info.extend_from_slice(&(space_id.as_ref().len() as u32).to_be_bytes());
    info.extend_from_slice(space_id.as_ref().as_bytes());
    let mut output = [0u8; 32];
    hkdf.expand(&info, &mut output)
        .map_err(|_| EncryptionError::CryptoFailure)?;
    Ok(ProofDerivedKey::from_bytes(output))
}

impl DefaultSpaceAccessAdapter {
    async fn prepare_admission_offer(
        &self,
        space_id: &SpaceId,
        invitation: &InvitationCode,
        pairing_session_id: &SessionId,
    ) -> Result<PreparedAdmissionOffer, SpaceAccessError> {
        if self.session.current_space_id().ok().as_ref() != Some(space_id) {
            return Err(SpaceAccessError::NotUnlocked);
        }
        let profile = self
            .current_profile
            .current_profile()
            .await
            .map_err(|error| SpaceAccessError::Internal(error.to_string()))?;
        let scope = key_scope_from_profile(&profile);
        let keyslot = self
            .key_material
            .load_keyslot(&scope)
            .await
            .map_err(map_encryption_error)?;
        let kek = self
            .key_material
            .load_kek(&scope)
            .await
            .map_err(map_encryption_error)?;
        let kdf_parameters_blob =
            serialize_admission_kdf_offer(&keyslot).map_err(map_encryption_error)?;
        let mut challenge_nonce = [0u8; 32];
        rand::rng().fill_bytes(&mut challenge_nonce);
        let verification_key =
            derive_admission_proof_key(&kek, invitation, pairing_session_id, space_id)
                .map_err(map_encryption_error)?;

        Ok(PreparedAdmissionOffer {
            offer: AdmissionOffer {
                space_id: space_id.clone(),
                kdf_parameters_blob,
                challenge_nonce,
            },
            verification_key,
        })
    }

    async fn derive_admission_proof_key(
        &self,
        offer: &AdmissionOffer,
        passphrase: &DomainPassphrase,
        invitation: &InvitationCode,
        pairing_session_id: &SessionId,
    ) -> Result<ProofDerivedKey, SpaceAccessError> {
        let parameters: AdmissionKdfOffer = serde_json::from_slice(&offer.kdf_parameters_blob)
            .map_err(|_| SpaceAccessError::CorruptedKeyMaterial)?;
        if parameters.version != "V1" {
            return Err(SpaceAccessError::CorruptedKeyMaterial);
        }
        let legacy = LegacyPassphrase(passphrase.expose().to_string());
        let kek = v1_aead::derive_kek_argon2id(&legacy, &parameters.salt, &parameters.kdf)
            .map_err(|error| map_and_log_kdf_error(error, "derive_admission_proof_key"))?;
        derive_admission_proof_key(&kek, invitation, pairing_session_id, &offer.space_id)
            .map_err(map_encryption_error)
    }

    pub(super) async fn prepare_group_join(
        &self,
        device_id: &DeviceId,
    ) -> Result<PreparedGroupJoin, SpaceAccessError> {
        let pending = MlsGroupEngine::prepare_join(device_id.as_str().as_bytes())
            .map_err(|error| SpaceAccessError::Internal(error.to_string()))?;
        Ok(PreparedGroupJoin::new(
            pending.key_package,
            pending.client_state.into_bytes(),
        ))
    }

    async fn acknowledge_bootstrap_readmission_after_admission(
        &self,
        space_id: &SpaceId,
        joiner_device_id: &DeviceId,
        now_ms: i64,
    ) {
        let Some(repository) = &self.legacy_bootstrap_repository else {
            return;
        };
        let records = match repository.list_incomplete_legacy_bootstraps().await {
            Ok(records) => records,
            Err(error) => {
                warn!(error = %error, "legacy bootstrap readmission lookup failed after group admission");
                return;
            }
        };
        for record in records {
            if record.space_id() != space_id
                || record.status() != LegacyBootstrapStatus::AwaitingReadmission
                || !record
                    .pending_readmission()
                    .iter()
                    .any(|member| member == joiner_device_id)
            {
                continue;
            }
            if let Err(error) = repository
                .acknowledge_legacy_readmission(record.bootstrap_id(), joiner_device_id, now_ms)
                .await
            {
                warn!(
                    error = %error,
                    bootstrap_id = %record.bootstrap_id().as_str(),
                    "legacy bootstrap readmission acknowledgement will be retried during recovery"
                );
            }
        }
    }

    pub(super) async fn admit_group_member_with_replay(
        &self,
        space_id: &SpaceId,
        sponsor_device_id: &DeviceId,
        joiner_device_id: &DeviceId,
        existing_member_ids: &[DeviceId],
        key_package: &[u8],
        admission_replay: Option<(DeviceId, AdmissionReplayId)>,
    ) -> Result<(GroupAdmission, Option<ProtectionGroupAdmission>), SpaceAccessError> {
        let repository = self
            .key_epoch_repository
            .as_ref()
            .ok_or_else(|| SpaceAccessError::Internal("key epoch repository unavailable".into()))?;
        let current = match repository
            .load_space_material(space_id)
            .await
            .map_err(|error| SpaceAccessError::Internal(error.to_string()))?
        {
            Some(current) => current,
            None if existing_member_ids.is_empty() => {
                let sponsor_state = MlsGroupEngine::create_sponsor(
                    space_id.as_ref().as_bytes(),
                    sponsor_device_id.as_str().as_bytes(),
                )
                .map_err(|error| SpaceAccessError::Internal(error.to_string()))?;
                self.session
                    .create_legacy_bootstrap_material(
                        space_id,
                        sponsor_state.into_bytes(),
                        chrono::Utc::now().timestamp_millis(),
                    )
                    .map_err(map_encryption_error)?
            }
            None => return Err(SpaceAccessError::CorruptedKeyMaterial),
        };
        if current.group_state().is_empty() {
            return Err(SpaceAccessError::CorruptedKeyMaterial);
        }
        let sponsor_state = MlsClientState::from_bytes(current.group_state().to_vec());
        let admission = MlsGroupEngine::admit_member(
            &sponsor_state,
            joiner_device_id.as_str().as_bytes(),
            key_package,
        )
        .map_err(|error| SpaceAccessError::Internal(error.to_string()))?;
        let now_ms = chrono::Utc::now().timestamp_millis();
        let epoch = GroupEpoch::new(admission.epoch);
        let mut next = self
            .session
            .rotate_space_material(
                &current,
                admission.sponsor_state.into_bytes(),
                epoch,
                now_ms,
            )
            .map_err(map_encryption_error)?;
        let encrypted_key_catalog =
            seal_group_catalog(&admission.wrapping_key, &next).map_err(map_encryption_error)?;
        let existing_member_update = serde_json::to_vec(&GroupEpochUpdate {
            version: 1,
            group_epoch: admission.epoch,
            commit: admission.commit,
            encrypted_key_catalog: encrypted_key_catalog.clone(),
        })
        .map_err(|_| SpaceAccessError::CorruptedKeyMaterial)?;
        let existing_member_updates = existing_member_ids
            .iter()
            .cloned()
            .map(|recipient| {
                PendingGroupUpdate::persistent(recipient, existing_member_update.clone())
            })
            .collect::<Vec<_>>();
        next.add_pending_group_updates(existing_member_updates.iter().cloned(), now_ms);
        let group_admission = GroupAdmission {
            welcome: admission.welcome,
            encrypted_key_catalog,
            existing_member_updates,
            group_epoch: admission.epoch,
        };
        let replay_admission = if let Some((receiver, replay_id)) = admission_replay {
            let protection_group_id = next
                .state()
                .protection_group_id()
                .cloned()
                .ok_or(SpaceAccessError::CorruptedKeyMaterial)?;
            let cached = ProtectionGroupAdmission {
                protection_group_id: protection_group_id.clone(),
                admission: group_admission.clone(),
            };
            next.cache_group_admission(receiver, replay_id, cached.clone(), now_ms);
            Some(cached)
        } else {
            None
        };

        // Validate the complete material before making the durable generation
        // visible. The real session install after the write is then infallible
        // for the same inputs.
        let validator = InMemorySession::new();
        validator.set_master_key_for_space(
            space_id.clone(),
            self.session
                .get_master_key()
                .map_err(map_encryption_error)?,
        );
        validator
            .install_space_material(&next)
            .map_err(map_encryption_error)?;
        repository
            .save_space_material(&next)
            .await
            .map_err(|error| SpaceAccessError::Internal(error.to_string()))?;
        self.session
            .install_space_material(&next)
            .map_err(map_encryption_error)?;
        self.acknowledge_bootstrap_readmission_after_admission(space_id, joiner_device_id, now_ms)
            .await;

        Ok((group_admission, replay_admission))
    }

    async fn admit_group_member(
        &self,
        space_id: &SpaceId,
        sponsor_device_id: &DeviceId,
        joiner_device_id: &DeviceId,
        existing_member_ids: &[DeviceId],
        key_package: &[u8],
    ) -> Result<GroupAdmission, SpaceAccessError> {
        self.admit_group_member_with_replay(
            space_id,
            sponsor_device_id,
            joiner_device_id,
            existing_member_ids,
            key_package,
            None,
        )
        .await
        .map(|(admission, _)| admission)
    }

    async fn install_group_join(
        &self,
        space_id: &SpaceId,
        passphrase: &DomainPassphrase,
        pending: PreparedGroupJoin,
        welcome: &[u8],
        encrypted_key_catalog: &[u8],
        group_epoch: u64,
    ) -> Result<(), SpaceAccessError> {
        let repository = self
            .key_epoch_repository
            .as_ref()
            .ok_or_else(|| SpaceAccessError::Internal("key epoch repository unavailable".into()))?;
        let (key_package, private_state) = pending.into_parts();
        let completed = MlsGroupEngine::complete_join(
            PendingMlsJoin {
                key_package,
                client_state: MlsClientState::from_bytes(private_state),
            },
            space_id.as_ref().as_bytes(),
            welcome,
        )
        .map_err(|error| SpaceAccessError::Internal(error.to_string()))?;
        if completed.epoch != group_epoch {
            return Err(SpaceAccessError::CorruptedKeyMaterial);
        }
        let portable = open_group_catalog(
            &completed.wrapping_key,
            space_id,
            group_epoch,
            encrypted_key_catalog,
        )
        .map_err(map_encryption_error)?;
        let material = SpaceKeyMaterial::new(
            portable.state,
            completed.client_state.into_bytes(),
            portable.key_catalog,
            chrono::Utc::now().timestamp_millis(),
        );

        let profile = self
            .current_profile
            .current_profile()
            .await
            .map_err(|error| SpaceAccessError::Internal(error.to_string()))?;
        let scope = key_scope_from_profile(&profile);
        let previous = if self
            .key_material
            .keyslot_exists()
            .await
            .map_err(map_encryption_error)?
        {
            Some((
                self.key_material
                    .load_keyslot(&scope)
                    .await
                    .map_err(map_encryption_error)?,
                self.key_material
                    .load_kek(&scope)
                    .await
                    .map_err(map_encryption_error)?,
            ))
        } else {
            None
        };
        let previous_session = self.session.snapshot();

        let keyslot_draft = KeySlot::draft_v1(scope.clone()).map_err(map_encryption_error)?;
        let legacy = LegacyPassphrase(passphrase.expose().to_string());
        let kek = v1_aead::derive_kek_argon2id(&legacy, &keyslot_draft.salt, &keyslot_draft.kdf)
            .map_err(|error| map_and_log_kdf_error(error, "install_group_join"))?;
        let local_root = MasterKey::generate().map_err(map_encryption_error)?;
        let wrapped = v1_aead::wrap_master_key_xchacha(&kek, &local_root)
            .map_err(|error| SpaceAccessError::Internal(error.to_string()))?;
        let keyslot = keyslot_draft.finalize(WrappedMasterKey { blob: wrapped });

        if let Err(error) = self.key_material.store_kek(&scope, &kek).await {
            return Err(map_encryption_error(error));
        }
        if let Err(error) = self.key_material.store_keyslot(&keyslot).await {
            self.restore_join_install(&scope, previous, previous_session)
                .await;
            return Err(map_encryption_error(error));
        }
        self.session
            .set_master_key_for_space(space_id.clone(), local_root);
        if let Err(error) = self.session.install_space_material(&material) {
            self.restore_join_install(&scope, previous, previous_session)
                .await;
            return Err(map_encryption_error(error));
        }
        if let Err(error) = repository.save_space_material(&material).await {
            self.restore_join_install(&scope, previous, previous_session)
                .await;
            return Err(SpaceAccessError::Internal(error.to_string()));
        }
        self.kek_observed.store(true, Ordering::Release);
        Ok(())
    }

    pub(super) fn complete_group_join_material(
        &self,
        space_id: &SpaceId,
        pending: PreparedGroupJoin,
        welcome: &[u8],
        encrypted_key_catalog: &[u8],
        group_epoch: u64,
    ) -> Result<SpaceKeyMaterial, SpaceAccessError> {
        let (key_package, private_state) = pending.into_parts();
        let completed = MlsGroupEngine::complete_join(
            PendingMlsJoin {
                key_package,
                client_state: MlsClientState::from_bytes(private_state),
            },
            space_id.as_ref().as_bytes(),
            welcome,
        )
        .map_err(|error| SpaceAccessError::Internal(error.to_string()))?;
        if completed.epoch != group_epoch {
            return Err(SpaceAccessError::CorruptedKeyMaterial);
        }
        let portable = open_group_catalog(
            &completed.wrapping_key,
            space_id,
            group_epoch,
            encrypted_key_catalog,
        )
        .map_err(map_encryption_error)?;
        Ok(SpaceKeyMaterial::new(
            portable.state,
            completed.client_state.into_bytes(),
            portable.key_catalog,
            chrono::Utc::now().timestamp_millis(),
        ))
    }

    async fn revoke_group_member(
        &self,
        target: &DeviceId,
        retained_recipients: &[DeviceId],
        now_ms: i64,
    ) -> Result<GroupRevocationResult, KeyEpochError> {
        let repository = self
            .key_epoch_repository
            .as_ref()
            .ok_or_else(|| KeyEpochError::Repository("key epoch repository unavailable".into()))?;
        let space_id = self
            .session
            .current_space_id()
            .map_err(|error| KeyEpochError::Repository(error.to_string()))?;
        let Some(current) = repository.load_space_material(&space_id).await? else {
            return Ok(GroupRevocationResult::LocalOnly);
        };
        if current.state().mode() != SpaceSecurityMode::Ready {
            return Ok(GroupRevocationResult::LocalOnly);
        }
        if current.group_state().is_empty() {
            return Err(KeyEpochError::Repository(
                "ready space key material is corrupted".into(),
            ));
        }
        if retained_recipients
            .iter()
            .any(|recipient| recipient == target)
        {
            return Err(KeyEpochError::RemovedMemberInOutbox);
        }

        let prepared = RevocationRecord::prepare_with_recipients(
            RevocationId::generate(),
            space_id.clone(),
            target.clone(),
            retained_recipients.to_vec(),
            current.state().epoch(),
            now_ms,
        )?;
        let mut record = match repository.begin_revocation(&prepared).await? {
            BeginRevocationOutcome::Begun(record) | BeginRevocationOutcome::Existing(record) => {
                record
            }
        };

        let mut stalled_iterations = 0;
        loop {
            let previous_status = record.status();
            match record.status() {
                RevocationStatus::Prepared => {
                    let base = repository
                        .load_space_material(&space_id)
                        .await?
                        .ok_or_else(|| {
                            KeyEpochError::Repository("space key material unavailable".into())
                        })?;
                    if base.state().epoch() != record.previous_epoch() {
                        return Err(KeyEpochError::Repository(
                            "prepared revocation epoch mismatch".into(),
                        ));
                    }
                    let removal = MlsGroupEngine::remove_member(
                        &MlsClientState::from_bytes(base.group_state().to_vec()),
                        target.as_str().as_bytes(),
                    )
                    .map_err(|error| KeyEpochError::Repository(error.to_string()))?;
                    if GroupEpoch::new(removal.epoch) != record.next_epoch() {
                        return Err(KeyEpochError::Repository(
                            "MLS revocation epoch mismatch".into(),
                        ));
                    }
                    let next = self
                        .session
                        .rotate_space_material(
                            &base,
                            removal.sponsor_state.into_bytes(),
                            record.next_epoch(),
                            now_ms,
                        )
                        .map_err(|error| KeyEpochError::Repository(error.to_string()))?;
                    let encrypted_key_catalog = seal_group_catalog(&removal.wrapping_key, &next)
                        .map_err(|error| KeyEpochError::Repository(error.to_string()))?;
                    let update = serde_json::to_vec(&GroupEpochUpdate {
                        version: 1,
                        group_epoch: removal.epoch,
                        commit: removal.commit,
                        encrypted_key_catalog,
                    })
                    .map_err(|error| KeyEpochError::Repository(error.to_string()))?;
                    record.transition_to(RevocationStatus::Staged, now_ms)?;
                    let stage = RevocationStage::new(
                        record.clone(),
                        next.state().clone(),
                        next.group_state().to_vec(),
                        next.key_catalog().to_vec(),
                        record
                            .retained_recipients()
                            .iter()
                            .cloned()
                            .map(|recipient| {
                                RevocationOutboxMessage::new(recipient, update.clone())
                            })
                            .collect(),
                    )?;
                    let validator = InMemorySession::new();
                    validator.set_master_key_for_space(
                        space_id.clone(),
                        self.session
                            .get_master_key()
                            .map_err(|error| KeyEpochError::Repository(error.to_string()))?,
                    );
                    validator
                        .install_space_material(&next)
                        .map_err(|error| KeyEpochError::Repository(error.to_string()))?;
                    repository.stage_revocation(&stage).await?;
                }
                RevocationStatus::Staged => {
                    record = repository
                        .activate_revocation(record.revocation_id(), now_ms)
                        .await?;
                    let activated = repository
                        .load_space_material(&space_id)
                        .await?
                        .ok_or_else(|| {
                            KeyEpochError::Repository("activated key material unavailable".into())
                        })?;
                    self.session
                        .install_space_material(&activated)
                        .map_err(|error| KeyEpochError::Repository(error.to_string()))?;
                }
                RevocationStatus::Activated => {
                    let activated = repository
                        .load_space_material(&space_id)
                        .await?
                        .ok_or_else(|| {
                            KeyEpochError::Repository("activated key material unavailable".into())
                        })?;
                    self.session
                        .install_space_material(&activated)
                        .map_err(|error| KeyEpochError::Repository(error.to_string()))?;
                    record = repository
                        .start_distribution(record.revocation_id(), now_ms)
                        .await?;
                }
                RevocationStatus::Distributing | RevocationStatus::Complete => {
                    return Self::group_revocation_result(repository.as_ref(), &record).await;
                }
                RevocationStatus::RecoveryRequired => {
                    return Err(KeyEpochError::Repository(
                        "revocation requires recovery".into(),
                    ));
                }
            }
            record = repository
                .get_revocation(record.revocation_id())
                .await?
                .ok_or_else(|| KeyEpochError::Repository("revocation state disappeared".into()))?;
            if record.status() == previous_status {
                stalled_iterations += 1;
                if stalled_iterations >= MAX_STALLED_REVOCATION_ITERATIONS {
                    return Err(KeyEpochError::Repository(format!(
                        "revocation recovery required: repository state remained at {previous_status:?}"
                    )));
                }
            } else {
                stalled_iterations = 0;
            }
        }
    }

    async fn acknowledge_group_update(
        &self,
        revocation_id: &RevocationId,
        recipient: &DeviceId,
        now_ms: i64,
    ) -> Result<GroupRevocationResult, KeyEpochError> {
        let repository = self
            .key_epoch_repository
            .as_ref()
            .ok_or_else(|| KeyEpochError::Repository("key epoch repository unavailable".into()))?;
        let record = repository
            .acknowledge_recipient(revocation_id, recipient, now_ms)
            .await?;
        Self::group_revocation_result(repository.as_ref(), &record).await
    }

    async fn apply_group_epoch_update(&self, payload: &[u8]) -> Result<GroupEpoch, KeyEpochError> {
        let update: GroupEpochUpdate = serde_json::from_slice(payload)
            .map_err(|_| KeyEpochError::Repository("invalid group epoch update".into()))?;
        if update.version != 1 {
            return Err(KeyEpochError::Repository(
                "unsupported group epoch update".into(),
            ));
        }
        let repository = self
            .key_epoch_repository
            .as_ref()
            .ok_or_else(|| KeyEpochError::Repository("key epoch repository unavailable".into()))?;
        let space_id = self
            .session
            .current_space_id()
            .map_err(|error| KeyEpochError::Repository(error.to_string()))?;
        let current = repository
            .load_space_material(&space_id)
            .await?
            .ok_or_else(|| KeyEpochError::Repository("space key material unavailable".into()))?;
        let update_epoch = GroupEpoch::new(update.group_epoch);
        if current.state().epoch() == update_epoch {
            return Ok(update_epoch);
        }
        if current.state().epoch().next()? != update_epoch || current.group_state().is_empty() {
            return Err(KeyEpochError::Repository(
                "group epoch update is out of order".into(),
            ));
        }
        let completed = MlsGroupEngine::apply_commit(
            &MlsClientState::from_bytes(current.group_state().to_vec()),
            space_id.as_ref().as_bytes(),
            &update.commit,
        )
        .map_err(|error| KeyEpochError::Repository(error.to_string()))?;
        if GroupEpoch::new(completed.epoch) != update_epoch {
            return Err(KeyEpochError::Repository(
                "applied group epoch mismatch".into(),
            ));
        }
        let portable = open_group_catalog(
            &completed.wrapping_key,
            &space_id,
            update.group_epoch,
            &update.encrypted_key_catalog,
        )
        .map_err(|error| KeyEpochError::Repository(error.to_string()))?;
        let material = SpaceKeyMaterial::new(
            portable.state,
            completed.client_state.into_bytes(),
            portable.key_catalog,
            chrono::Utc::now().timestamp_millis(),
        )
        .with_pending_group_updates_from(&current);
        let validator = InMemorySession::new();
        validator.set_master_key_for_space(
            space_id,
            self.session
                .get_master_key()
                .map_err(|error| KeyEpochError::Repository(error.to_string()))?,
        );
        validator
            .install_space_material(&material)
            .map_err(|error| KeyEpochError::Repository(error.to_string()))?;
        repository.save_space_material(&material).await?;
        self.session
            .install_space_material(&material)
            .map_err(|error| KeyEpochError::Repository(error.to_string()))?;
        Ok(update_epoch)
    }

    async fn pending_group_updates(
        &self,
        revocation_id: &RevocationId,
    ) -> Result<Vec<PendingGroupUpdate>, KeyEpochError> {
        let repository = self
            .key_epoch_repository
            .as_ref()
            .ok_or_else(|| KeyEpochError::Repository("key epoch repository unavailable".into()))?;
        let Some(stage) = repository.load_staged_revocation(revocation_id).await? else {
            return Ok(Vec::new());
        };
        Ok(stage
            .outbox()
            .iter()
            .filter(|message| !message.is_confirmed())
            .map(|message| {
                PendingGroupUpdate::new(
                    revocation_id.clone(),
                    message.recipient().clone(),
                    message.payload().to_vec(),
                )
            })
            .collect())
    }

    async fn query_group_revocation(
        &self,
        revocation_id: &RevocationId,
    ) -> Result<Option<GroupRevocationResult>, KeyEpochError> {
        let repository = self
            .key_epoch_repository
            .as_ref()
            .ok_or_else(|| KeyEpochError::Repository("key epoch repository unavailable".into()))?;
        let Some(record) = repository.get_revocation(revocation_id).await? else {
            return Ok(None);
        };
        Self::group_revocation_result(repository.as_ref(), &record)
            .await
            .map(Some)
    }

    async fn resume_group_revocations(
        &self,
        now_ms: i64,
    ) -> Result<Vec<GroupRevocationResult>, KeyEpochError> {
        let repository = self
            .key_epoch_repository
            .as_ref()
            .ok_or_else(|| KeyEpochError::Repository("key epoch repository unavailable".into()))?;
        let records = repository.list_incomplete_revocations().await?;
        let mut results = Vec::with_capacity(records.len());
        for record in records {
            results.push(
                self.revoke_group_member(
                    record.target_device_id(),
                    record.retained_recipients(),
                    now_ms,
                )
                .await?,
            );
        }
        Ok(results)
    }

    async fn pending_space_group_updates(&self) -> Result<Vec<PendingGroupUpdate>, KeyEpochError> {
        let repository = self
            .key_epoch_repository
            .as_ref()
            .ok_or_else(|| KeyEpochError::Repository("key epoch repository unavailable".into()))?;
        let space_id = self
            .session
            .current_space_id()
            .map_err(|error| KeyEpochError::Repository(error.to_string()))?;
        Ok(repository
            .load_space_material(&space_id)
            .await?
            .map(|material| material.pending_group_updates().to_vec())
            .unwrap_or_default())
    }

    async fn acknowledge_space_group_update(
        &self,
        update_id: &str,
        now_ms: i64,
    ) -> Result<bool, KeyEpochError> {
        let repository = self
            .key_epoch_repository
            .as_ref()
            .ok_or_else(|| KeyEpochError::Repository("key epoch repository unavailable".into()))?;
        let space_id = self
            .session
            .current_space_id()
            .map_err(|error| KeyEpochError::Repository(error.to_string()))?;
        let Some(mut material) = repository.load_space_material(&space_id).await? else {
            return Ok(false);
        };
        if !material.acknowledge_group_update(update_id, now_ms) {
            return Ok(false);
        }
        repository.save_space_material(&material).await?;
        Ok(true)
    }

    async fn group_revocation_result(
        repository: &dyn RevocationRepositoryPort,
        record: &RevocationRecord,
    ) -> Result<GroupRevocationResult, KeyEpochError> {
        let pending_recipients = if record.status() == RevocationStatus::Distributing {
            repository
                .load_staged_revocation(record.revocation_id())
                .await?
                .ok_or_else(|| {
                    KeyEpochError::Repository("revocation distribution payload missing".into())
                })?
                .outbox()
                .iter()
                .filter(|message| !message.is_confirmed())
                .count()
        } else {
            0
        };
        Ok(GroupRevocationResult::Reliable {
            revocation_id: record.revocation_id().clone(),
            status: record.status(),
            pending_recipients,
        })
    }

    async fn restore_join_install(
        &self,
        scope: &KeyScope,
        previous: Option<(KeySlot, Kek)>,
        previous_session: super::session::SessionSnapshot,
    ) {
        self.session.restore(previous_session);
        match previous {
            Some((keyslot, kek)) => {
                if let Err(error) = self.key_material.store_kek(scope, &kek).await {
                    error!(error = %error, "failed to restore previous KEK after join failure");
                }
                if let Err(error) = self.key_material.store_keyslot(&keyslot).await {
                    error!(error = %error, "failed to restore previous keyslot after join failure");
                }
            }
            None => {
                if let Err(error) = self.key_material.delete_keyslot(scope).await {
                    warn!(error = %error, "failed to remove staged keyslot after join failure");
                }
                if let Err(error) = self.key_material.delete_kek(scope).await {
                    warn!(error = %error, "failed to remove staged KEK after join failure");
                }
            }
        }
    }

    async fn activate_session(
        &self,
        space_id: &SpaceId,
        master_key: MasterKey,
    ) -> Result<(), SpaceAccessError> {
        self.session
            .set_master_key_for_space(space_id.clone(), master_key);
        let Some(repository) = &self.key_epoch_repository else {
            return Ok(());
        };

        let material = match repository.load_space_material(space_id).await {
            Ok(Some(material)) => material,
            // A missing record is an existing Legacy space, not evidence that
            // a group and new content key catalog have been safely created.
            Ok(None) => return Ok(()),
            Err(error) => {
                self.session.clear();
                return Err(SpaceAccessError::Internal(error.to_string()));
            }
        };
        if let Err(error) = self.session.install_space_material(&material) {
            self.session.clear();
            return Err(map_encryption_error(error));
        }
        Ok(())
    }

    /// 私有 helper：执行首次初始化的核心步骤
    /// （生成 KeySlot 草稿 → 派生 KEK → 生成 MasterKey → 包装 → 落盘 →
    /// 写入会话 → 标记 Initialized）。任何中间步骤失败时按依赖反向回滚。
    async fn do_first_time_init(
        &self,
        space_id: &SpaceId,
        scope: &KeyScope,
        passphrase: &DomainPassphrase,
    ) -> Result<KeySlot, SpaceAccessError> {
        const PATH: &str = "first_time_init";

        let keyslot_draft = KeySlot::draft_v1(scope.clone())
            .map_err(|e| map_and_log_local_crypto_error(e.to_string(), PATH, "draft_keyslot_v1"))?;
        debug!("keyslot draft created");

        let legacy = LegacyPassphrase(passphrase.expose().to_string());
        let kek = v1_aead::derive_kek_argon2id(&legacy, &keyslot_draft.salt, &keyslot_draft.kdf)
            .map_err(|e| map_and_log_kdf_error(e, PATH))?;
        debug!("KEK derived");

        let master_key = MasterKey::generate().map_err(|e| {
            map_and_log_local_crypto_error(e.to_string(), PATH, "generate_master_key")
        })?;
        debug!("master key generated");

        let blob = v1_aead::wrap_master_key_xchacha(&kek, &master_key)
            .map_err(|e| map_and_log_local_crypto_error(e.to_string(), PATH, "wrap_master_key"))?;
        debug!("master key wrapped");

        let keyslot = keyslot_draft.finalize(WrappedMasterKey { blob });

        if let Err(e) = self.key_material.store_kek(scope, &kek).await {
            error!(path = PATH, error = %e, "store_kek failed");
            return Err(map_encryption_error(e));
        }
        self.kek_observed.store(true, Ordering::Release);

        if let Err(e) = self.key_material.store_keyslot(&keyslot).await {
            error!(path = PATH, error = %e, "store_keyslot failed, rolling back KEK");
            if let Err(err) = self.key_material.delete_keyslot(scope).await {
                warn!(path = PATH, error = %err, "rollback delete_keyslot failed");
            }
            if let Err(err) = self.key_material.delete_kek(scope).await {
                warn!(path = PATH, error = %err, "rollback delete_kek failed");
            }
            self.kek_observed.store(false, Ordering::Release);
            return Err(map_encryption_error(e));
        }

        // session 写入是 in-memory 操作,不会失败——直接写。
        // Phase C 起不再写 `.initialized_encryption` marker 文件;"已初始化"
        // 真相由磁盘 keyslot 存在性 (`key_material.keyslot_exists()`) 回答,
        // setup 完成事实由 `SetupStatusPort.has_completed` 承载。
        if let Err(error) = self.activate_session(space_id, master_key).await {
            self.session.clear();
            if let Err(rollback_error) = self.key_material.delete_keyslot(scope).await {
                warn!(path = PATH, error = %rollback_error, "rollback delete_keyslot failed");
            }
            if let Err(rollback_error) = self.key_material.delete_kek(scope).await {
                warn!(path = PATH, error = %rollback_error, "rollback delete_kek failed");
            }
            self.kek_observed.store(false, Ordering::Release);
            return Err(error);
        }

        Ok(keyslot)
    }
}

#[async_trait]
impl SpaceAccessStore for DefaultSpaceAccessAdapter {
    async fn initialize(
        &self,
        space_id: &SpaceId,
        passphrase: &DomainPassphrase,
    ) -> Result<ActiveSpace, SpaceAccessError> {
        const PATH: &str = "initialize";
        let span = info_span!("infra.space_access.initialize", space_id = %space_id);
        async {
            info!("initializing new space");

            if self.key_material.keyslot_exists().await.map_err(|e| {
                error!(path = PATH, error = %e, "keyslot_exists probe failed");
                SpaceAccessError::Internal(e.to_string())
            })? {
                info!(
                    path = PATH,
                    "initialize rejected: keyslot already exists on disk"
                );
                return Err(SpaceAccessError::AlreadyInitialized);
            }

            let profile = self.current_profile.current_profile().await.map_err(|e| {
                error!(path = PATH, error = %e, "current_profile resolution failed");
                SpaceAccessError::Internal(e.to_string())
            })?;
            let scope = key_scope_from_profile(&profile);
            debug!(path = PATH, scope = %scope_identifier(&scope), "got key scope");

            self.do_first_time_init(space_id, &scope, passphrase)
                .await?;

            info!("space initialized successfully");
            Ok(ActiveSpace::new(space_id.clone()))
        }
        .instrument(span)
        .await
    }

    async fn unlock(
        &self,
        space_id: &SpaceId,
        passphrase: &DomainPassphrase,
    ) -> Result<ActiveSpace, SpaceAccessError> {
        const PATH: &str = "unlock";
        let span = info_span!("infra.space_access.unlock", space_id = %space_id);
        async {
            info!("unlocking space with passphrase");

            if !self.key_material.keyslot_exists().await.map_err(|e| {
                error!(path = PATH, error = %e, "keyslot_exists probe failed");
                SpaceAccessError::Internal(e.to_string())
            })? {
                info!(
                    path = PATH,
                    "unlock rejected: no keyslot on disk (not initialized)"
                );
                return Err(SpaceAccessError::NotInitialized);
            }

            let profile = self.current_profile.current_profile().await.map_err(|e| {
                error!(path = PATH, error = %e, "current_profile resolution failed");
                SpaceAccessError::Internal(e.to_string())
            })?;
            let scope = key_scope_from_profile(&profile);
            debug!(path = PATH, scope = %scope_identifier(&scope), "got key scope");

            let keyslot = self.key_material.load_keyslot(&scope).await.map_err(|e| {
                warn!(path = PATH, error = %e, "load_keyslot failed");
                map_encryption_error(e)
            })?;

            let wrapped_master_key = keyslot.wrapped_master_key.as_ref().ok_or_else(|| {
                warn!(
                    path = PATH,
                    "keyslot on disk has no wrapped_master_key (corrupted key material)"
                );
                SpaceAccessError::CorruptedKeyMaterial
            })?;

            let legacy = LegacyPassphrase(passphrase.expose().to_string());
            let kek = v1_aead::derive_kek_argon2id(&legacy, &keyslot.salt, &keyslot.kdf)
                .map_err(|e| map_and_log_kdf_error(e, PATH))?;
            debug!(path = PATH, "KEK derived from passphrase");

            let master_key = v1_aead::unwrap_master_key_xchacha(&kek, &wrapped_master_key.blob)
                .map_err(|e| map_and_log_unwrap_aead_error(e, PATH))?;
            debug!(path = PATH, "master key unwrapped");

            // 把派生出的 KEK 重新写入 keyring,保持 keyring 与最新口令对齐
            // (让下次静默 startup 路径仍可命中)。失败仅 warn,不影响本次解锁。
            //
            // 优化:若本进程内已确认 keychain 中存在 KEK
            // (`try_resume_session` / `do_first_time_init` /
            // `derive_master_key_for_proof` 任一已置位 `kek_observed`),
            // 此处 `unwrap` 已经成功——意味着本次派生出的 KEK 字节就是
            // keychain 里那条记录的字节,再写一次没有信息增量,但在 macOS
            // 上每次 set_secret 仍可能触发授权弹窗。因此跳过。
            if self.kek_observed.load(Ordering::Acquire) {
                debug!("skip store_kek refresh: KEK already observed in keychain this session");
            } else if let Err(e) = self.key_material.store_kek(&scope, &kek).await {
                warn!(error = %e, "store_kek refresh failed (non-fatal)");
            } else {
                self.kek_observed.store(true, Ordering::Release);
            }

            self.activate_session(space_id, master_key).await?;

            info!("space unlocked successfully");
            Ok(ActiveSpace::new(space_id.clone()))
        }
        .instrument(span)
        .await
    }

    async fn is_unlocked(&self, _space_id: &SpaceId) -> bool {
        self.session.is_ready()
    }

    async fn lock(&self, _space_id: &SpaceId) -> Result<(), SpaceAccessError> {
        self.session.clear();
        Ok(())
    }

    async fn factory_reset(&self, space_id: &SpaceId) -> Result<(), SpaceAccessError> {
        const PATH: &str = "factory_reset";
        let span = info_span!("infra.space_access.factory_reset", space_id = %space_id);
        async {
            info!(path = PATH, "factory reset requested");

            let profile = self.current_profile.current_profile().await.map_err(|e| {
                error!(path = PATH, error = %e, "current_profile resolution failed");
                SpaceAccessError::Internal(e.to_string())
            })?;
            let scope = key_scope_from_profile(&profile);
            debug!(path = PATH, scope = %scope_identifier(&scope), "got key scope");

            // 幂等: 不存在的物料视为已经删除,不报错。
            match self.key_material.delete_keyslot(&scope).await {
                Ok(()) => debug!(path = PATH, "keyslot deleted"),
                Err(EncryptionError::KeyNotFound) => {
                    debug!(path = PATH, "keyslot already absent (idempotent)")
                }
                Err(e) => {
                    error!(path = PATH, error = %e, "delete_keyslot failed");
                    return Err(map_encryption_error(e));
                }
            }
            match self.key_material.delete_kek(&scope).await {
                Ok(()) => debug!(path = PATH, "KEK deleted from keyring"),
                Err(EncryptionError::KeyNotFound) => {
                    debug!(path = PATH, "KEK already absent in keyring (idempotent)")
                }
                Err(e) => {
                    error!(path = PATH, error = %e, "delete_kek failed");
                    return Err(map_encryption_error(e));
                }
            }
            self.kek_observed.store(false, Ordering::Release);
            self.session.clear();
            info!(path = PATH, "factory reset completed");
            Ok(())
        }
        .instrument(span)
        .await
    }

    async fn try_resume_session(
        &self,
        space_id: &SpaceId,
    ) -> Result<Option<ActiveSpace>, SpaceAccessError> {
        const PATH: &str = "try_resume_session";
        let span = info_span!("infra.space_access.try_resume_session", space_id = %space_id);
        async {
            info!("attempting silent session resume from keyring");

            // session 已经在内存中(典型场景:用户刚 `initialize` 完成,前端
            // setup 后的 onSetupComplete 回调又调了一次 `EncryptionFacade::unlock`
            // → 这里)。已经有 master_key,没必要再走 load_kek + unwrap +
            // set_master_key 这一整圈——尤其是 load_kek 在 macOS 上每次都可能
            // 触发 keychain 授权弹窗。直接返回 Ok(Some) 表达"会话已就绪"。
            if self.session.is_ready() {
                info!("session already in-memory, skip keychain probe");
                return Ok(Some(ActiveSpace::new(space_id.clone())));
            }

            if !self.key_material.keyslot_exists().await.map_err(|e| {
                error!(path = PATH, error = %e, "keyslot_exists probe failed");
                SpaceAccessError::Internal(e.to_string())
            })? {
                info!(path = PATH, "no keyslot on disk, no session to resume");
                return Ok(None);
            }

            let profile = self.current_profile.current_profile().await.map_err(|e| {
                error!(path = PATH, error = %e, "current_profile resolution failed");
                SpaceAccessError::Internal(e.to_string())
            })?;
            let scope = key_scope_from_profile(&profile);
            debug!(path = PATH, scope = %scope_identifier(&scope), "got key scope");

            let keyslot = self.key_material.load_keyslot(&scope).await.map_err(|e| {
                warn!(path = PATH, error = %e, "load_keyslot failed during resume");
                map_encryption_error(e)
            })?;
            let wrapped_master_key = keyslot.wrapped_master_key.as_ref().ok_or_else(|| {
                warn!(
                    path = PATH,
                    "keyslot on disk has no wrapped_master_key (corrupted key material)"
                );
                SpaceAccessError::CorruptedKeyMaterial
            })?;

            // 静默路径: 直接读 keyring 缓存的 KEK,不重新派生。
            // load_kek 失败通常意味着 keyring 中没有这条 KEK——首次启动 /
            // keyring 被清 / 跨设备 profile 迁移——属于业务正常路径,
            // 上层会回退到要求用户重新输入口令走 unlock。warn 级别即可。
            let kek = self.key_material.load_kek(&scope).await.map_err(|e| {
                info!(
                    path = PATH,
                    error = %e,
                    "load_kek from keyring failed; caller will fall back to passphrase unlock"
                );
                map_encryption_error(e)
            })?;

            let master_key = v1_aead::unwrap_master_key_xchacha(&kek, &wrapped_master_key.blob)
                .map_err(|e| map_and_log_unwrap_aead_error(e, PATH))?;

            // load_kek 成功 + unwrap 成功 ⇒ keychain 中 KEK 与本机 keyslot 匹配。
            // 标记本进程已观察到该 KEK,后续 verify_keychain_access /
            // unlock 路径无需再次访问 keychain。
            self.kek_observed.store(true, Ordering::Release);

            self.activate_session(space_id, master_key).await?;

            info!("session resumed from keyring");
            Ok(Some(ActiveSpace::new(space_id.clone())))
        }
        .instrument(span)
        .await
    }

    async fn verify_keychain_access(&self) -> Result<bool, SpaceAccessError> {
        const PATH: &str = "verify_keychain_access";
        let span = info_span!("infra.space_access.verify_keychain_access");
        async {
            // 缓存命中:本进程内已成功 load_kek / store_kek 过——keychain
            // 已经为本应用授予访问权限,无需再次探测。再次探测在 macOS 上
            // 等价于一次 set_secret/get_secret 系统调用,可能触发新一轮
            // 授权弹窗。
            if self.kek_observed.load(Ordering::Acquire) {
                debug!(path = PATH, "kek_observed cached, skip keychain probe");
                return Ok(true);
            }

            let profile = self
                .current_profile
                .current_profile()
                .await
                .map_err(|e| {
                    error!(path = PATH, error = %e, "current_profile resolution failed");
                    SpaceAccessError::Internal(e.to_string())
                })?;
            let scope = key_scope_from_profile(&profile);

            // 探测: 把"权限被拒绝"和"keyring 暂时不可用"都视为 "Always Allow 未授予"
            // (Ok(false));只有"KEK 不存在"才升格成 NotInitialized 报错给上层。
            match self.key_material.load_kek(&scope).await {
                Ok(_) => {
                    self.kek_observed.store(true, Ordering::Release);
                    debug!(path = PATH, "keychain access verified via load_kek probe");
                    Ok(true)
                }
                Err(EncryptionError::PermissionDenied) => {
                    info!(path = PATH, "keychain access denied (Always Allow not granted)");
                    Ok(false)
                }
                Err(EncryptionError::KeyringError(msg)) => {
                    warn!(path = PATH, error = %msg, "keyring transiently unavailable; treat as not-granted");
                    Ok(false)
                }
                Err(EncryptionError::KeyNotFound) => {
                    info!(path = PATH, "no KEK in keychain for current profile");
                    Err(SpaceAccessError::NotInitialized)
                }
                Err(other) => {
                    error!(path = PATH, error = %other, "unexpected load_kek failure during keychain probe");
                    Err(SpaceAccessError::Internal(other.to_string()))
                }
            }
        }
        .instrument(span)
        .await
    }

    async fn derive_subkey(&self, salt: &[u8], info: &[u8]) -> Result<[u8; 32], SpaceAccessError> {
        const PATH: &str = "derive_subkey";
        if !self.session.is_ready() {
            warn!(path = PATH, "derive_subkey called while session not ready");
            return Err(SpaceAccessError::NotUnlocked);
        }
        let okm = self.session.derive_stable_subkey(salt, info).map_err(|e| {
            error!(path = PATH, error = %e, "derive stable session subkey failed");
            map_encryption_error(e)
        })?;
        Ok(okm)
    }

    async fn current_session_proof_key(&self) -> Result<Option<ProofDerivedKey>, SpaceAccessError> {
        const PATH: &str = "current_session_proof_key";
        if !self.session.is_ready() {
            return Ok(None);
        }
        let master_key = self.session.get_master_key().map_err(|e| {
            error!(path = PATH, error = %e, "get_master_key from session failed");
            map_encryption_error(e)
        })?;
        Ok(Some(ProofDerivedKey::from_bytes(master_key.into_bytes())))
    }

    async fn prepare_join_offer(
        &self,
        space_id: &SpaceId,
        passphrase: &DomainPassphrase,
    ) -> Result<JoinOffer, SpaceAccessError> {
        const PATH: &str = "prepare_join_offer";
        let span = info_span!("infra.space_access.prepare_join_offer", space_id = %space_id);
        async {
            info!("preparing sponsor join offer");

            let already_initialized = self
                .key_material
                .keyslot_exists()
                .await
                .map_err(|e| {
                    error!(path = PATH, error = %e, "keyslot_exists probe failed");
                    SpaceAccessError::Internal(e.to_string())
                })?;
            debug!(path = PATH, already_initialized, "checked keyslot existence");

            let profile = self
                .current_profile
                .current_profile()
                .await
                .map_err(|e| {
                    error!(path = PATH, error = %e, "current_profile resolution failed");
                    SpaceAccessError::Internal(e.to_string())
                })?;
            let scope = key_scope_from_profile(&profile);
            debug!(path = PATH, scope = %scope_identifier(&scope), "got key scope");

            // Branch A — 运行时已初始化的 sponsor 路径: 从 key_material 读已有 keyslot,
            // 不重新生成 MasterKey。passphrase 参数此时不参与派生。
            if already_initialized {
                let _ = passphrase;
                let keyslot = self
                    .key_material
                    .load_keyslot(&scope)
                    .await
                    .map_err(|e| {
                        warn!(path = PATH, branch = "already_initialized", error = %e, "load_keyslot failed");
                        map_encryption_error(e)
                    })?;
                let keyslot_blob = serde_json::to_vec(&keyslot).map_err(|e| {
                    error!(
                        path = PATH,
                        branch = "already_initialized",
                        error = %e,
                        "serialize keyslot to wire blob failed"
                    );
                    SpaceAccessError::Internal(format!("serialize keyslot: {e}"))
                })?;
                let mut challenge_nonce = [0u8; 32];
                rand::rng().fill_bytes(&mut challenge_nonce);
                info!("sponsor join offer prepared (runtime, already initialized)");
                return Ok(JoinOffer {
                    space_id: space_id.clone(),
                    keyslot_blob,
                    challenge_nonce,
                });
            }

            // Branch B — 首次 setup sponsor 路径: 未初始化,走完整 KEK 派生 +
            // MasterKey 生成 + 包装 + 落盘 + 标记 Initialized。
            let keyslot = self
                .do_first_time_init(space_id, &scope, passphrase)
                .await?;
            let keyslot_blob = serde_json::to_vec(&keyslot).map_err(|e| {
                error!(
                    path = PATH,
                    branch = "first_time_init",
                    error = %e,
                    "serialize freshly-initialized keyslot to wire blob failed"
                );
                SpaceAccessError::Internal(format!("serialize keyslot: {e}"))
            })?;
            let mut challenge_nonce = [0u8; 32];
            rand::rng().fill_bytes(&mut challenge_nonce);

            info!("sponsor join offer prepared");
            Ok(JoinOffer {
                space_id: space_id.clone(),
                keyslot_blob,
                challenge_nonce,
            })
        }
        .instrument(span)
        .await
    }

    async fn derive_master_key_for_proof(
        &self,
        offer: &JoinOffer,
        passphrase: &DomainPassphrase,
    ) -> Result<ProofDerivedKey, SpaceAccessError> {
        const PATH: &str = "derive_master_key_for_proof";
        let span = info_span!("infra.space_access.derive_master_key_for_proof", space_id = %offer.space_id);
        async {
            info!("deriving master key from pairing offer");

            let keyslot: KeySlot = serde_json::from_slice(&offer.keyslot_blob).map_err(|e| {
                warn!(
                    path = PATH,
                    error = %e,
                    offer_blob_len = offer.keyslot_blob.len(),
                    "failed to deserialize keyslot from offer blob (corrupted wire data)"
                );
                SpaceAccessError::CorruptedKeyMaterial
            })?;
            let scope = keyslot.scope.clone();
            debug!(path = PATH, scope = %scope_identifier(&scope), "parsed keyslot from offer blob");

            let wrapped_master_key = keyslot.wrapped_master_key.as_ref().ok_or_else(|| {
                warn!(
                    path = PATH,
                    "offer keyslot has no wrapped_master_key (corrupted offer)"
                );
                SpaceAccessError::CorruptedKeyMaterial
            })?;

            let legacy = LegacyPassphrase(passphrase.expose().to_string());
            let kek = v1_aead::derive_kek_argon2id(&legacy, &keyslot.salt, &keyslot.kdf)
                .map_err(|e| map_and_log_kdf_error(e, PATH))?;
            debug!(path = PATH, "KEK derived from passphrase and offer keyslot");

            // 先 unwrap 验证 KEK + keyslot 真的匹配,再动本机持久状态。
            // 之前的顺序是 store_kek → store_keyslot → unwrap, unwrap 失败时
            // 走 delete_keyslot/delete_kek 回滚——但 store 是**覆盖式**写入,
            // 此时本机原有 KEK / keyslot 已被替换,删除回滚等于把"已 setup
            // 且能解锁"的设备打回未 setup 状态(switch_space/mod.rs 头注里
            // 担心的"derive_master_key_for_proof 已经覆写了那种情况下设备
            // 需要手动 factory_reset"就来源于此)。把 unwrap 抬到 store
            // 之前,失败时直接返回, 本机原状一字不动。
            let master_key = v1_aead::unwrap_master_key_xchacha(&kek, &wrapped_master_key.blob)
                .map_err(|e| map_and_log_unwrap_aead_error(e, PATH))?;
            debug!(path = PATH, "master key unwrapped");

            // unwrap 已确认 KEK + keyslot 匹配, 再覆盖本机磁盘 / keyring。
            // 此处仍有"store_keyslot 失败 → delete_kek 回滚把刚刚覆盖的本机
            // 原 KEK 一并删掉"的窄窗口(需要 keyring/磁盘真实 IO 失败),
            // 影响远小于 unwrap 失败这条常见路径, 留作后续单独修复。
            if let Err(e) = self.key_material.store_kek(&scope, &kek).await {
                error!(path = PATH, error = %e, "store_kek failed");
                return Err(map_encryption_error(e));
            }
            self.kek_observed.store(true, Ordering::Release);

            if let Err(e) = self.key_material.store_keyslot(&keyslot).await {
                error!(path = PATH, error = %e, "store_keyslot failed, rolling back KEK");
                if let Err(err) = self.key_material.delete_keyslot(&scope).await {
                    warn!(path = PATH, error = %err, "rollback delete_keyslot failed");
                }
                if let Err(err) = self.key_material.delete_kek(&scope).await {
                    warn!(path = PATH, error = %err, "rollback delete_kek failed");
                }
                self.kek_observed.store(false, Ordering::Release);
                return Err(map_encryption_error(e));
            }

            // 把字节注入会话(让 sponsor 后续 verify 走 fallback 路径),
            // 同时包装一份成不透明凭据返回 joiner 侧调用方。
            // Phase C 起不再写 `.initialized_encryption` marker 文件;
            // "本机已初始化" 的真相由磁盘 keyslot 文件存在性回答。
            self.activate_session(&offer.space_id, master_key.clone()).await?;
            let derived = ProofDerivedKey::from_bytes(master_key.into_bytes());

            info!("master key derivation completed");
            Ok(derived)
        }
        .instrument(span)
        .await
    }
}

// ---- Intent ports ----
//
// The single adapter satisfies every narrow space-access intent port by
// delegating to its aggregate-store methods (UFCS disambiguates the same-named
// methods). The composition root coerces one
// `Arc<DefaultSpaceAccessAdapter>` into each port (see ports.md §8.3).
//
// These impls live in a private submodule so the narrow port traits do not
// leak into other method-resolution scopes (they share method names with the
// aggregate store); trait-impl coherence still applies crate-wide.
mod intent_ports {
    use super::*;
    use uc_core::ports::space::{
        CurrentSessionProofKeyPort, DeriveAdmissionProofKeyPort, DeriveProofKeyPort,
        DeriveSpaceSubkeyPort, FactoryResetSpacePort, GroupAdmissionPort, InitializeSpacePort,
        IsSpaceUnlockedPort, LockSpacePort, PrepareAdmissionOfferPort, PrepareJoinOfferPort,
        ResumeSpaceSessionPort, UnlockSpacePort, VerifyKeychainAccessPort,
    };

    #[async_trait]
    impl InitializeSpacePort for DefaultSpaceAccessAdapter {
        async fn initialize(
            &self,
            space_id: &SpaceId,
            passphrase: &DomainPassphrase,
        ) -> Result<ActiveSpace, SpaceAccessError> {
            SpaceAccessStore::initialize(self, space_id, passphrase).await
        }
    }

    #[async_trait]
    impl UnlockSpacePort for DefaultSpaceAccessAdapter {
        async fn unlock(
            &self,
            space_id: &SpaceId,
            passphrase: &DomainPassphrase,
        ) -> Result<ActiveSpace, SpaceAccessError> {
            SpaceAccessStore::unlock(self, space_id, passphrase).await
        }
    }

    #[async_trait]
    impl IsSpaceUnlockedPort for DefaultSpaceAccessAdapter {
        async fn is_unlocked(&self, space_id: &SpaceId) -> bool {
            SpaceAccessStore::is_unlocked(self, space_id).await
        }
    }

    #[async_trait]
    impl LockSpacePort for DefaultSpaceAccessAdapter {
        async fn lock(&self, space_id: &SpaceId) -> Result<(), SpaceAccessError> {
            SpaceAccessStore::lock(self, space_id).await
        }
    }

    #[async_trait]
    impl FactoryResetSpacePort for DefaultSpaceAccessAdapter {
        async fn factory_reset(&self, space_id: &SpaceId) -> Result<(), SpaceAccessError> {
            SpaceAccessStore::factory_reset(self, space_id).await
        }
    }

    #[async_trait]
    impl ResumeSpaceSessionPort for DefaultSpaceAccessAdapter {
        async fn try_resume_session(
            &self,
            space_id: &SpaceId,
        ) -> Result<Option<ActiveSpace>, SpaceAccessError> {
            SpaceAccessStore::try_resume_session(self, space_id).await
        }
    }

    #[async_trait]
    impl VerifyKeychainAccessPort for DefaultSpaceAccessAdapter {
        async fn verify_keychain_access(&self) -> Result<bool, SpaceAccessError> {
            SpaceAccessStore::verify_keychain_access(self).await
        }
    }

    #[async_trait]
    impl DeriveSpaceSubkeyPort for DefaultSpaceAccessAdapter {
        async fn derive_subkey(
            &self,
            salt: &[u8],
            info: &[u8],
        ) -> Result<[u8; 32], SpaceAccessError> {
            SpaceAccessStore::derive_subkey(self, salt, info).await
        }
    }

    #[async_trait]
    impl CurrentSessionProofKeyPort for DefaultSpaceAccessAdapter {
        async fn current_session_proof_key(
            &self,
        ) -> Result<Option<ProofDerivedKey>, SpaceAccessError> {
            SpaceAccessStore::current_session_proof_key(self).await
        }
    }

    #[async_trait]
    impl PrepareJoinOfferPort for DefaultSpaceAccessAdapter {
        async fn prepare_join_offer(
            &self,
            space_id: &SpaceId,
            passphrase: &DomainPassphrase,
        ) -> Result<JoinOffer, SpaceAccessError> {
            SpaceAccessStore::prepare_join_offer(self, space_id, passphrase).await
        }
    }

    #[async_trait]
    impl DeriveProofKeyPort for DefaultSpaceAccessAdapter {
        async fn derive_master_key_for_proof(
            &self,
            offer: &JoinOffer,
            passphrase: &DomainPassphrase,
        ) -> Result<ProofDerivedKey, SpaceAccessError> {
            SpaceAccessStore::derive_master_key_for_proof(self, offer, passphrase).await
        }
    }

    #[async_trait]
    impl PrepareAdmissionOfferPort for DefaultSpaceAccessAdapter {
        async fn prepare_admission_offer(
            &self,
            space_id: &SpaceId,
            invitation: &InvitationCode,
            pairing_session_id: &SessionId,
        ) -> Result<PreparedAdmissionOffer, SpaceAccessError> {
            DefaultSpaceAccessAdapter::prepare_admission_offer(
                self,
                space_id,
                invitation,
                pairing_session_id,
            )
            .await
        }
    }

    #[async_trait]
    impl DeriveAdmissionProofKeyPort for DefaultSpaceAccessAdapter {
        async fn derive_admission_proof_key(
            &self,
            offer: &AdmissionOffer,
            passphrase: &DomainPassphrase,
            invitation: &InvitationCode,
            pairing_session_id: &SessionId,
        ) -> Result<ProofDerivedKey, SpaceAccessError> {
            DefaultSpaceAccessAdapter::derive_admission_proof_key(
                self,
                offer,
                passphrase,
                invitation,
                pairing_session_id,
            )
            .await
        }
    }

    #[async_trait]
    impl GroupAdmissionPort for DefaultSpaceAccessAdapter {
        async fn prepare_group_join(
            &self,
            device_id: &DeviceId,
        ) -> Result<PreparedGroupJoin, SpaceAccessError> {
            DefaultSpaceAccessAdapter::prepare_group_join(self, device_id).await
        }

        async fn admit_group_member(
            &self,
            space_id: &SpaceId,
            sponsor_device_id: &DeviceId,
            joiner_device_id: &DeviceId,
            existing_member_ids: &[DeviceId],
            key_package: &[u8],
        ) -> Result<GroupAdmission, SpaceAccessError> {
            DefaultSpaceAccessAdapter::admit_group_member(
                self,
                space_id,
                sponsor_device_id,
                joiner_device_id,
                existing_member_ids,
                key_package,
            )
            .await
        }

        async fn install_group_join(
            &self,
            space_id: &SpaceId,
            passphrase: &DomainPassphrase,
            pending: PreparedGroupJoin,
            welcome: &[u8],
            encrypted_key_catalog: &[u8],
            group_epoch: u64,
        ) -> Result<(), SpaceAccessError> {
            DefaultSpaceAccessAdapter::install_group_join(
                self,
                space_id,
                passphrase,
                pending,
                welcome,
                encrypted_key_catalog,
                group_epoch,
            )
            .await
        }
    }
}

#[async_trait]
impl GroupRevocationPort for DefaultSpaceAccessAdapter {
    async fn revoke_group_member(
        &self,
        target: &DeviceId,
        retained_recipients: &[DeviceId],
        now_ms: i64,
    ) -> Result<GroupRevocationResult, KeyEpochError> {
        DefaultSpaceAccessAdapter::revoke_group_member(self, target, retained_recipients, now_ms)
            .await
    }

    async fn acknowledge_group_update(
        &self,
        revocation_id: &RevocationId,
        recipient: &DeviceId,
        now_ms: i64,
    ) -> Result<GroupRevocationResult, KeyEpochError> {
        DefaultSpaceAccessAdapter::acknowledge_group_update(self, revocation_id, recipient, now_ms)
            .await
    }

    async fn apply_group_epoch_update(&self, payload: &[u8]) -> Result<GroupEpoch, KeyEpochError> {
        DefaultSpaceAccessAdapter::apply_group_epoch_update(self, payload).await
    }

    async fn pending_group_updates(
        &self,
        revocation_id: &RevocationId,
    ) -> Result<Vec<PendingGroupUpdate>, KeyEpochError> {
        DefaultSpaceAccessAdapter::pending_group_updates(self, revocation_id).await
    }

    async fn query_group_revocation(
        &self,
        revocation_id: &RevocationId,
    ) -> Result<Option<GroupRevocationResult>, KeyEpochError> {
        DefaultSpaceAccessAdapter::query_group_revocation(self, revocation_id).await
    }

    async fn resume_group_revocations(
        &self,
        now_ms: i64,
    ) -> Result<Vec<GroupRevocationResult>, KeyEpochError> {
        DefaultSpaceAccessAdapter::resume_group_revocations(self, now_ms).await
    }

    async fn pending_space_group_updates(&self) -> Result<Vec<PendingGroupUpdate>, KeyEpochError> {
        DefaultSpaceAccessAdapter::pending_space_group_updates(self).await
    }

    async fn acknowledge_space_group_update(
        &self,
        update_id: &str,
        now_ms: i64,
    ) -> Result<bool, KeyEpochError> {
        DefaultSpaceAccessAdapter::acknowledge_space_group_update(self, update_id, now_ms).await
    }
}

fn bootstrap_result(record: LegacyBootstrapRecord) -> Result<GroupBootstrapResult, BootstrapError> {
    match record.status() {
        LegacyBootstrapStatus::AwaitingReadmission => {
            Ok(GroupBootstrapResult::AwaitingReadmission {
                bootstrap_id: record.bootstrap_id().clone(),
                pending_members: record.pending_readmission().len(),
            })
        }
        LegacyBootstrapStatus::Complete => Ok(GroupBootstrapResult::Complete {
            bootstrap_id: record.bootstrap_id().clone(),
        }),
        LegacyBootstrapStatus::RecoveryRequired => Ok(GroupBootstrapResult::RecoveryRequired {
            bootstrap_id: record.bootstrap_id().clone(),
        }),
        LegacyBootstrapStatus::Prepared | LegacyBootstrapStatus::Staged => {
            Err(BootstrapError::InvalidRecord)
        }
    }
}

#[async_trait]
impl GroupBootstrapPort for DefaultSpaceAccessAdapter {
    async fn bootstrap_legacy_space(
        &self,
        sponsor: &DeviceId,
        retained_members: &[DeviceId],
        now_ms: i64,
    ) -> Result<GroupBootstrapResult, BootstrapError> {
        let repository = self.legacy_bootstrap_repository.as_ref().ok_or_else(|| {
            BootstrapError::Repository("legacy bootstrap is not configured".into())
        })?;
        let space_id = self
            .session
            .current_space_id()
            .map_err(|_| BootstrapError::CryptographicState)?;
        let prepared = LegacyBootstrapRecord::prepare(
            BootstrapId::generate(),
            space_id.clone(),
            sponsor.clone(),
            retained_members.to_vec(),
            now_ms,
        )?;
        let record = repository.begin_legacy_bootstrap(&prepared).await?;
        let bootstrap_id = record.bootstrap_id().clone();
        let material = match record.status() {
            LegacyBootstrapStatus::Prepared => {
                let sponsor_state = MlsGroupEngine::create_sponsor(
                    space_id.as_ref().as_bytes(),
                    record.sponsor_device_id().as_str().as_bytes(),
                )
                .map_err(|_| BootstrapError::CryptographicState)?;
                let material = self
                    .session
                    .create_legacy_bootstrap_material_in_group(
                        &space_id,
                        ProtectionGroupId::from_string(record.bootstrap_id().as_str())
                            .map_err(|_| BootstrapError::InvalidBootstrapId)?,
                        sponsor_state.into_bytes(),
                        now_ms,
                    )
                    .map_err(|_| BootstrapError::CryptographicState)?;
                let mut staged_record = record;
                staged_record.transition_to(LegacyBootstrapStatus::Staged, now_ms)?;
                let stage = LegacyBootstrapStage::new(staged_record, material.clone())?;
                repository.stage_legacy_bootstrap(&stage).await?;
                material
            }
            LegacyBootstrapStatus::Staged => repository
                .load_legacy_bootstrap_stage(record.bootstrap_id())
                .await?
                .ok_or(BootstrapError::InvalidStage)?
                .material()
                .clone(),
            LegacyBootstrapStatus::AwaitingReadmission
            | LegacyBootstrapStatus::Complete
            | LegacyBootstrapStatus::RecoveryRequired => return bootstrap_result(record),
        };
        let activated = repository
            .activate_legacy_bootstrap(&bootstrap_id, now_ms)
            .await?;
        self.session
            .install_space_material(&material)
            .map_err(|_| BootstrapError::SessionMaterial)?;
        bootstrap_result(activated)
    }

    async fn acknowledge_legacy_readmission(
        &self,
        bootstrap_id: &BootstrapId,
        member: &DeviceId,
        now_ms: i64,
    ) -> Result<GroupBootstrapResult, BootstrapError> {
        let repository = self.legacy_bootstrap_repository.as_ref().ok_or_else(|| {
            BootstrapError::Repository("legacy bootstrap is not configured".into())
        })?;
        bootstrap_result(
            repository
                .acknowledge_legacy_readmission(bootstrap_id, member, now_ms)
                .await?,
        )
    }

    async fn withdraw_legacy_readmission(
        &self,
        bootstrap_id: &BootstrapId,
        member: &DeviceId,
        now_ms: i64,
    ) -> Result<GroupBootstrapResult, BootstrapError> {
        let repository = self.legacy_bootstrap_repository.as_ref().ok_or_else(|| {
            BootstrapError::Repository("legacy bootstrap is not configured".into())
        })?;
        bootstrap_result(
            repository
                .acknowledge_legacy_readmission(bootstrap_id, member, now_ms)
                .await?,
        )
    }

    async fn query_legacy_bootstrap(
        &self,
        bootstrap_id: &BootstrapId,
    ) -> Result<Option<GroupBootstrapResult>, BootstrapError> {
        let repository = self.legacy_bootstrap_repository.as_ref().ok_or_else(|| {
            BootstrapError::Repository("legacy bootstrap is not configured".into())
        })?;
        repository
            .get_legacy_bootstrap(bootstrap_id)
            .await?
            .map(bootstrap_result)
            .transpose()
    }

    async fn resume_legacy_bootstraps(
        &self,
        now_ms: i64,
    ) -> Result<Vec<GroupBootstrapResult>, BootstrapError> {
        let repository = self.legacy_bootstrap_repository.as_ref().ok_or_else(|| {
            BootstrapError::Repository("legacy bootstrap is not configured".into())
        })?;
        let records = repository.list_incomplete_legacy_bootstraps().await?;
        let active_space_id = self
            .session
            .current_space_id()
            .map_err(|_| BootstrapError::CryptographicState)?;
        let active_material = self
            .key_epoch_repository
            .as_ref()
            .ok_or_else(|| {
                BootstrapError::Repository("key epoch repository is not configured".into())
            })?
            .load_space_material(&active_space_id)
            .await
            .map_err(|error| BootstrapError::Repository(error.to_string()))?;
        let mut results = Vec::with_capacity(records.len());
        for record in records {
            if active_material.as_ref().is_some_and(|material| {
                material.state().mode() == SpaceSecurityMode::Ready
                    && material
                        .state()
                        .protection_group_id()
                        .is_some_and(|group_id| group_id.as_str() != record.bootstrap_id().as_str())
            }) {
                continue;
            }
            match record.status() {
                LegacyBootstrapStatus::Prepared | LegacyBootstrapStatus::Staged => {
                    results.push(
                        self.bootstrap_legacy_space(
                            record.sponsor_device_id(),
                            record.pending_readmission(),
                            now_ms,
                        )
                        .await?,
                    );
                }
                LegacyBootstrapStatus::AwaitingReadmission => {
                    let key_epoch_repository =
                        self.key_epoch_repository.as_ref().ok_or_else(|| {
                            BootstrapError::Repository(
                                "key epoch repository is not configured".into(),
                            )
                        })?;
                    let material = key_epoch_repository
                        .load_space_material(record.space_id())
                        .await
                        .map_err(|error| BootstrapError::Repository(error.to_string()))?
                        .ok_or(BootstrapError::InvalidRecord)?;
                    let group_state = MlsClientState::from_bytes(material.group_state().to_vec());
                    for member in record.pending_readmission().to_vec() {
                        let is_member = MlsGroupEngine::contains_active_member(
                            &group_state,
                            member.as_str().as_bytes(),
                        )
                        .map_err(|_| BootstrapError::CryptographicState)?;
                        if is_member {
                            repository
                                .acknowledge_legacy_readmission(
                                    record.bootstrap_id(),
                                    &member,
                                    now_ms,
                                )
                                .await?;
                        }
                    }
                    let updated = repository
                        .get_legacy_bootstrap(record.bootstrap_id())
                        .await?
                        .ok_or(BootstrapError::InvalidRecord)?;
                    results.push(bootstrap_result(updated)?);
                }
                LegacyBootstrapStatus::Complete | LegacyBootstrapStatus::RecoveryRequired => {
                    results.push(bootstrap_result(record)?);
                }
            }
        }
        Ok(results)
    }
}

#[async_trait]
impl SpaceProtectionStatusPort for DefaultSpaceAccessAdapter {
    async fn query_space_protection(
        &self,
        members: &[DeviceId],
    ) -> Result<SpaceProtectionSnapshot, SpaceProtectionError> {
        let space_id = self
            .session
            .current_space_id()
            .map_err(|_| SpaceProtectionError::Unavailable)?;
        let key_epoch_repository = self
            .key_epoch_repository
            .as_ref()
            .ok_or(SpaceProtectionError::Unavailable)?;
        let material = key_epoch_repository
            .load_space_material(&space_id)
            .await
            .map_err(|error| SpaceProtectionError::Repository(error.to_string()))?;
        let legacy_bootstrap = if let Some(repository) = &self.legacy_bootstrap_repository {
            repository
                .list_legacy_bootstraps()
                .await
                .map_err(|error| SpaceProtectionError::Repository(error.to_string()))?
                .into_iter()
                .find(|record| {
                    record.space_id() == &space_id
                        && record.status() != LegacyBootstrapStatus::Complete
                        && material.as_ref().is_none_or(|material| {
                            material.state().mode() != SpaceSecurityMode::Ready
                                || material
                                    .state()
                                    .protection_group_id()
                                    .is_none_or(|group_id| {
                                        group_id.as_str() == record.bootstrap_id().as_str()
                                    })
                        })
                })
        } else {
            None
        };
        let mode = match (material.as_ref(), legacy_bootstrap.as_ref()) {
            (_, Some(record)) if record.status() == LegacyBootstrapStatus::RecoveryRequired => {
                SpaceProtectionMode::Migrating
            }
            (Some(material), _) => material.state().mode().into(),
            (None, Some(_)) => SpaceProtectionMode::Migrating,
            (None, None) => SpaceProtectionMode::Legacy,
        };
        let active_group = if mode == SpaceProtectionMode::Ready {
            let material = material.as_ref().ok_or(SpaceProtectionError::Corrupted)?;
            if material.group_state().is_empty() {
                return Err(SpaceProtectionError::Corrupted);
            }
            Some(MlsClientState::from_bytes(material.group_state().to_vec()))
        } else {
            None
        };
        let member_status =
            |member: &DeviceId| -> Result<MemberProtectionStatus, SpaceProtectionError> {
                if legacy_bootstrap.as_ref().is_some_and(|record| {
                    record.status() == LegacyBootstrapStatus::AwaitingReadmission
                        && record
                            .pending_readmission()
                            .iter()
                            .any(|pending| pending == member)
                }) {
                    return Ok(MemberProtectionStatus::AwaitingReadmission);
                }
                match mode {
                    SpaceProtectionMode::Legacy => Ok(MemberProtectionStatus::LegacyUnprotected),
                    SpaceProtectionMode::Migrating => Ok(MemberProtectionStatus::RecoveryRequired),
                    SpaceProtectionMode::Ready => {
                        let group = active_group
                            .as_ref()
                            .ok_or(SpaceProtectionError::Corrupted)?;
                        let is_active = MlsGroupEngine::contains_active_member(
                            group,
                            member.as_str().as_bytes(),
                        )
                        .map_err(|_| SpaceProtectionError::Corrupted)?;
                        Ok(if is_active {
                            MemberProtectionStatus::Protected
                        } else {
                            MemberProtectionStatus::RequiresReadmission
                        })
                    }
                }
            };
        let members = members
            .iter()
            .map(|device_id| {
                Ok(MemberProtection {
                    device_id: device_id.clone(),
                    status: member_status(device_id)?,
                })
            })
            .collect::<Result<Vec<_>, SpaceProtectionError>>()?;
        Ok(SpaceProtectionSnapshot {
            mode,
            members,
            legacy_bootstrap: legacy_bootstrap.map(|record| LegacyBootstrapProgress {
                bootstrap_id: record.bootstrap_id().clone(),
                status: record.status(),
                pending_readmission: record.pending_readmission().len(),
            }),
        })
    }
}

#[async_trait]
impl CurrentMemberSignaturePort for DefaultSpaceAccessAdapter {
    async fn current_member_epoch(&self) -> Result<u64, CurrentMemberSignatureError> {
        let group = self.current_member_group_state().await?;
        MlsGroupEngine::current_epoch(&group).map_err(|_| CurrentMemberSignatureError::InvalidState)
    }

    async fn sign_current_member_payload(
        &self,
        payload: &[u8],
    ) -> Result<Vec<u8>, CurrentMemberSignatureError> {
        let group = self.current_member_group_state().await?;
        MlsGroupEngine::sign_member_payload(&group, payload)
            .map_err(|_| CurrentMemberSignatureError::InvalidState)
    }

    async fn verify_current_member_payload(
        &self,
        member: &DeviceId,
        payload: &[u8],
        signature: &[u8],
    ) -> Result<bool, CurrentMemberSignatureError> {
        let group = self.current_member_group_state().await?;
        MlsGroupEngine::verify_member_payload(
            &group,
            member.as_str().as_bytes(),
            payload,
            signature,
        )
        .map_err(|_| CurrentMemberSignatureError::InvalidState)
    }
}

impl DefaultSpaceAccessAdapter {
    async fn current_member_group_state(
        &self,
    ) -> Result<MlsClientState, CurrentMemberSignatureError> {
        let space_id = self
            .session
            .current_space_id()
            .map_err(|_| CurrentMemberSignatureError::Unavailable)?;
        let repository = self
            .key_epoch_repository
            .as_ref()
            .ok_or(CurrentMemberSignatureError::Unavailable)?;
        let material = repository
            .load_space_material(&space_id)
            .await
            .map_err(|error| CurrentMemberSignatureError::Repository(error.to_string()))?
            .ok_or(CurrentMemberSignatureError::Unavailable)?;
        if material.state().mode() != SpaceSecurityMode::Ready || material.group_state().is_empty()
        {
            return Err(CurrentMemberSignatureError::InvalidState);
        }
        Ok(MlsClientState::from_bytes(material.group_state().to_vec()))
    }
}

#[cfg(test)]
mod admission_tests {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use tempfile::{tempdir, TempDir};
    use uc_core::crypto::domain::Passphrase;
    use uc_core::membership::{
        BeginRevocationOutcome, BootstrapError, BootstrapId, ContentKeyId, ContentKeyPurpose,
        GroupBootstrapPort, GroupBootstrapResult, KeyEpochError, LegacyBootstrapRecord,
        LegacyBootstrapRepositoryPort, LegacyBootstrapStage, LegacyBootstrapStatus,
        LegacyProtectionCommand, LegacyProtectionPort, LegacyProtectionResult,
        LegacyRequestInspection, LegacyUpgradeDescriptor, LegacyUpgradeRequest, RevocationId,
        RevocationRecord, RevocationStage, RevocationStatus,
    };
    use uc_core::pairing::InvitationCode;
    use uc_core::ports::{SecureStorageError, SecureStoragePort};

    use super::*;
    use crate::db::executor::DieselSqliteExecutor;
    use crate::db::pool::init_db_pool;
    use crate::db::repositories::DieselSpaceSecurityStore;
    use crate::fs::key_slot_store::JsonKeySlotStore;
    use crate::security::legacy_upgrade::proof::{request_id, request_transcript};
    use crate::security::legacy_upgrade::DefaultLegacyProtection;
    use crate::security::DefaultCurrentProfile;

    mockall::mock! {
        SecureStorage {}

        impl SecureStoragePort for SecureStorage {
            fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SecureStorageError>;
            fn set(&self, key: &str, value: &[u8]) -> Result<(), SecureStorageError>;
            fn delete(&self, key: &str) -> Result<(), SecureStorageError>;
        }
    }

    struct MemoryLegacyBootstrapRepository {
        record: Mutex<Option<LegacyBootstrapRecord>>,
        stage: Mutex<Option<LegacyBootstrapStage>>,
    }

    impl MemoryLegacyBootstrapRepository {
        fn new() -> Self {
            Self {
                record: Mutex::new(None),
                stage: Mutex::new(None),
            }
        }
    }

    #[async_trait]
    impl LegacyBootstrapRepositoryPort for MemoryLegacyBootstrapRepository {
        async fn begin_legacy_bootstrap(
            &self,
            prepared: &LegacyBootstrapRecord,
        ) -> Result<LegacyBootstrapRecord, BootstrapError> {
            let mut record = self.record.lock().unwrap();
            if let Some(existing) = record.as_ref() {
                return Ok(existing.clone());
            }
            *record = Some(prepared.clone());
            Ok(prepared.clone())
        }

        async fn stage_legacy_bootstrap(
            &self,
            stage: &LegacyBootstrapStage,
        ) -> Result<(), BootstrapError> {
            *self.record.lock().unwrap() = Some(stage.record().clone());
            *self.stage.lock().unwrap() = Some(stage.clone());
            Ok(())
        }

        async fn activate_legacy_bootstrap(
            &self,
            bootstrap_id: &BootstrapId,
            now_ms: i64,
        ) -> Result<LegacyBootstrapRecord, BootstrapError> {
            let mut record = self.record.lock().unwrap();
            let current = record.as_mut().ok_or(BootstrapError::InvalidRecord)?;
            if current.bootstrap_id() != bootstrap_id {
                return Err(BootstrapError::InvalidRecord);
            }
            if current.status() == LegacyBootstrapStatus::Staged {
                let status = if current.pending_readmission().is_empty() {
                    LegacyBootstrapStatus::Complete
                } else {
                    LegacyBootstrapStatus::AwaitingReadmission
                };
                current.transition_to(status, now_ms)?;
            }
            Ok(current.clone())
        }

        async fn load_legacy_bootstrap_stage(
            &self,
            bootstrap_id: &BootstrapId,
        ) -> Result<Option<LegacyBootstrapStage>, BootstrapError> {
            Ok(self
                .stage
                .lock()
                .unwrap()
                .as_ref()
                .filter(|stage| stage.record().bootstrap_id() == bootstrap_id)
                .cloned())
        }

        async fn get_legacy_bootstrap(
            &self,
            bootstrap_id: &BootstrapId,
        ) -> Result<Option<LegacyBootstrapRecord>, BootstrapError> {
            Ok(self
                .record
                .lock()
                .unwrap()
                .as_ref()
                .filter(|record| record.bootstrap_id() == bootstrap_id)
                .cloned())
        }

        async fn list_incomplete_legacy_bootstraps(
            &self,
        ) -> Result<Vec<LegacyBootstrapRecord>, BootstrapError> {
            Ok(self
                .record
                .lock()
                .unwrap()
                .iter()
                .filter(|record| !record.status().is_terminal())
                .cloned()
                .collect())
        }

        async fn list_legacy_bootstraps(
            &self,
        ) -> Result<Vec<LegacyBootstrapRecord>, BootstrapError> {
            Ok(self.record.lock().unwrap().iter().cloned().collect())
        }

        async fn acknowledge_legacy_readmission(
            &self,
            bootstrap_id: &BootstrapId,
            member: &DeviceId,
            now_ms: i64,
        ) -> Result<LegacyBootstrapRecord, BootstrapError> {
            let mut record = self.record.lock().unwrap();
            let current = record.as_mut().ok_or(BootstrapError::InvalidRecord)?;
            if current.bootstrap_id() != bootstrap_id {
                return Err(BootstrapError::InvalidRecord);
            }
            current.mark_readmitted(member, now_ms)?;
            Ok(current.clone())
        }
    }

    fn memory_secure_storage() -> Arc<MockSecureStorage> {
        let values = Arc::new(Mutex::new(HashMap::<String, Vec<u8>>::new()));
        let mut mock = MockSecureStorage::new();
        let get_values = values.clone();
        mock.expect_get()
            .returning(move |key| Ok(get_values.lock().unwrap().get(key).cloned()));
        let set_values = values.clone();
        mock.expect_set().returning(move |key, value| {
            set_values
                .lock()
                .unwrap()
                .insert(key.to_string(), value.to_vec());
            Ok(())
        });
        mock.expect_delete().returning(move |key| {
            values.lock().unwrap().remove(key);
            Ok(())
        });
        Arc::new(mock)
    }

    mockall::mock! {
        RevocationRepository {}

        #[async_trait]
        impl RevocationRepositoryPort for RevocationRepository {
            async fn save_space_material(
                &self,
                material: &SpaceKeyMaterial,
            ) -> Result<(), KeyEpochError>;
            async fn load_space_material(
                &self,
                space_id: &SpaceId,
            ) -> Result<Option<SpaceKeyMaterial>, KeyEpochError>;
            async fn begin_revocation(
                &self,
                prepared: &RevocationRecord,
            ) -> Result<BeginRevocationOutcome, KeyEpochError>;
            async fn get_revocation(
                &self,
                revocation_id: &RevocationId,
            ) -> Result<Option<RevocationRecord>, KeyEpochError>;
            async fn list_incomplete_revocations(
                &self,
            ) -> Result<Vec<RevocationRecord>, KeyEpochError>;
            async fn stage_revocation(
                &self,
                stage: &RevocationStage,
            ) -> Result<(), KeyEpochError>;
            async fn load_staged_revocation(
                &self,
                revocation_id: &RevocationId,
            ) -> Result<Option<RevocationStage>, KeyEpochError>;
            async fn activate_revocation(
                &self,
                revocation_id: &RevocationId,
                now_ms: i64,
            ) -> Result<RevocationRecord, KeyEpochError>;
            async fn start_distribution(
                &self,
                revocation_id: &RevocationId,
                now_ms: i64,
            ) -> Result<RevocationRecord, KeyEpochError>;
            async fn acknowledge_recipient(
                &self,
                revocation_id: &RevocationId,
                recipient: &DeviceId,
                now_ms: i64,
            ) -> Result<RevocationRecord, KeyEpochError>;
        }
    }

    fn memory_revocation_repository_with_stage_persistence(
        initial_material: Option<SpaceKeyMaterial>,
        persist_stage: bool,
    ) -> (
        Arc<MockRevocationRepository>,
        Arc<AtomicBool>,
        Arc<AtomicUsize>,
    ) {
        let material = Arc::new(Mutex::new(initial_material));
        let record = Arc::new(Mutex::new(None::<RevocationRecord>));
        let stage = Arc::new(Mutex::new(None::<RevocationStage>));
        let fail_saves = Arc::new(AtomicBool::new(false));
        let stage_calls = Arc::new(AtomicUsize::new(0));
        let mut mock = MockRevocationRepository::new();

        let save_material = material.clone();
        let save_failures = fail_saves.clone();
        mock.expect_save_space_material().returning(move |value| {
            if save_failures.load(Ordering::Acquire) {
                return Err(KeyEpochError::Repository("injected save failure".into()));
            }
            *save_material.lock().unwrap() = Some(value.clone());
            Ok(())
        });

        let load_material = material.clone();
        mock.expect_load_space_material()
            .returning(move |space_id| {
                Ok(load_material
                    .lock()
                    .unwrap()
                    .as_ref()
                    .filter(|value| value.state().space_id() == space_id)
                    .cloned())
            });

        let begin_record = record.clone();
        mock.expect_begin_revocation().returning(move |prepared| {
            let mut current = begin_record.lock().unwrap();
            if let Some(existing) = current.as_ref() {
                return Ok(BeginRevocationOutcome::Existing(existing.clone()));
            }
            *current = Some(prepared.clone());
            Ok(BeginRevocationOutcome::Begun(prepared.clone()))
        });

        let get_record = record.clone();
        mock.expect_get_revocation()
            .returning(move |revocation_id| {
                Ok(get_record
                    .lock()
                    .unwrap()
                    .as_ref()
                    .filter(|value| value.revocation_id() == revocation_id)
                    .cloned())
            });

        let list_record = record.clone();
        mock.expect_list_incomplete_revocations()
            .returning(move || {
                Ok(list_record
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|value| !value.status().is_terminal())
                    .cloned()
                    .collect())
            });

        let staged_record = record.clone();
        let staged_value = stage.clone();
        let recorded_stage_calls = stage_calls.clone();
        mock.expect_stage_revocation().returning(move |value| {
            let call = recorded_stage_calls.fetch_add(1, Ordering::AcqRel) + 1;
            if persist_stage {
                *staged_record.lock().unwrap() = Some(value.record().clone());
                *staged_value.lock().unwrap() = Some(value.clone());
            } else if call > 3 {
                return Err(KeyEpochError::Repository(
                    "test repository observed excessive staging retries".into(),
                ));
            }
            Ok(())
        });

        let load_stage = stage.clone();
        mock.expect_load_staged_revocation()
            .returning(move |revocation_id| {
                Ok(load_stage
                    .lock()
                    .unwrap()
                    .as_ref()
                    .filter(|value| value.record().revocation_id() == revocation_id)
                    .cloned())
            });

        let activate_material = material.clone();
        let activate_record = record.clone();
        let activate_stage = stage.clone();
        mock.expect_activate_revocation()
            .returning(move |revocation_id, now_ms| {
                let mut current = activate_stage.lock().unwrap();
                let value = current
                    .as_mut()
                    .filter(|value| value.record().revocation_id() == revocation_id)
                    .ok_or_else(|| KeyEpochError::Repository("stage not found".into()))?;
                value.transition_to(RevocationStatus::Activated, now_ms)?;
                let activated = value.record().clone();
                *activate_material.lock().unwrap() = Some(SpaceKeyMaterial::new(
                    value.next_space_state().clone(),
                    value.group_state().to_vec(),
                    value.key_catalog().to_vec(),
                    now_ms,
                ));
                *activate_record.lock().unwrap() = Some(activated.clone());
                Ok(activated)
            });

        let distribution_record = record.clone();
        let distribution_stage = stage.clone();
        mock.expect_start_distribution()
            .returning(move |revocation_id, now_ms| {
                let mut current = distribution_stage.lock().unwrap();
                let value = current
                    .as_mut()
                    .filter(|value| value.record().revocation_id() == revocation_id)
                    .ok_or_else(|| KeyEpochError::Repository("stage not found".into()))?;
                value.transition_to(RevocationStatus::Distributing, now_ms)?;
                if value.all_recipients_confirmed() {
                    value.transition_to(RevocationStatus::Complete, now_ms)?;
                }
                let distributing = value.record().clone();
                *distribution_record.lock().unwrap() = Some(distributing.clone());
                if distributing.status() == RevocationStatus::Complete {
                    *current = None;
                }
                Ok(distributing)
            });

        let acknowledge_record = record;
        let acknowledge_stage = stage;
        mock.expect_acknowledge_recipient()
            .returning(move |revocation_id, recipient, now_ms| {
                let mut current = acknowledge_stage.lock().unwrap();
                let value = current
                    .as_mut()
                    .filter(|value| value.record().revocation_id() == revocation_id)
                    .ok_or_else(|| KeyEpochError::Repository("stage not found".into()))?;
                value.acknowledge_recipient(recipient, now_ms)?;
                if value.all_recipients_confirmed() {
                    value.transition_to(RevocationStatus::Complete, now_ms)?;
                }
                let acknowledged = value.record().clone();
                *acknowledge_record.lock().unwrap() = Some(acknowledged.clone());
                if acknowledged.status() == RevocationStatus::Complete {
                    *current = None;
                }
                Ok(acknowledged)
            });

        (Arc::new(mock), fail_saves, stage_calls)
    }

    fn memory_revocation_repository(
        initial_material: Option<SpaceKeyMaterial>,
    ) -> (Arc<MockRevocationRepository>, Arc<AtomicBool>) {
        let (repository, fail_saves, _) =
            memory_revocation_repository_with_stage_persistence(initial_material, true);
        (repository, fail_saves)
    }

    fn local_key_material(
        directory: &TempDir,
        secure_storage: Arc<MockSecureStorage>,
    ) -> Arc<KeyMaterialStore> {
        Arc::new(KeyMaterialStore::new(
            secure_storage,
            Arc::new(JsonKeySlotStore::new(directory.path().to_path_buf())),
        ))
    }

    fn adapter(
        key_material: Arc<KeyMaterialStore>,
        session: Arc<InMemorySession>,
        repository: Arc<MockRevocationRepository>,
    ) -> DefaultSpaceAccessAdapter {
        DefaultSpaceAccessAdapter::new_with_key_epoch_repository(
            key_material,
            Arc::new(DefaultCurrentProfile::new()),
            session,
            repository,
        )
    }

    #[tokio::test]
    async fn legacy_bootstrap_creates_a_real_sponsor_group_and_waits_for_readmission() {
        let directory = tempdir().unwrap();
        let session = Arc::new(InMemorySession::new());
        let space_id = SpaceId::from("legacy-bootstrap-space");
        session.set_master_key_for_space(
            space_id.clone(),
            MasterKey::from_bytes(&[0x7a; 32]).unwrap(),
        );
        let bootstrap_repository = Arc::new(MemoryLegacyBootstrapRepository::new());
        let key_epoch_repository: Arc<dyn RevocationRepositoryPort> =
            Arc::new(MockRevocationRepository::new());
        let adapter = DefaultSpaceAccessAdapter::new_with_security_repositories(
            local_key_material(&directory, memory_secure_storage()),
            Arc::new(DefaultCurrentProfile::new()),
            Arc::clone(&session),
            key_epoch_repository,
            bootstrap_repository.clone(),
        );
        let sponsor = DeviceId::new("sponsor-device");
        let retained = DeviceId::new("retained-device");

        let result = adapter
            .bootstrap_legacy_space(&sponsor, &[retained.clone()], 100)
            .await
            .unwrap();
        let bootstrap_id = match result {
            GroupBootstrapResult::AwaitingReadmission {
                bootstrap_id,
                pending_members,
            } => {
                assert_eq!(pending_members, 1);
                bootstrap_id
            }
            other => panic!("unexpected bootstrap result: {other:?}"),
        };
        let stage = bootstrap_repository
            .load_legacy_bootstrap_stage(&bootstrap_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            stage
                .material()
                .state()
                .protection_group_id()
                .map(|id| id.as_str()),
            Some(bootstrap_id.as_str())
        );
        assert!(MlsGroupEngine::contains_active_member(
            &MlsClientState::from_bytes(stage.material().group_state().to_vec()),
            sponsor.as_str().as_bytes(),
        )
        .unwrap());
        assert!(!MlsGroupEngine::contains_active_member(
            &MlsClientState::from_bytes(stage.material().group_state().to_vec()),
            retained.as_str().as_bytes(),
        )
        .unwrap());
        assert_eq!(
            session
                .current_content_key(&space_id, ContentKeyPurpose::Content)
                .unwrap()
                .epoch(),
            GroupEpoch::new(1)
        );

        assert!(matches!(
            adapter
                .acknowledge_legacy_readmission(&bootstrap_id, &retained, 110)
                .await
                .unwrap(),
            GroupBootstrapResult::Complete { .. }
        ));
    }

    #[tokio::test]
    async fn withdrawing_legacy_readmission_removes_the_pending_member() {
        let directory = tempdir().unwrap();
        let session = Arc::new(InMemorySession::new());
        let space_id = SpaceId::from("legacy-bootstrap-withdrawal");
        session.set_master_key_for_space(space_id, MasterKey::from_bytes(&[0x7b; 32]).unwrap());
        let bootstrap_repository = Arc::new(MemoryLegacyBootstrapRepository::new());
        let key_epoch_repository: Arc<dyn RevocationRepositoryPort> =
            Arc::new(MockRevocationRepository::new());
        let adapter = DefaultSpaceAccessAdapter::new_with_security_repositories(
            local_key_material(&directory, memory_secure_storage()),
            Arc::new(DefaultCurrentProfile::new()),
            Arc::clone(&session),
            key_epoch_repository,
            bootstrap_repository.clone(),
        );
        let sponsor = DeviceId::new("sponsor-device");
        let removed = DeviceId::new("legacy-device");
        let bootstrap_id = match adapter
            .bootstrap_legacy_space(&sponsor, &[removed.clone()], 100)
            .await
            .unwrap()
        {
            GroupBootstrapResult::AwaitingReadmission { bootstrap_id, .. } => bootstrap_id,
            other => panic!("unexpected bootstrap result: {other:?}"),
        };

        let result = adapter
            .withdraw_legacy_readmission(&bootstrap_id, &removed, 110)
            .await
            .unwrap();

        assert!(matches!(result, GroupBootstrapResult::Complete { .. }));
        let record = bootstrap_repository
            .get_legacy_bootstrap(&bootstrap_id)
            .await
            .unwrap()
            .unwrap();
        assert!(record.pending_readmission().is_empty());
    }

    #[tokio::test]
    async fn legacy_upgrade_request_is_verified_only_for_the_bound_peer_pair() {
        let directory = tempdir().unwrap();
        let session = Arc::new(InMemorySession::new());
        session.set_master_key_for_space(
            SpaceId::from("legacy-upgrade-space"),
            MasterKey::from_bytes(&[0x31; 32]).unwrap(),
        );
        let (repository, _) = memory_revocation_repository(None);
        let space_access = Arc::new(adapter(
            local_key_material(&directory, memory_secure_storage()),
            Arc::clone(&session),
            repository,
        ));
        let pool = init_db_pool(":memory:").unwrap();
        let attempt_store = Arc::new(DieselSpaceSecurityStore::new(
            DieselSqliteExecutor::new(pool),
            session.as_ref().clone(),
        ));
        let protection = DefaultLegacyProtection::new(space_access, attempt_store);

        let request = protection
            .begin_attempt(&DeviceId::new("device-a"), &DeviceId::new("device-b"))
            .await
            .unwrap();
        assert_eq!(
            protection.inspect_request(&request).await.unwrap(),
            LegacyRequestInspection::Verified
        );

        let replay = LegacyUpgradeRequest::unsigned(
            DeviceId::new("device-a"),
            DeviceId::new("device-c"),
            request.descriptor().clone(),
            request.key_package().to_vec(),
        )
        .with_proof(request.proof().to_vec());
        assert_eq!(
            protection.inspect_request(&replay).await.unwrap(),
            LegacyRequestInspection::Invalid
        );
    }

    #[tokio::test]
    async fn legacy_ready_material_without_group_id_is_backfilled_once() {
        let directory = tempdir().unwrap();
        let database_url = directory.path().join("legacy-ready-backfill.sqlite");
        let pool = init_db_pool(database_url.to_str().unwrap()).unwrap();
        let session = Arc::new(InMemorySession::new());
        let space_id = SpaceId::from("legacy-ready-backfill-space");
        session.set_master_key_for_space(
            space_id.clone(),
            MasterKey::from_bytes(&[0x36; 32]).unwrap(),
        );
        let sponsor = DeviceId::new("device-a");
        let sponsor_state = MlsGroupEngine::create_sponsor(
            space_id.as_ref().as_bytes(),
            sponsor.as_str().as_bytes(),
        )
        .unwrap();
        let material = session
            .create_legacy_bootstrap_material_in_group(
                &space_id,
                ProtectionGroupId::from_string("removed-by-old-format").unwrap(),
                sponsor_state.into_bytes(),
                100,
            )
            .unwrap();
        let mut legacy_json = serde_json::to_value(material).unwrap();
        let removed = legacy_json
            .get_mut("state")
            .and_then(serde_json::Value::as_object_mut)
            .and_then(|state| state.remove("protection_group_id"));
        assert!(removed.is_some());
        let legacy_material: SpaceKeyMaterial = serde_json::from_value(legacy_json).unwrap();
        assert!(legacy_material.state().protection_group_id().is_none());

        let repository = Arc::new(DieselSpaceSecurityStore::new(
            DieselSqliteExecutor::new(pool.clone()),
            session.as_ref().clone(),
        ));
        repository
            .save_space_material(&legacy_material)
            .await
            .unwrap();
        let repository_port: Arc<dyn RevocationRepositoryPort> = repository.clone();
        let space_access = Arc::new(DefaultSpaceAccessAdapter::new_with_key_epoch_repository(
            local_key_material(&directory, memory_secure_storage()),
            Arc::new(DefaultCurrentProfile::new()),
            Arc::clone(&session),
            repository_port,
        ));
        let protection = DefaultLegacyProtection::new(space_access, repository.clone());

        let first = protection.snapshot(&[]).await.unwrap().descriptor;
        assert!(first.is_ready());
        let first_group_id = first.protection_group_id().cloned().unwrap();
        let second = protection.snapshot(&[]).await.unwrap().descriptor;
        assert_eq!(second.protection_group_id(), Some(&first_group_id));

        drop(protection);
        let reopened_repository = DieselSpaceSecurityStore::new(
            DieselSqliteExecutor::new(pool),
            session.as_ref().clone(),
        );
        let persisted = reopened_repository
            .load_space_material(&space_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            persisted.state().protection_group_id(),
            Some(&first_group_id)
        );
    }

    #[tokio::test]
    async fn pending_upgrade_request_is_reused_after_adapter_restart() {
        let directory = tempdir().unwrap();
        let database_url = directory.path().join("legacy-upgrade.sqlite");
        let pool = init_db_pool(database_url.to_str().unwrap()).unwrap();
        let session = Arc::new(InMemorySession::new());
        session.set_master_key_for_space(
            SpaceId::from("legacy-upgrade-restart-space"),
            MasterKey::from_bytes(&[0x35; 32]).unwrap(),
        );
        let (key_epoch_repository, _) = memory_revocation_repository(None);
        let upgrade_repository = Arc::new(DieselSpaceSecurityStore::new(
            DieselSqliteExecutor::new(pool.clone()),
            session.as_ref().clone(),
        ));
        let first_space_access = Arc::new(adapter(
            local_key_material(&directory, memory_secure_storage()),
            Arc::clone(&session),
            Arc::clone(&key_epoch_repository),
        ));
        let first_adapter = DefaultLegacyProtection::new(first_space_access, upgrade_repository);
        let local = DeviceId::new("device-b");
        let peer = DeviceId::new("device-a");
        let first = first_adapter.begin_attempt(&local, &peer).await.unwrap();
        drop(first_adapter);

        let reopened_repository = Arc::new(DieselSpaceSecurityStore::new(
            DieselSqliteExecutor::new(pool),
            session.as_ref().clone(),
        ));
        let reopened_space_access = Arc::new(adapter(
            local_key_material(&directory, memory_secure_storage()),
            session,
            key_epoch_repository,
        ));
        let reopened_adapter =
            DefaultLegacyProtection::new(reopened_space_access, reopened_repository);
        let restored = reopened_adapter.begin_attempt(&local, &peer).await.unwrap();

        assert_eq!(restored, first);
    }

    #[tokio::test]
    async fn legacy_admission_and_replay_response_share_one_durable_material() {
        let directory = tempdir().unwrap();
        let session = Arc::new(InMemorySession::new());
        let space_id = SpaceId::from("legacy-admission-cache-space");
        session.set_master_key_for_space(
            space_id.clone(),
            MasterKey::from_bytes(&[0x34; 32]).unwrap(),
        );
        let sponsor = DeviceId::new("device-a");
        let joiner = DeviceId::new("device-b");
        let sponsor_state = MlsGroupEngine::create_sponsor(
            space_id.as_ref().as_bytes(),
            sponsor.as_str().as_bytes(),
        )
        .unwrap();
        let material = session
            .create_legacy_bootstrap_material_in_group(
                &space_id,
                ProtectionGroupId::from_string("group-a").unwrap(),
                sponsor_state.into_bytes(),
                100,
            )
            .unwrap();
        session.install_space_material(&material).unwrap();
        let (repository, _) = memory_revocation_repository(Some(material));
        let space_access = Arc::new(adapter(
            local_key_material(&directory, memory_secure_storage()),
            Arc::clone(&session),
            Arc::clone(&repository),
        ));
        let pending = space_access.prepare_group_join(&joiner).await.unwrap();
        let unsigned = LegacyUpgradeRequest::unsigned(
            joiner,
            sponsor,
            LegacyUpgradeDescriptor::legacy(session.legacy_upgrade_id().unwrap()),
            pending.key_package.clone(),
        );
        let proof = session
            .legacy_upgrade_proof(&request_transcript(&unsigned))
            .unwrap();
        let request = unsigned.with_proof(proof.to_vec());
        let attempt_pool = init_db_pool(":memory:").unwrap();
        let attempt_store = Arc::new(DieselSpaceSecurityStore::new(
            DieselSqliteExecutor::new(attempt_pool),
            session.as_ref().clone(),
        ));
        let protection = DefaultLegacyProtection::new(space_access, attempt_store);

        let result = protection
            .execute(LegacyProtectionCommand::AdmitMember {
                sponsor,
                existing_members: Vec::new(),
                request: request.clone(),
            })
            .await
            .unwrap();
        let LegacyProtectionResult::MemberAdmitted(admission) = result else {
            panic!("expected admitted member result");
        };
        let persisted = repository
            .load_space_material(&space_id)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(
            persisted
                .cached_group_admission(request.source_device_id(), request_id(&request),)
                .map(|cached| (
                    &cached.protection_group_id,
                    &cached.admission.welcome,
                    &cached.admission.encrypted_key_catalog,
                    cached.admission.group_epoch,
                )),
            Some((
                &admission.protection_group_id,
                &admission.admission.welcome,
                &admission.admission.encrypted_key_catalog,
                admission.admission.group_epoch,
            ))
        );
        assert_eq!(
            protection.inspect_request(&request).await.unwrap(),
            LegacyRequestInspection::Replay(admission)
        );
    }

    #[tokio::test]
    async fn protection_status_ignores_a_superseded_local_bootstrap_after_convergence() {
        let directory = tempdir().unwrap();
        let session = Arc::new(InMemorySession::new());
        let space_id = SpaceId::from("legacy-convergence-space");
        session.set_master_key_for_space(
            space_id.clone(),
            MasterKey::from_bytes(&[0x32; 32]).unwrap(),
        );
        let bootstrap_repository = Arc::new(MemoryLegacyBootstrapRepository::new());
        let (key_epoch_repository, _) = memory_revocation_repository(None);
        let key_epoch_port: Arc<dyn RevocationRepositoryPort> = key_epoch_repository.clone();
        let adapter = DefaultSpaceAccessAdapter::new_with_security_repositories(
            local_key_material(&directory, memory_secure_storage()),
            Arc::new(DefaultCurrentProfile::new()),
            Arc::clone(&session),
            key_epoch_port,
            bootstrap_repository,
        );
        let sponsor = DeviceId::new("device-b");
        let retained = DeviceId::new("device-c");
        adapter
            .bootstrap_legacy_space(&sponsor, &[retained], 100)
            .await
            .unwrap();

        let winning_state = MlsGroupEngine::create_sponsor(
            space_id.as_ref().as_bytes(),
            sponsor.as_str().as_bytes(),
        )
        .unwrap();
        let winning_material = session
            .create_legacy_bootstrap_material_in_group(
                &space_id,
                ProtectionGroupId::from_string("000-winning-group").unwrap(),
                winning_state.into_bytes(),
                200,
            )
            .unwrap();
        key_epoch_repository
            .save_space_material(&winning_material)
            .await
            .unwrap();
        session.install_space_material(&winning_material).unwrap();

        let snapshot = adapter
            .query_space_protection(&[sponsor, retained])
            .await
            .unwrap();

        assert_eq!(snapshot.mode, SpaceProtectionMode::Ready);
        assert!(snapshot.legacy_bootstrap.is_none());
        assert_eq!(
            snapshot.members[1].status,
            MemberProtectionStatus::RequiresReadmission
        );
        assert!(adapter
            .resume_legacy_bootstraps(300)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn group_admission_confirms_a_pending_legacy_readmission() {
        let directory = tempdir().unwrap();
        let session = Arc::new(InMemorySession::new());
        let space_id = SpaceId::from("legacy-readmission-space");
        session.set_master_key_for_space(
            space_id.clone(),
            MasterKey::from_bytes(&[0x7b; 32]).unwrap(),
        );
        let bootstrap_repository = Arc::new(MemoryLegacyBootstrapRepository::new());
        let (key_epoch_repository, _) = memory_revocation_repository(None);
        let key_epoch_port: Arc<dyn RevocationRepositoryPort> = key_epoch_repository.clone();
        let adapter = DefaultSpaceAccessAdapter::new_with_security_repositories(
            local_key_material(&directory, memory_secure_storage()),
            Arc::new(DefaultCurrentProfile::new()),
            Arc::clone(&session),
            key_epoch_port,
            bootstrap_repository.clone(),
        );
        let sponsor = DeviceId::new("sponsor-device");
        let retained = DeviceId::new("retained-device");
        let bootstrap_id = match adapter
            .bootstrap_legacy_space(&sponsor, &[retained.clone()], 100)
            .await
            .unwrap()
        {
            GroupBootstrapResult::AwaitingReadmission { bootstrap_id, .. } => bootstrap_id,
            other => panic!("unexpected bootstrap result: {other:?}"),
        };
        let stage = bootstrap_repository
            .load_legacy_bootstrap_stage(&bootstrap_id)
            .await
            .unwrap()
            .unwrap();
        key_epoch_repository
            .save_space_material(stage.material())
            .await
            .unwrap();
        let pending = adapter.prepare_group_join(&retained).await.unwrap();

        adapter
            .admit_group_member(&space_id, &sponsor, &retained, &[], &pending.key_package)
            .await
            .unwrap();

        assert_eq!(
            bootstrap_repository
                .get_legacy_bootstrap(&bootstrap_id)
                .await
                .unwrap()
                .unwrap()
                .status(),
            LegacyBootstrapStatus::Complete
        );
    }

    #[tokio::test]
    async fn bootstrap_recovery_confirms_readmission_from_persisted_mls_state() {
        let directory = tempdir().unwrap();
        let session = Arc::new(InMemorySession::new());
        let space_id = SpaceId::from("legacy-recovery-space");
        session.set_master_key_for_space(
            space_id.clone(),
            MasterKey::from_bytes(&[0x6d; 32]).unwrap(),
        );
        let bootstrap_repository = Arc::new(MemoryLegacyBootstrapRepository::new());
        let (key_epoch_repository, _) = memory_revocation_repository(None);
        let key_epoch_port: Arc<dyn RevocationRepositoryPort> = key_epoch_repository.clone();
        let recovery_adapter = DefaultSpaceAccessAdapter::new_with_security_repositories(
            local_key_material(&directory, memory_secure_storage()),
            Arc::new(DefaultCurrentProfile::new()),
            Arc::clone(&session),
            key_epoch_port.clone(),
            bootstrap_repository.clone(),
        );
        let sponsor = DeviceId::new("sponsor-device");
        let retained = DeviceId::new("retained-device");
        let bootstrap_id = match recovery_adapter
            .bootstrap_legacy_space(&sponsor, &[retained.clone()], 100)
            .await
            .unwrap()
        {
            GroupBootstrapResult::AwaitingReadmission { bootstrap_id, .. } => bootstrap_id,
            other => panic!("unexpected bootstrap result: {other:?}"),
        };
        let stage = bootstrap_repository
            .load_legacy_bootstrap_stage(&bootstrap_id)
            .await
            .unwrap()
            .unwrap();
        key_epoch_repository
            .save_space_material(stage.material())
            .await
            .unwrap();
        let before_readmission = recovery_adapter
            .query_space_protection(&[sponsor.clone(), retained.clone()])
            .await
            .unwrap();
        assert_eq!(before_readmission.mode, SpaceProtectionMode::Ready);
        assert!(matches!(
            before_readmission.members.as_slice(),
            [
                MemberProtection {
                    status: MemberProtectionStatus::Protected,
                    ..
                },
                MemberProtection {
                    status: MemberProtectionStatus::AwaitingReadmission,
                    ..
                },
            ]
        ));
        assert!(matches!(
            before_readmission.legacy_bootstrap,
            Some(LegacyBootstrapProgress { bootstrap_id: ref actual, .. }) if actual == &bootstrap_id
        ));

        // Simulate a process exit after the admission commit persisted but before
        // the bootstrap record acknowledgement could run.
        let admission_adapter = DefaultSpaceAccessAdapter::new_with_security_repositories(
            local_key_material(&directory, memory_secure_storage()),
            Arc::new(DefaultCurrentProfile::new()),
            Arc::clone(&session),
            key_epoch_port,
            Arc::new(MemoryLegacyBootstrapRepository::new()),
        );
        let pending = admission_adapter
            .prepare_group_join(&retained)
            .await
            .unwrap();
        admission_adapter
            .admit_group_member(&space_id, &sponsor, &retained, &[], &pending.key_package)
            .await
            .unwrap();

        let resumed = recovery_adapter
            .resume_legacy_bootstraps(200)
            .await
            .unwrap();

        assert!(matches!(
            resumed.as_slice(),
            [GroupBootstrapResult::Complete { bootstrap_id: actual }] if actual == &bootstrap_id
        ));
        assert_eq!(
            bootstrap_repository
                .get_legacy_bootstrap(&bootstrap_id)
                .await
                .unwrap()
                .unwrap()
                .status(),
            LegacyBootstrapStatus::Complete
        );
        let after_recovery = recovery_adapter
            .query_space_protection(&[sponsor, retained])
            .await
            .unwrap();
        assert_eq!(after_recovery.mode, SpaceProtectionMode::Ready);
        assert!(after_recovery.legacy_bootstrap.is_none());
        assert!(after_recovery
            .members
            .iter()
            .all(|member| member.status == MemberProtectionStatus::Protected));
    }

    fn sponsor_fixture_with_stage_persistence(
        persist_stage: bool,
    ) -> (
        DefaultSpaceAccessAdapter,
        Arc<InMemorySession>,
        Arc<MockRevocationRepository>,
        SpaceId,
        TempDir,
        Arc<AtomicUsize>,
    ) {
        let directory = tempdir().unwrap();
        let session = Arc::new(InMemorySession::new());
        let space_id = SpaceId::from("space-group-admission");
        let local_root = MasterKey::from_bytes(&[0x11; 32]).unwrap();
        session.set_master_key_for_space(space_id.clone(), local_root);
        let sponsor_state =
            MlsGroupEngine::create_sponsor(space_id.as_ref().as_bytes(), b"alice").unwrap();
        let material = session
            .create_legacy_bootstrap_material(&space_id, sponsor_state.into_bytes(), 1)
            .unwrap();
        session.install_space_material(&material).unwrap();
        let (repository, _, stage_calls) =
            memory_revocation_repository_with_stage_persistence(Some(material), persist_stage);
        let key_material = local_key_material(&directory, memory_secure_storage());
        (
            adapter(key_material, session.clone(), repository.clone()),
            session,
            repository,
            space_id,
            directory,
            stage_calls,
        )
    }

    fn sponsor_fixture() -> (
        DefaultSpaceAccessAdapter,
        Arc<InMemorySession>,
        Arc<MockRevocationRepository>,
        SpaceId,
        TempDir,
    ) {
        let (adapter, session, repository, space_id, directory, _) =
            sponsor_fixture_with_stage_persistence(true);
        (adapter, session, repository, space_id, directory)
    }

    #[tokio::test]
    async fn current_member_signature_port_uses_persisted_current_group() {
        let (adapter, _session, _repository, _space_id, _directory) = sponsor_fixture();
        let payload = b"member-attestation-transcript";

        let signature = adapter.sign_current_member_payload(payload).await.unwrap();

        assert_eq!(adapter.current_member_epoch().await.unwrap(), 1);
        assert!(adapter
            .verify_current_member_payload(&DeviceId::new("alice"), payload, &signature)
            .await
            .unwrap());
        assert!(!adapter
            .verify_current_member_payload(&DeviceId::new("missing"), payload, &signature)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn revocation_without_space_material_requires_legacy_bootstrap() {
        let directory = tempdir().unwrap();
        let session = Arc::new(InMemorySession::new());
        let space_id = SpaceId::from("legacy-space-without-group-material");
        session.set_master_key_for_space(space_id, MasterKey::from_bytes(&[0x31; 32]).unwrap());
        let (repository, _) = memory_revocation_repository(None);
        let adapter = adapter(
            local_key_material(&directory, memory_secure_storage()),
            session,
            repository,
        );

        let result = adapter
            .revoke_group_member(&DeviceId::new("removed-device"), &[], 100)
            .await
            .unwrap();

        assert_eq!(result, GroupRevocationResult::LocalOnly);
    }

    #[tokio::test]
    async fn revocation_rejects_ready_material_without_group_state_as_corrupted() {
        let directory = tempdir().unwrap();
        let session = Arc::new(InMemorySession::new());
        let space_id = SpaceId::from("ready-space-without-group-state");
        session.set_master_key_for_space(
            space_id.clone(),
            MasterKey::from_bytes(&[0x32; 32]).unwrap(),
        );
        let mut state = SpaceKeyState::legacy(space_id);
        state.mark_migrating().unwrap();
        state
            .mark_ready(ContentKeyId::generate(), ProtectionGroupId::generate())
            .unwrap();
        let material = SpaceKeyMaterial::new(state, Vec::new(), vec![0x01], 100);
        let (repository, _) = memory_revocation_repository(Some(material));
        let adapter = adapter(
            local_key_material(&directory, memory_secure_storage()),
            session,
            repository,
        );

        let error = adapter
            .revoke_group_member(&DeviceId::new("removed-device"), &[], 100)
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            KeyEpochError::Repository(message) if message.contains("corrupted")
        ));
    }

    #[tokio::test]
    async fn first_group_admission_bootstraps_a_single_member_legacy_space() {
        let directory = tempdir().unwrap();
        let session = Arc::new(InMemorySession::new());
        let space_id = SpaceId::from("new-space-before-first-pairing");
        session.set_master_key_for_space(
            space_id.clone(),
            MasterKey::from_bytes(&[0x33; 32]).unwrap(),
        );
        let (repository, _) = memory_revocation_repository(None);
        let adapter = adapter(
            local_key_material(&directory, memory_secure_storage()),
            Arc::clone(&session),
            Arc::clone(&repository),
        );
        let sponsor = DeviceId::new("sponsor-device");
        let joiner = DeviceId::new("joiner-device");
        let pending = adapter.prepare_group_join(&joiner).await.unwrap();

        adapter
            .admit_group_member(&space_id, &sponsor, &joiner, &[], &pending.key_package)
            .await
            .unwrap();

        let material = repository
            .load_space_material(&space_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(material.state().mode(), SpaceSecurityMode::Ready);
        let group = MlsClientState::from_bytes(material.group_state().to_vec());
        assert!(
            MlsGroupEngine::contains_active_member(&group, sponsor.as_str().as_bytes()).unwrap()
        );
        assert!(
            MlsGroupEngine::contains_active_member(&group, joiner.as_str().as_bytes()).unwrap()
        );
    }

    #[tokio::test]
    async fn group_admission_does_not_bootstrap_a_legacy_space_with_existing_members() {
        let directory = tempdir().unwrap();
        let session = Arc::new(InMemorySession::new());
        let space_id = SpaceId::from("legacy-space-with-existing-members");
        session.set_master_key_for_space(
            space_id.clone(),
            MasterKey::from_bytes(&[0x34; 32]).unwrap(),
        );
        let (repository, _) = memory_revocation_repository(None);
        let adapter = adapter(
            local_key_material(&directory, memory_secure_storage()),
            session,
            repository,
        );
        let pending = adapter
            .prepare_group_join(&DeviceId::new("joiner-device"))
            .await
            .unwrap();

        let error = adapter
            .admit_group_member(
                &space_id,
                &DeviceId::new("sponsor-device"),
                &DeviceId::new("joiner-device"),
                &[DeviceId::new("existing-device")],
                &pending.key_package,
            )
            .await
            .unwrap_err();

        assert!(matches!(error, SpaceAccessError::CorruptedKeyMaterial));
    }

    #[tokio::test]
    async fn activate_session_without_material_keeps_legacy_key_state() {
        let directory = tempdir().unwrap();
        let session = Arc::new(InMemorySession::new());
        let space_id = SpaceId::from("legacy-space");
        let mut repository = MockRevocationRepository::new();
        repository
            .expect_load_space_material()
            .times(1)
            .returning(|_| Ok(None));
        repository.expect_save_space_material().never();
        let adapter = adapter(
            local_key_material(&directory, memory_secure_storage()),
            Arc::clone(&session),
            Arc::new(repository),
        );

        adapter
            .activate_session(&space_id, MasterKey::from_bytes(&[0x11; 32]).unwrap())
            .await
            .unwrap();

        let current = session
            .current_content_key(&space_id, ContentKeyPurpose::Content)
            .unwrap();
        assert_eq!(current.content_key_id(), &ContentKeyId::legacy_v1());
        assert_eq!(current.epoch(), GroupEpoch::new(0));
    }

    #[test]
    fn admission_offer_contains_kdf_parameters_but_not_wrapped_content_key() {
        let scope = KeyScope {
            profile_id: "profile-a".to_string(),
        };
        let slot = KeySlot::draft_v1(scope)
            .unwrap()
            .finalize(WrappedMasterKey {
                blob: v1_aead::encrypt_blob_xchacha(
                    &MasterKey::from_bytes(&[7u8; 32]).unwrap(),
                    &[9u8; 32],
                    b"wrapped-master-key",
                )
                .unwrap(),
            });

        let encoded = serialize_admission_kdf_offer(&slot).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&encoded).unwrap();

        assert_eq!(value["version"], "V1");
        assert!(value.get("kdf").is_some());
        assert!(value.get("salt").is_some());
        assert!(value.get("wrapped_master_key").is_none());
    }

    #[test]
    fn admission_proof_key_is_bound_to_invitation_session_and_space() {
        let kek = Kek::from_bytes(&[3u8; 32]).unwrap();
        let invitation = InvitationCode::new("invite-a");
        let session = SessionId::new("session-a".to_string());
        let space = SpaceId::from("space-a");

        let base = derive_admission_proof_key(&kek, &invitation, &session, &space).unwrap();
        let other_invitation =
            derive_admission_proof_key(&kek, &InvitationCode::new("invite-b"), &session, &space)
                .unwrap();
        let other_session = derive_admission_proof_key(
            &kek,
            &invitation,
            &SessionId::new("session-b".to_string()),
            &space,
        )
        .unwrap();
        let other_space =
            derive_admission_proof_key(&kek, &invitation, &session, &SpaceId::from("space-b"))
                .unwrap();

        assert_ne!(base.as_bytes(), kek.as_bytes());
        assert_ne!(base.as_bytes(), other_invitation.as_bytes());
        assert_ne!(base.as_bytes(), other_session.as_bytes());
        assert_ne!(base.as_bytes(), other_space.as_bytes());
    }

    #[tokio::test]
    async fn group_join_uses_distinct_local_root_and_cold_restores_shared_catalog() {
        let (sponsor, sponsor_session, _, space_id, _sponsor_dir) = sponsor_fixture();
        let joiner_dir = tempdir().unwrap();
        let joiner_storage = memory_secure_storage();
        let joiner_key_material = local_key_material(&joiner_dir, joiner_storage);
        let joiner_session = Arc::new(InMemorySession::new());
        let (joiner_repository, _) = memory_revocation_repository(None);
        let joiner = adapter(
            joiner_key_material.clone(),
            joiner_session.clone(),
            joiner_repository.clone(),
        );
        let pending = joiner
            .prepare_group_join(&DeviceId::new("joiner-device"))
            .await
            .unwrap();
        let admission = sponsor
            .admit_group_member(
                &space_id,
                &DeviceId::new("alice"),
                &DeviceId::new("joiner-device"),
                &[],
                &pending.key_package,
            )
            .await
            .unwrap();
        joiner
            .install_group_join(
                &space_id,
                &Passphrase::new("correct horse battery staple"),
                pending,
                &admission.welcome,
                &admission.encrypted_key_catalog,
                admission.group_epoch,
            )
            .await
            .unwrap();

        assert_ne!(
            sponsor_session.get_master_key().unwrap(),
            joiner_session.get_master_key().unwrap()
        );
        assert_eq!(
            sponsor_session.legacy_content_key().unwrap(),
            joiner_session.legacy_content_key().unwrap()
        );
        let sponsor_current = sponsor_session
            .current_content_key(&space_id, ContentKeyPurpose::Content)
            .unwrap();
        let joiner_current = joiner_session
            .current_content_key(&space_id, ContentKeyPurpose::Content)
            .unwrap();
        assert_eq!(sponsor_current.epoch(), GroupEpoch::new(2));
        assert_eq!(sponsor_current.key(), joiner_current.key());

        let restored_session = Arc::new(InMemorySession::new());
        let restored = adapter(
            joiner_key_material,
            restored_session.clone(),
            joiner_repository,
        );
        SpaceAccessStore::unlock(
            &restored,
            &space_id,
            &Passphrase::new("correct horse battery staple"),
        )
        .await
        .unwrap();
        let restored_current = restored_session
            .current_content_key(&space_id, ContentKeyPurpose::Content)
            .unwrap();
        assert_eq!(restored_current.key(), sponsor_current.key());
    }

    #[tokio::test]
    async fn failed_group_install_restores_previous_local_state() {
        let (sponsor, _, _, target_space, _sponsor_dir) = sponsor_fixture();
        let joiner_dir = tempdir().unwrap();
        let joiner_storage = memory_secure_storage();
        let joiner_key_material = local_key_material(&joiner_dir, joiner_storage);
        let joiner_session = Arc::new(InMemorySession::new());
        let (joiner_repository, fail_saves) = memory_revocation_repository(None);
        let joiner = adapter(
            joiner_key_material.clone(),
            joiner_session.clone(),
            joiner_repository.clone(),
        );
        let old_space = SpaceId::from("old-space");
        SpaceAccessStore::initialize(
            &joiner,
            &old_space,
            &Passphrase::new("old passphrase remains valid"),
        )
        .await
        .unwrap();
        let old_root = joiner_session.get_master_key().unwrap();
        let old_slot = joiner_key_material
            .load_keyslot(&KeyScope {
                profile_id: "default".into(),
            })
            .await
            .unwrap();

        let pending = joiner
            .prepare_group_join(&DeviceId::new("joiner-device"))
            .await
            .unwrap();
        let admission = sponsor
            .admit_group_member(
                &target_space,
                &DeviceId::new("alice"),
                &DeviceId::new("joiner-device"),
                &[],
                &pending.key_package,
            )
            .await
            .unwrap();
        fail_saves.store(true, Ordering::Release);
        assert!(joiner
            .install_group_join(
                &target_space,
                &Passphrase::new("new passphrase is not committed"),
                pending,
                &admission.welcome,
                &admission.encrypted_key_catalog,
                admission.group_epoch,
            )
            .await
            .is_err());

        assert_eq!(joiner_session.current_space_id().unwrap(), old_space);
        assert_eq!(joiner_session.get_master_key().unwrap(), old_root);
        assert_eq!(
            joiner_key_material
                .load_keyslot(&KeyScope {
                    profile_id: "default".into(),
                })
                .await
                .unwrap(),
            old_slot
        );
    }

    #[tokio::test]
    async fn reliable_revocation_activates_a_new_epoch_for_retained_members_only() {
        let (sponsor, sponsor_session, repository, space_id, _sponsor_dir) = sponsor_fixture();
        let bob = sponsor
            .prepare_group_join(&DeviceId::new("bob"))
            .await
            .unwrap();
        sponsor
            .admit_group_member(
                &space_id,
                &DeviceId::new("alice"),
                &DeviceId::new("bob"),
                &[],
                &bob.key_package,
            )
            .await
            .unwrap();
        let charlie = sponsor
            .prepare_group_join(&DeviceId::new("charlie"))
            .await
            .unwrap();
        sponsor
            .admit_group_member(
                &space_id,
                &DeviceId::new("alice"),
                &DeviceId::new("charlie"),
                &[DeviceId::new("bob")],
                &charlie.key_package,
            )
            .await
            .unwrap();
        let before = sponsor_session
            .current_content_key(&space_id, ContentKeyPurpose::Content)
            .unwrap();

        let result = sponsor
            .revoke_group_member(&DeviceId::new("charlie"), &[DeviceId::new("bob")], 100)
            .await
            .unwrap();

        assert_eq!(result.status(), Some(RevocationStatus::Distributing));
        assert_eq!(result.pending_recipients(), 1);
        let after = sponsor_session
            .current_content_key(&space_id, ContentKeyPurpose::Content)
            .unwrap();
        assert_eq!(after.epoch(), before.epoch().next().unwrap());
        assert_ne!(after.key(), before.key());
        let stage = repository
            .load_staged_revocation(result.revocation_id().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stage.outbox().len(), 1);
        assert_eq!(stage.outbox()[0].recipient(), &DeviceId::new("bob"));
    }

    #[tokio::test]
    async fn reliable_revocation_stops_when_repository_state_does_not_advance() {
        let (sponsor, _session, _repository, space_id, _directory, stage_calls) =
            sponsor_fixture_with_stage_persistence(false);
        let charlie = sponsor
            .prepare_group_join(&DeviceId::new("charlie"))
            .await
            .unwrap();
        sponsor
            .admit_group_member(
                &space_id,
                &DeviceId::new("alice"),
                &DeviceId::new("charlie"),
                &[],
                &charlie.key_package,
            )
            .await
            .unwrap();

        let error = sponsor
            .revoke_group_member(&DeviceId::new("charlie"), &[], 100)
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            KeyEpochError::Repository(message) if message.contains("recovery required")
        ));
        assert_eq!(stage_calls.load(Ordering::Acquire), 3);
    }

    #[tokio::test]
    async fn retained_device_applies_admission_then_revocation_epoch_updates() {
        let (sponsor, sponsor_session, repository, space_id, _sponsor_dir) = sponsor_fixture();
        let bob_dir = tempdir().unwrap();
        let bob_session = Arc::new(InMemorySession::new());
        let (bob_repository, _) = memory_revocation_repository(None);
        let bob = adapter(
            local_key_material(&bob_dir, memory_secure_storage()),
            bob_session.clone(),
            bob_repository.clone(),
        );
        let bob_pending = bob.prepare_group_join(&DeviceId::new("bob")).await.unwrap();
        let bob_admission = sponsor
            .admit_group_member(
                &space_id,
                &DeviceId::new("alice"),
                &DeviceId::new("bob"),
                &[],
                &bob_pending.key_package,
            )
            .await
            .unwrap();
        bob.install_group_join(
            &space_id,
            &Passphrase::new("shared passphrase for bob"),
            bob_pending,
            &bob_admission.welcome,
            &bob_admission.encrypted_key_catalog,
            bob_admission.group_epoch,
        )
        .await
        .unwrap();
        let bob_outbound_update =
            PendingGroupUpdate::persistent(DeviceId::new("dave"), b"bob-pending-update".to_vec());
        let mut bob_material = bob_repository
            .load_space_material(&space_id)
            .await
            .unwrap()
            .unwrap();
        bob_material.add_pending_group_updates([bob_outbound_update.clone()], 150);
        bob_repository
            .save_space_material(&bob_material)
            .await
            .unwrap();

        let charlie_pending = sponsor
            .prepare_group_join(&DeviceId::new("charlie"))
            .await
            .unwrap();
        let charlie_admission = sponsor
            .admit_group_member(
                &space_id,
                &DeviceId::new("alice"),
                &DeviceId::new("charlie"),
                &[DeviceId::new("bob")],
                &charlie_pending.key_package,
            )
            .await
            .unwrap();
        let admission_update = charlie_admission.existing_member_updates[0].payload();
        bob.apply_group_epoch_update(admission_update)
            .await
            .unwrap();
        assert_eq!(
            bob.apply_group_epoch_update(admission_update)
                .await
                .unwrap(),
            GroupEpoch::new(charlie_admission.group_epoch)
        );
        assert_eq!(
            bob_repository
                .load_space_material(&space_id)
                .await
                .unwrap()
                .unwrap()
                .pending_group_updates(),
            &[bob_outbound_update]
        );

        let result = sponsor
            .revoke_group_member(&DeviceId::new("charlie"), &[DeviceId::new("bob")], 200)
            .await
            .unwrap();
        let stage = repository
            .load_staged_revocation(result.revocation_id().unwrap())
            .await
            .unwrap()
            .unwrap();
        bob.apply_group_epoch_update(stage.outbox()[0].payload())
            .await
            .unwrap();

        let sponsor_key = sponsor_session
            .current_content_key(&space_id, ContentKeyPurpose::Content)
            .unwrap();
        let bob_key = bob_session
            .current_content_key(&space_id, ContentKeyPurpose::Content)
            .unwrap();
        assert_eq!(bob_key.epoch(), sponsor_key.epoch());
        assert_eq!(bob_key.key(), sponsor_key.key());
    }

    #[tokio::test]
    async fn joiner_relays_admission_update_after_sponsor_stops() {
        let (sponsor_b, _sponsor_session, _repository, space_id, _sponsor_dir) = sponsor_fixture();

        let device_a_dir = tempdir().unwrap();
        let device_a_session = Arc::new(InMemorySession::new());
        let (device_a_repository, _) = memory_revocation_repository(None);
        let device_a = adapter(
            local_key_material(&device_a_dir, memory_secure_storage()),
            Arc::clone(&device_a_session),
            Arc::clone(&device_a_repository),
        );
        let device_a_id = DeviceId::new("device-a");
        let device_a_pending = sponsor_b.prepare_group_join(&device_a_id).await.unwrap();
        let device_a_admission = sponsor_b
            .admit_group_member(
                &space_id,
                &DeviceId::new("alice"),
                &device_a_id,
                &[],
                &device_a_pending.key_package,
            )
            .await
            .unwrap();
        device_a
            .install_group_join(
                &space_id,
                &Passphrase::new("shared passphrase for device a"),
                device_a_pending,
                &device_a_admission.welcome,
                &device_a_admission.encrypted_key_catalog,
                device_a_admission.group_epoch,
            )
            .await
            .unwrap();

        let device_c_dir = tempdir().unwrap();
        let device_c_session = Arc::new(InMemorySession::new());
        let (device_c_repository, _) = memory_revocation_repository(None);
        let device_c = adapter(
            local_key_material(&device_c_dir, memory_secure_storage()),
            device_c_session,
            device_c_repository,
        );
        let device_c_id = DeviceId::new("device-c");
        let device_c_pending = device_c.prepare_group_join(&device_c_id).await.unwrap();
        let device_c_admission = sponsor_b
            .admit_group_member(
                &space_id,
                &DeviceId::new("alice"),
                &device_c_id,
                std::slice::from_ref(&device_a_id),
                &device_c_pending.key_package,
            )
            .await
            .unwrap();
        device_c
            .install_group_join(
                &space_id,
                &Passphrase::new("shared passphrase for device c"),
                device_c_pending,
                &device_c_admission.welcome,
                &device_c_admission.encrypted_key_catalog,
                device_c_admission.group_epoch,
            )
            .await
            .unwrap();
        let relayed_update = device_c_admission.existing_member_updates[0]
            .payload()
            .to_vec();

        drop(sponsor_b);

        assert_eq!(
            device_a
                .apply_group_epoch_update(&relayed_update)
                .await
                .unwrap(),
            GroupEpoch::new(device_c_admission.group_epoch)
        );
        let payload = b"member-attestation-after-relayed-update";
        let signature = device_c.sign_current_member_payload(payload).await.unwrap();
        assert!(device_a
            .verify_current_member_payload(&device_c_id, payload, &signature)
            .await
            .unwrap());
    }
}
