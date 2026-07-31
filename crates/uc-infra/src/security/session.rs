//! 进程内会话存储——`SpaceAccessAdapter` / `BlobCipherAdapter` /
//! `TransferCipherAdapter` / `EncryptedBlobStore` 共享同一份 `Arc<InMemorySession>`。
//!
//! 历史上这是 `InMemoryEncryptionSessionPort`（uc-platform 的 trait 实现);
//! Slice 3 - C8 把 `EncryptionSessionPort` trait 删除后,这个类型下沉到
//! uc-infra 作为具体类型——所有 uc-infra 内部 adapter 共用同一个 Arc,
//! 不再走 dyn trait 间接层。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use tracing::{debug, debug_span};
use uc_core::crypto::model::EncryptionError;
use uc_core::ids::SpaceId;
use uc_core::membership::{
    ContentKeyId, ContentKeyPurpose, GroupEpoch, LegacyUpgradeId, ProtectionGroupId,
    SpaceKeyMaterial, SpaceSecurityMode,
};

use super::secrets::MasterKey;

#[derive(Clone, Debug)]
struct State {
    master_key: Option<MasterKey>,
    space_id: Option<SpaceId>,
    current_content_key_id: Option<ContentKeyId>,
    current_epoch: Option<GroupEpoch>,
    content_keys: HashMap<ContentKeyId, ContentKeyEntry>,
}

#[derive(Clone, Debug)]
struct ContentKeyEntry {
    epoch: GroupEpoch,
    key: MasterKey,
}

#[derive(Serialize, Deserialize)]
struct PersistedContentKeyCatalog {
    version: u8,
    entries: Vec<PersistedContentKeyEntry>,
}

#[derive(Serialize, Deserialize)]
struct PersistedContentKeyEntry {
    content_key_id: String,
    epoch: u64,
    key: Vec<u8>,
}

pub(crate) struct ResolvedContentKey {
    content_key_id: ContentKeyId,
    epoch: GroupEpoch,
    key: MasterKey,
}

impl ResolvedContentKey {
    pub(crate) fn content_key_id(&self) -> &ContentKeyId {
        &self.content_key_id
    }

    pub(crate) const fn epoch(&self) -> GroupEpoch {
        self.epoch
    }

    pub(crate) fn key(&self) -> &MasterKey {
        &self.key
    }
}

/// In-memory master-key 容器,线程安全。
///
/// `MasterKey` 派生 `ZeroizeOnDrop`(见 `super::secrets`),所以
/// `set_master_key` 替换旧值、`clear` 把 `Option` 置空、整个 `InMemorySession`
/// 被 drop 等路径都会就地把 32 字节密钥清零——会话生命周期结束后,残留密钥
/// 物料就不会停留在堆/栈/swap 页面里。
#[derive(Clone)]
pub struct InMemorySession {
    state: Arc<Mutex<State>>,
}

pub(crate) struct SessionSnapshot(State);

impl InMemorySession {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                master_key: None,
                space_id: None,
                current_content_key_id: None,
                current_epoch: None,
                content_keys: HashMap::new(),
            })),
        }
    }

    pub fn is_ready(&self) -> bool {
        match self.state.lock() {
            Ok(state) => state.master_key.is_some(),
            Err(poisoned) => poisoned.into_inner().master_key.is_some(),
        }
    }

    pub fn get_master_key(&self) -> Result<MasterKey, EncryptionError> {
        self.lock_state()
            .master_key
            .as_ref()
            .cloned()
            .ok_or(EncryptionError::NotInitialized)
    }

    pub fn set_master_key(&self, master_key: MasterKey) {
        let span = debug_span!("infra.session.set_master_key");
        span.in_scope(|| {
            let mut state = self.lock_state();
            state.master_key = Some(master_key);
            state.space_id = None;
            state.current_content_key_id = None;
            state.current_epoch = None;
            state.content_keys.clear();
            debug!("master key set");
        });
    }

    pub(crate) fn set_master_key_for_space(&self, space_id: SpaceId, master_key: MasterKey) {
        let mut state = self.lock_state();
        state.master_key = Some(master_key.clone());
        state.space_id = Some(space_id);
        state.current_content_key_id = Some(ContentKeyId::legacy_v1());
        state.current_epoch = Some(GroupEpoch::new(0));
        state.content_keys.clear();
        state.content_keys.insert(
            ContentKeyId::legacy_v1(),
            ContentKeyEntry {
                epoch: GroupEpoch::new(0),
                key: master_key,
            },
        );
    }

    fn create_ready_space_material(
        &self,
        space_id: &SpaceId,
        protection_group_id: Option<ProtectionGroupId>,
        group_state: Vec<u8>,
        updated_at_ms: i64,
    ) -> Result<SpaceKeyMaterial, EncryptionError> {
        let state = self.lock_state();
        if state.master_key.is_none() || state.space_id.as_ref() != Some(space_id) {
            return Err(EncryptionError::NotInitialized);
        }
        let legacy_key = state
            .content_keys
            .get(&ContentKeyId::legacy_v1())
            .map(|entry| entry.key.clone())
            .ok_or(EncryptionError::KeyNotFound)?;
        drop(state);

        let content_key_id = ContentKeyId::generate();
        let content_key = MasterKey::generate()?;
        let catalog = PersistedContentKeyCatalog {
            version: 2,
            entries: vec![
                PersistedContentKeyEntry {
                    content_key_id: ContentKeyId::legacy_v1().as_str().to_owned(),
                    epoch: 0,
                    key: legacy_key.as_bytes().to_vec(),
                },
                PersistedContentKeyEntry {
                    content_key_id: content_key_id.as_str().to_owned(),
                    epoch: 1,
                    key: content_key.as_bytes().to_vec(),
                },
            ],
        };
        let key_catalog =
            serde_json::to_vec(&catalog).map_err(|_| EncryptionError::KeyMaterialCorrupt)?;
        let mut key_state = uc_core::membership::SpaceKeyState::legacy(space_id.clone());
        key_state
            .mark_migrating()
            .map_err(|_| EncryptionError::KeyMaterialCorrupt)?;
        key_state
            .mark_ready(
                content_key_id,
                protection_group_id.unwrap_or_else(ProtectionGroupId::generate),
            )
            .map_err(|_| EncryptionError::KeyMaterialCorrupt)?;
        Ok(SpaceKeyMaterial::new(
            key_state,
            group_state,
            key_catalog,
            updated_at_ms,
        ))
    }

    pub(crate) fn create_legacy_bootstrap_material(
        &self,
        space_id: &SpaceId,
        group_state: Vec<u8>,
        updated_at_ms: i64,
    ) -> Result<SpaceKeyMaterial, EncryptionError> {
        if group_state.is_empty() {
            return Err(EncryptionError::KeyMaterialCorrupt);
        }
        self.create_ready_space_material(space_id, None, group_state, updated_at_ms)
    }

    pub(crate) fn create_legacy_bootstrap_material_in_group(
        &self,
        space_id: &SpaceId,
        protection_group_id: ProtectionGroupId,
        group_state: Vec<u8>,
        updated_at_ms: i64,
    ) -> Result<SpaceKeyMaterial, EncryptionError> {
        if group_state.is_empty() {
            return Err(EncryptionError::KeyMaterialCorrupt);
        }
        self.create_ready_space_material(
            space_id,
            Some(protection_group_id),
            group_state,
            updated_at_ms,
        )
    }

    #[cfg(test)]
    pub(crate) fn create_migrated_space_material(
        &self,
        space_id: &SpaceId,
        updated_at_ms: i64,
    ) -> Result<SpaceKeyMaterial, EncryptionError> {
        self.create_ready_space_material(
            space_id,
            None,
            b"test-only-group-state".to_vec(),
            updated_at_ms,
        )
    }

    pub(crate) fn install_space_material(
        &self,
        material: &SpaceKeyMaterial,
    ) -> Result<(), EncryptionError> {
        if material.state().mode() != SpaceSecurityMode::Ready || material.group_state().is_empty()
        {
            return Err(EncryptionError::KeyMaterialCorrupt);
        }
        let catalog: PersistedContentKeyCatalog = serde_json::from_slice(material.key_catalog())
            .map_err(|_| EncryptionError::KeyMaterialCorrupt)?;
        if catalog.version != 1 && catalog.version != 2 {
            return Err(EncryptionError::UnsupportedVersion);
        }

        let mut state = self.lock_state();
        if state.master_key.is_none()
            || state.space_id.as_ref() != Some(material.state().space_id())
        {
            return Err(EncryptionError::NotInitialized);
        }
        let mut keys = HashMap::new();
        if catalog.version == 1 {
            let legacy_key = state
                .master_key
                .as_ref()
                .cloned()
                .ok_or(EncryptionError::NotInitialized)?;
            keys.insert(
                ContentKeyId::legacy_v1(),
                ContentKeyEntry {
                    epoch: GroupEpoch::new(0),
                    key: legacy_key,
                },
            );
        }
        for persisted in catalog.entries {
            let content_key_id = ContentKeyId::from_string(persisted.content_key_id)
                .map_err(|_| EncryptionError::KeyMaterialCorrupt)?;
            if keys.contains_key(&content_key_id)
                || (content_key_id == ContentKeyId::legacy_v1()
                    && (catalog.version != 2 || persisted.epoch != 0))
            {
                return Err(EncryptionError::KeyMaterialCorrupt);
            }
            let key = MasterKey::from_bytes(&persisted.key)
                .map_err(|_| EncryptionError::KeyMaterialCorrupt)?;
            keys.insert(
                content_key_id,
                ContentKeyEntry {
                    epoch: GroupEpoch::new(persisted.epoch),
                    key,
                },
            );
        }
        if !keys.contains_key(&ContentKeyId::legacy_v1()) {
            return Err(EncryptionError::KeyMaterialCorrupt);
        }
        let current_id = material.state().current_content_key_id();
        let current = keys
            .get(current_id)
            .ok_or(EncryptionError::KeyMaterialCorrupt)?;
        if current.epoch != material.state().epoch() {
            return Err(EncryptionError::KeyMaterialCorrupt);
        }
        state.content_keys = keys;
        state.current_content_key_id = Some(current_id.clone());
        state.current_epoch = Some(material.state().epoch());
        Ok(())
    }

    pub(crate) fn merge_space_material_history(
        &self,
        previous: &SpaceKeyMaterial,
        incoming: SpaceKeyMaterial,
    ) -> Result<SpaceKeyMaterial, EncryptionError> {
        let previous_catalog: PersistedContentKeyCatalog =
            serde_json::from_slice(previous.key_catalog())
                .map_err(|_| EncryptionError::KeyMaterialCorrupt)?;
        let mut incoming_catalog: PersistedContentKeyCatalog =
            serde_json::from_slice(incoming.key_catalog())
                .map_err(|_| EncryptionError::KeyMaterialCorrupt)?;
        if previous_catalog.version != 2 || incoming_catalog.version != 2 {
            return Err(EncryptionError::UnsupportedVersion);
        }
        for entry in previous_catalog.entries {
            match incoming_catalog
                .entries
                .iter()
                .find(|candidate| candidate.content_key_id == entry.content_key_id)
            {
                Some(candidate) if candidate.epoch != entry.epoch || candidate.key != entry.key => {
                    return Err(EncryptionError::KeyMaterialCorrupt);
                }
                Some(_) => {}
                None => incoming_catalog.entries.push(entry),
            }
        }
        let key_catalog = serde_json::to_vec(&incoming_catalog)
            .map_err(|_| EncryptionError::KeyMaterialCorrupt)?;
        Ok(SpaceKeyMaterial::new(
            incoming.state().clone(),
            incoming.group_state().to_vec(),
            key_catalog,
            incoming.updated_at_ms(),
        ))
    }

    pub(crate) fn rotate_space_material(
        &self,
        material: &SpaceKeyMaterial,
        group_state: Vec<u8>,
        expected_epoch: GroupEpoch,
        updated_at_ms: i64,
    ) -> Result<SpaceKeyMaterial, EncryptionError> {
        let mut catalog: PersistedContentKeyCatalog =
            serde_json::from_slice(material.key_catalog())
                .map_err(|_| EncryptionError::KeyMaterialCorrupt)?;
        if catalog.version != 2 || material.state().mode() != SpaceSecurityMode::Ready {
            return Err(EncryptionError::KeyMaterialCorrupt);
        }
        let content_key_id = ContentKeyId::generate();
        let content_key = MasterKey::generate()?;
        let mut state = material.state().clone();
        state
            .rotate(content_key_id.clone())
            .map_err(|_| EncryptionError::KeyMaterialCorrupt)?;
        if state.epoch() != expected_epoch {
            return Err(EncryptionError::KeyMaterialCorrupt);
        }
        catalog.entries.push(PersistedContentKeyEntry {
            content_key_id: content_key_id.as_str().to_owned(),
            epoch: expected_epoch.value(),
            key: content_key.as_bytes().to_vec(),
        });
        let key_catalog =
            serde_json::to_vec(&catalog).map_err(|_| EncryptionError::KeyMaterialCorrupt)?;
        Ok(
            SpaceKeyMaterial::new(state, group_state, key_catalog, updated_at_ms)
                .with_pending_group_updates_from(material),
        )
    }

    pub(crate) fn snapshot(&self) -> SessionSnapshot {
        SessionSnapshot(self.lock_state().clone())
    }

    pub(crate) fn restore(&self, snapshot: SessionSnapshot) {
        *self.lock_state() = snapshot.0;
    }

    pub(crate) fn current_content_key(
        &self,
        space_id: &SpaceId,
        purpose: ContentKeyPurpose,
    ) -> Result<ResolvedContentKey, EncryptionError> {
        let state = self.lock_state();
        if state.space_id.as_ref() != Some(space_id) {
            return Err(EncryptionError::NotInitialized);
        }
        let content_key_id = state
            .current_content_key_id
            .as_ref()
            .ok_or(EncryptionError::NotInitialized)?;
        Self::resolve_from_state(&state, content_key_id, purpose)
    }

    pub(crate) fn current_space_id(&self) -> Result<SpaceId, EncryptionError> {
        self.lock_state()
            .space_id
            .clone()
            .ok_or(EncryptionError::NotInitialized)
    }

    pub(crate) fn legacy_content_key(&self) -> Result<MasterKey, EncryptionError> {
        self.lock_state()
            .content_keys
            .get(&ContentKeyId::legacy_v1())
            .map(|entry| entry.key.clone())
            .ok_or(EncryptionError::KeyNotFound)
    }

    pub(crate) fn content_key(
        &self,
        space_id: &SpaceId,
        content_key_id: &ContentKeyId,
        purpose: ContentKeyPurpose,
    ) -> Result<ResolvedContentKey, EncryptionError> {
        let state = self.lock_state();
        if state.space_id.as_ref() != Some(space_id) {
            return Err(EncryptionError::NotInitialized);
        }
        Self::resolve_from_state(&state, content_key_id, purpose)
    }

    pub(crate) fn derive_stable_subkey(
        &self,
        salt: &[u8],
        info: &[u8],
    ) -> Result<[u8; 32], EncryptionError> {
        let state = self.lock_state();
        let legacy_key = state
            .content_keys
            .get(&ContentKeyId::legacy_v1())
            .ok_or(EncryptionError::KeyNotFound)?;
        let hkdf = Hkdf::<Sha256>::new(Some(salt), legacy_key.key.as_bytes());
        let mut output = [0u8; 32];
        hkdf.expand(info, &mut output)
            .map_err(|_| EncryptionError::CryptoFailure)?;
        Ok(output)
    }

    pub(crate) fn legacy_upgrade_id(&self) -> Result<LegacyUpgradeId, EncryptionError> {
        let secret = self.derive_legacy_upgrade_secret(b"uniclipboard-legacy-upgrade-id/v1")?;
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&secret).map_err(|_| EncryptionError::CryptoFailure)?;
        mac.update(b"legacy-space-membership");
        Ok(LegacyUpgradeId::from_bytes(
            mac.finalize().into_bytes().into(),
        ))
    }

    pub(crate) fn legacy_upgrade_proof(
        &self,
        transcript: &[u8],
    ) -> Result<[u8; 32], EncryptionError> {
        let secret = self.derive_legacy_upgrade_secret(b"uniclipboard-legacy-upgrade-proof/v1")?;
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&secret).map_err(|_| EncryptionError::CryptoFailure)?;
        mac.update(transcript);
        Ok(mac.finalize().into_bytes().into())
    }

    pub(crate) fn verify_legacy_upgrade_proof(
        &self,
        transcript: &[u8],
        proof: &[u8],
    ) -> Result<bool, EncryptionError> {
        let secret = self.derive_legacy_upgrade_secret(b"uniclipboard-legacy-upgrade-proof/v1")?;
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&secret).map_err(|_| EncryptionError::CryptoFailure)?;
        mac.update(transcript);
        Ok(mac.verify_slice(proof).is_ok())
    }

    fn derive_legacy_upgrade_secret(&self, info: &[u8]) -> Result<[u8; 32], EncryptionError> {
        let state = self.lock_state();
        if state.space_id.is_none() {
            return Err(EncryptionError::NotInitialized);
        }
        let legacy_key = state
            .content_keys
            .get(&ContentKeyId::legacy_v1())
            .ok_or(EncryptionError::KeyNotFound)?;
        let hkdf = Hkdf::<Sha256>::new(
            Some(b"uniclipboard-legacy-upgrade/v1"),
            legacy_key.key.as_bytes(),
        );
        let mut output = [0u8; 32];
        hkdf.expand(info, &mut output)
            .map_err(|_| EncryptionError::CryptoFailure)?;
        Ok(output)
    }

    fn resolve_from_state(
        state: &State,
        content_key_id: &ContentKeyId,
        purpose: ContentKeyPurpose,
    ) -> Result<ResolvedContentKey, EncryptionError> {
        let entry = state
            .content_keys
            .get(content_key_id)
            .ok_or(EncryptionError::KeyNotFound)?;
        let space_id = state
            .space_id
            .as_ref()
            .ok_or(EncryptionError::NotInitialized)?;
        let hkdf = Hkdf::<Sha256>::new(Some(space_id.as_ref().as_bytes()), entry.key.as_bytes());
        let mut output = [0u8; MasterKey::LEN];
        let info = format!("uniclipboard-content-key/v1/{}", purpose.as_str());
        hkdf.expand(info.as_bytes(), &mut output)
            .map_err(|_| EncryptionError::CryptoFailure)?;
        Ok(ResolvedContentKey {
            content_key_id: content_key_id.clone(),
            epoch: entry.epoch,
            key: MasterKey::from_bytes(&output)?,
        })
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, State> {
        match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    pub fn clear(&self) {
        let span = debug_span!("infra.session.clear");
        span.in_scope(|| {
            let mut state = self.lock_state();
            state.master_key = None;
            state.space_id = None;
            state.current_content_key_id = None;
            state.current_epoch = None;
            state.content_keys.clear();
            debug!("master key cleared");
        });
    }
}

impl Default for InMemorySession {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use uc_core::ids::SpaceId;
    use uc_core::membership::{ContentKeyId, ContentKeyPurpose, GroupEpoch, ProtectionGroupId};

    use super::*;

    fn key(seed: u8) -> MasterKey {
        MasterKey::from_bytes(&[seed; 32]).unwrap()
    }

    #[test]
    fn legacy_key_is_registered_and_purpose_keys_are_isolated() {
        let session = InMemorySession::new();
        let space_id = SpaceId::from_str("space-a");
        session.set_master_key_for_space(space_id.clone(), key(1));

        let content = session
            .current_content_key(&space_id, ContentKeyPurpose::Content)
            .unwrap();
        let transport = session
            .current_content_key(&space_id, ContentKeyPurpose::Transport)
            .unwrap();

        assert_eq!(content.content_key_id(), &ContentKeyId::legacy_v1());
        assert_eq!(content.epoch(), GroupEpoch::new(0));
        assert_ne!(content.key().as_bytes(), transport.key().as_bytes());
    }

    #[test]
    fn migrated_catalog_survives_a_fresh_session() {
        let space_id = SpaceId::from_str("space-a");
        let first = InMemorySession::new();
        first.set_master_key_for_space(space_id.clone(), key(1));
        let material = first
            .create_migrated_space_material(&space_id, 100)
            .unwrap();
        first.install_space_material(&material).unwrap();
        let before = first
            .current_content_key(&space_id, ContentKeyPurpose::Content)
            .unwrap();

        let reopened = InMemorySession::new();
        reopened.set_master_key_for_space(space_id.clone(), key(1));
        reopened.install_space_material(&material).unwrap();
        let after = reopened
            .current_content_key(&space_id, ContentKeyPurpose::Content)
            .unwrap();

        assert_eq!(before.content_key_id(), after.content_key_id());
        assert_eq!(before.epoch(), GroupEpoch::new(1));
        assert_eq!(before.key().as_bytes(), after.key().as_bytes());
        assert_ne!(before.content_key_id(), &ContentKeyId::legacy_v1());
    }

    #[test]
    fn empty_group_state_cannot_be_installed_as_ready_material() {
        let space_id = SpaceId::from_str("space-a");
        let session = InMemorySession::new();
        session.set_master_key_for_space(space_id.clone(), key(1));
        let material = session
            .create_migrated_space_material(&space_id, 100)
            .unwrap();
        let malformed = SpaceKeyMaterial::new(
            material.state().clone(),
            Vec::new(),
            material.key_catalog().to_vec(),
            100,
        );

        assert!(session.install_space_material(&malformed).is_err());
    }

    #[test]
    fn missing_key_and_wrong_space_fail_without_legacy_fallback() {
        let session = InMemorySession::new();
        let space_id = SpaceId::from_str("space-a");
        session.set_master_key_for_space(space_id.clone(), key(1));

        assert!(session
            .content_key(
                &SpaceId::from_str("space-b"),
                &ContentKeyId::legacy_v1(),
                ContentKeyPurpose::Content,
            )
            .is_err());
        assert!(session
            .content_key(
                &space_id,
                &ContentKeyId::from_string("missing").unwrap(),
                ContentKeyPurpose::Content,
            )
            .is_err());
    }

    #[test]
    fn stable_subkey_matches_the_legacy_master_key_derivation() {
        let root = key(3);
        let session = InMemorySession::new();
        session.set_master_key_for_space(SpaceId::from_str("space-a"), root.clone());
        let mut expected = [0u8; 32];
        Hkdf::<Sha256>::new(Some(b"profile-a"), root.as_bytes())
            .expand(b"uniclipboard-search-index/v1", &mut expected)
            .unwrap();

        assert_eq!(
            session
                .derive_stable_subkey(b"profile-a", b"uniclipboard-search-index/v1")
                .unwrap(),
            expected
        );
    }

    #[test]
    fn legacy_upgrade_id_is_stable_for_the_same_legacy_space_key() {
        let first = InMemorySession::new();
        first.set_master_key_for_space(SpaceId::from_str("space-a"), key(7));
        let second = InMemorySession::new();
        second.set_master_key_for_space(SpaceId::from_str("space-a"), key(7));

        assert_eq!(
            first.legacy_upgrade_id().unwrap(),
            second.legacy_upgrade_id().unwrap()
        );
    }

    #[test]
    fn legacy_upgrade_identity_uses_the_shared_key_not_the_local_space_label() {
        let first = InMemorySession::new();
        first.set_master_key_for_space(SpaceId::from_str("local-space-a"), key(7));
        let second = InMemorySession::new();
        second.set_master_key_for_space(SpaceId::from_str("local-space-b"), key(7));
        let transcript = b"same-upgrade-request";

        assert_eq!(
            first.legacy_upgrade_id().unwrap(),
            second.legacy_upgrade_id().unwrap()
        );
        let proof = first.legacy_upgrade_proof(transcript).unwrap();
        assert!(second
            .verify_legacy_upgrade_proof(transcript, &proof)
            .unwrap());
    }

    #[test]
    fn legacy_upgrade_id_differs_for_a_different_legacy_space_key() {
        let first = InMemorySession::new();
        first.set_master_key_for_space(SpaceId::from_str("space-a"), key(7));
        let second = InMemorySession::new();
        second.set_master_key_for_space(SpaceId::from_str("space-a"), key(8));

        assert_ne!(
            first.legacy_upgrade_id().unwrap(),
            second.legacy_upgrade_id().unwrap()
        );
    }

    #[test]
    fn legacy_upgrade_proof_is_bound_to_the_complete_request_transcript() {
        let session = InMemorySession::new();
        session.set_master_key_for_space(SpaceId::from_str("space-a"), key(7));
        let request = b"device-a|device-b|group-a|key-package-a";
        let proof = session.legacy_upgrade_proof(request).unwrap();

        assert!(session
            .verify_legacy_upgrade_proof(request, &proof)
            .unwrap());
        assert!(!session
            .verify_legacy_upgrade_proof(b"device-a|device-c|group-a|key-package-a", &proof,)
            .unwrap());
    }

    #[test]
    fn bootstrap_material_uses_the_selected_protection_group_id() {
        let session = InMemorySession::new();
        let space_id = SpaceId::from_str("space-a");
        let group_id = ProtectionGroupId::from_string("group-a").unwrap();
        session.set_master_key_for_space(space_id.clone(), key(7));

        let material = session
            .create_legacy_bootstrap_material_in_group(&space_id, group_id.clone(), vec![1], 100)
            .unwrap();

        assert_eq!(material.state().protection_group_id(), Some(&group_id));
    }

    #[test]
    fn joining_a_winning_group_preserves_local_history_keys() {
        let space_id = SpaceId::from_str("space-a");
        let local = InMemorySession::new();
        local.set_master_key_for_space(space_id.clone(), key(7));
        let local_material = local
            .create_legacy_bootstrap_material_in_group(
                &space_id,
                ProtectionGroupId::from_string("group-b").unwrap(),
                vec![1],
                100,
            )
            .unwrap();
        let local_key_id = local_material.state().current_content_key_id().clone();

        let winner = InMemorySession::new();
        winner.set_master_key_for_space(space_id.clone(), key(7));
        let winning_material = winner
            .create_legacy_bootstrap_material_in_group(
                &space_id,
                ProtectionGroupId::from_string("group-a").unwrap(),
                vec![2],
                100,
            )
            .unwrap();

        let merged = local
            .merge_space_material_history(&local_material, winning_material)
            .unwrap();
        local.install_space_material(&merged).unwrap();

        assert!(local
            .content_key(&space_id, &local_key_id, ContentKeyPurpose::Content)
            .is_ok());
        assert_eq!(
            merged.state().protection_group_id().unwrap().as_str(),
            "group-a"
        );
    }
}
