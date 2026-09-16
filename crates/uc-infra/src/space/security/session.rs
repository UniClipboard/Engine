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
use sha2::Sha256;
use tokio::sync::Notify;
use tracing::{debug, debug_span};
use uc_core::crypto::model::EncryptionError;
use uc_core::ids::SpaceId;
use uc_core::membership::{
    ContentKeyId, ContentKeyPurpose, GroupEpoch, ProtectionGroupId, SpaceKeyMaterial,
    SpaceSecurityMode,
};
use zeroize::Zeroizing;

use crate::security::ProfileKeyReadLease;
use crate::security::{MasterKey, ProfileContentKeyVault};

use super::content_key_catalog::{
    decode as decode_content_key_catalog, encode as encode_content_key_catalog,
    PersistedContentKeyCatalog, PersistedContentKeyEntry,
};

#[derive(Clone, Debug)]
struct State {
    master_key: Option<MasterKey>,
    space_id: Option<SpaceId>,
    protection_group_id: Option<ProtectionGroupId>,
    current_content_key_id: Option<ContentKeyId>,
    current_epoch: Option<GroupEpoch>,
    content_keys: HashMap<ContentKeyId, ContentKeyEntry>,
}

#[derive(Clone, Debug)]
struct ContentKeyEntry {
    epoch: GroupEpoch,
    key: MasterKey,
}

pub(crate) struct ResolvedContentKey {
    content_key_id: ContentKeyId,
    epoch: GroupEpoch,
    key: MasterKey,
}

/// V3 持久内容新写入所需的完整原始密钥上下文。
///
/// 该值只在 Infra 内部跨越 session 与 `ContentProtection` 边界；调用方不能
/// 选择保护组、key id 或 epoch，也不能取得密钥字节。
pub(crate) struct ActiveContentProtectionKey {
    protection_group_id: ProtectionGroupId,
    content_key_id: ContentKeyId,
    epoch: GroupEpoch,
    key: MasterKey,
}

impl ActiveContentProtectionKey {
    pub(crate) fn protection_group_id(&self) -> &ProtectionGroupId {
        &self.protection_group_id
    }

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
/// `MasterKey` 派生 `ZeroizeOnDrop`，替换、clear 与析构会清理本容器材料；
/// clear 同时撤销 profile 目录复用。密码操作已取得的短期副本由各自析构清理，
/// 不承诺撤回已返回的明文，也不保证操作系统交换页中的历史副本已被擦除。
#[derive(Clone)]
pub struct InMemorySession {
    state: Arc<Mutex<SessionState>>,
    ready: Arc<Notify>,
}

struct SessionState {
    material: State,
    generation: Arc<()>,
    lease: Option<ProfileKeyReadLease>,
    closed: bool,
    allow_reuse: bool,
}

impl std::ops::Deref for SessionState {
    type Target = State;
    fn deref(&self) -> &State {
        &self.material
    }
}
impl std::ops::DerefMut for SessionState {
    fn deref_mut(&mut self) -> &mut State {
        &mut self.material
    }
}
impl SessionState {
    fn new(material: State) -> Self {
        Self {
            material,
            generation: Arc::new(()),
            lease: None,
            closed: false,
            allow_reuse: true,
        }
    }
}

pub(crate) struct SessionSnapshot {
    material: State,
    generation: Arc<()>,
}

/// 异步激活的回滚责任留在 Infra；取消也不能留下临时装入的目标密钥。
pub(crate) struct SessionTransaction<'a> {
    session: &'a InMemorySession,
    previous: Option<SessionSnapshot>,
}

impl SessionTransaction<'_> {
    pub(crate) fn commit(
        mut self,
        vault: &ProfileContentKeyVault,
    ) -> Result<(), super::active_space_security_session::ActiveSpaceSecuritySessionError> {
        use super::active_space_security_session::ActiveSpaceSecuritySessionError;
        let mut state = self.session.lock_state();
        let valid = self
            .previous
            .as_ref()
            .is_some_and(|previous| Arc::ptr_eq(&previous.generation, &state.generation));
        if !valid || state.closed || state.master_key.is_none() {
            return Err(ActiveSpaceSecuritySessionError::Session {
                source: anyhow::Error::new(EncryptionError::NotInitialized),
            });
        }
        if state.allow_reuse && state.lease.is_none() {
            state.lease = Some(vault.begin_read_reuse().map_err(|source| {
                ActiveSpaceSecuritySessionError::Vault {
                    source: anyhow::Error::new(source),
                }
            })?);
        }
        self.previous = None;
        drop(state);
        self.session.ready.notify_waiters();
        Ok(())
    }
}

impl Drop for SessionTransaction<'_> {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            self.session.restore(previous);
        }
    }
}

impl InMemorySession {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(SessionState::new(State {
                master_key: None,
                space_id: None,
                protection_group_id: None,
                current_content_key_id: None,
                current_epoch: None,
                content_keys: HashMap::new(),
            }))),
            ready: Arc::new(Notify::new()),
        }
    }

    /// 离线升级/验证不取得后台运行期的复用许可。
    pub(crate) fn for_maintenance() -> Self {
        let session = Self::new();
        session.lock_state().allow_reuse = false;
        session
    }

    pub fn is_ready(&self) -> bool {
        match self.state.lock() {
            Ok(state) => state.master_key.is_some(),
            Err(poisoned) => poisoned.into_inner().master_key.is_some(),
        }
    }

    pub(crate) fn detached_clone(&self) -> Arc<Self> {
        let session = Self::for_maintenance();
        session.lock_state().material = self.lock_state().material.clone();
        Arc::new(session)
    }

    pub async fn wait_until_ready(&self) {
        loop {
            let notified = self.ready.notified();
            if self.is_ready() {
                return;
            }
            notified.await;
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
            if state.closed {
                return;
            }
            state.master_key = Some(master_key);
            state.space_id = None;
            state.protection_group_id = None;
            state.current_content_key_id = None;
            state.current_epoch = None;
            state.content_keys.clear();
            debug!("master key set");
        });
        self.ready.notify_waiters();
    }

    pub(crate) fn set_master_key_for_space(&self, space_id: SpaceId, master_key: MasterKey) {
        let mut state = self.lock_state();
        if state.closed {
            return;
        }
        Self::set_space_key(&mut state, space_id, master_key);
        drop(state);
        self.ready.notify_waiters();
    }

    /// 重绑与 clear 共用一个临界区，不能把清理前复制出的 MasterKey 写回。
    pub(crate) fn rebind_to_space(&self, space_id: &SpaceId) -> Result<(), EncryptionError> {
        let mut state = self.lock_state();
        let master_key = state
            .master_key
            .as_ref()
            .cloned()
            .ok_or(EncryptionError::NotInitialized)?;
        Self::set_space_key(&mut state, space_id.clone(), master_key);
        drop(state);
        self.ready.notify_waiters();
        Ok(())
    }

    fn set_space_key(state: &mut State, space_id: SpaceId, master_key: MasterKey) {
        state.master_key = Some(master_key.clone());
        state.space_id = Some(space_id);
        state.protection_group_id = None;
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

    pub(crate) fn begin_transaction(
        &self,
        target: Option<(SpaceId, MasterKey)>,
    ) -> Result<SessionTransaction<'_>, EncryptionError> {
        let mut state = self.lock_state();
        if state.closed {
            return Err(EncryptionError::NotInitialized);
        }
        let previous = SessionSnapshot {
            material: state.material.clone(),
            generation: state.generation.clone(),
        };
        if let Some((space_id, key)) = target {
            Self::set_space_key(&mut state, space_id, key);
        }
        Ok(SessionTransaction {
            session: self,
            previous: Some(previous),
        })
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
        let key_catalog = encode_content_key_catalog(&catalog)?;
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

    pub(crate) fn create_profile_storage_upgrade_material(
        &self,
        space_id: &SpaceId,
    ) -> Result<SpaceKeyMaterial, EncryptionError> {
        const CONTENT_KEY_INFO: &[u8] = b"uniclipboard/profile-storage-upgrade/content/v1";
        const CONTENT_KEY_ID: &str = "legacy-profile-upgrade-v1";
        const PROTECTION_GROUP_ID: &str = "legacy-profile-upgrade-v1";

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

        let content_key_id = ContentKeyId::from_string(CONTENT_KEY_ID)
            .map_err(|_| EncryptionError::KeyMaterialCorrupt)?;
        let protection_group_id = ProtectionGroupId::from_string(PROTECTION_GROUP_ID)
            .map_err(|_| EncryptionError::KeyMaterialCorrupt)?;
        let hkdf = Hkdf::<Sha256>::new(Some(space_id.as_ref().as_bytes()), legacy_key.as_bytes());
        let mut content_key_bytes = Zeroizing::new([0_u8; MasterKey::LEN]);
        hkdf.expand(CONTENT_KEY_INFO, content_key_bytes.as_mut())
            .map_err(|_| EncryptionError::CryptoFailure)?;
        let content_key = MasterKey::from_bytes(content_key_bytes.as_ref())?;
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
        let mut key_state = uc_core::membership::SpaceKeyState::legacy(space_id.clone());
        key_state
            .mark_migrating()
            .map_err(|_| EncryptionError::KeyMaterialCorrupt)?;
        key_state
            .mark_ready(content_key_id, protection_group_id)
            .map_err(|_| EncryptionError::KeyMaterialCorrupt)?;
        Ok(SpaceKeyMaterial::new(
            key_state,
            CONTENT_KEY_INFO.to_vec(),
            encode_content_key_catalog(&catalog)?,
            0,
        ))
    }

    #[cfg(test)]
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
        let protection_group_id = material.state().protection_group_id().cloned();
        let catalog = decode_content_key_catalog(material.key_catalog())?;
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
        for persisted in &catalog.entries {
            let content_key_id = ContentKeyId::from_string(persisted.content_key_id.clone())
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
        state.protection_group_id = protection_group_id;
        state.current_content_key_id = Some(current_id.clone());
        state.current_epoch = Some(material.state().epoch());
        Ok(())
    }

    pub(crate) fn rotate_space_material(
        &self,
        material: &SpaceKeyMaterial,
        group_state: Vec<u8>,
        expected_epoch: GroupEpoch,
        updated_at_ms: i64,
    ) -> Result<SpaceKeyMaterial, EncryptionError> {
        let mut catalog = decode_content_key_catalog(material.key_catalog())?;
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
        let key_catalog = encode_content_key_catalog(&catalog)?;
        Ok(
            SpaceKeyMaterial::new(state, group_state, key_catalog, updated_at_ms)
                .with_pending_group_updates_from(material),
        )
    }

    #[cfg(test)]
    pub(crate) fn snapshot(&self) -> SessionSnapshot {
        {
            let state = self.lock_state();
            SessionSnapshot {
                material: state.material.clone(),
                generation: state.generation.clone(),
            }
        }
    }

    pub(crate) fn restore(&self, snapshot: SessionSnapshot) {
        let mut state = self.lock_state();
        if !state.closed && Arc::ptr_eq(&state.generation, &snapshot.generation) {
            state.material = snapshot.material;
        }
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

    pub(crate) fn current_content_protection_key(
        &self,
    ) -> Result<ActiveContentProtectionKey, EncryptionError> {
        let state = self.lock_state();
        let protection_group_id = state
            .protection_group_id
            .as_ref()
            .cloned()
            .ok_or(EncryptionError::NotInitialized)?;
        let content_key_id = state
            .current_content_key_id
            .as_ref()
            .cloned()
            .ok_or(EncryptionError::NotInitialized)?;
        let entry = state
            .content_keys
            .get(&content_key_id)
            .ok_or(EncryptionError::KeyNotFound)?;
        if entry.epoch != state.current_epoch.ok_or(EncryptionError::NotInitialized)? {
            return Err(EncryptionError::KeyMaterialCorrupt);
        }
        Ok(ActiveContentProtectionKey {
            protection_group_id,
            content_key_id,
            epoch: entry.epoch,
            key: entry.key.clone(),
        })
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

    fn lock_state(&self) -> std::sync::MutexGuard<'_, SessionState> {
        match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    pub(crate) fn close(&self) {
        let mut state = self.lock_state();
        state.closed = true;
        Self::clear_state(&mut state);
    }

    pub fn clear(&self) {
        let span = debug_span!("infra.session.clear");
        span.in_scope(|| {
            let mut state = self.lock_state();
            Self::clear_state(&mut state);
            debug!("master key cleared");
        });
    }
    fn clear_state(state: &mut SessionState) {
        state.generation = Arc::new(());
        state.lease = None;
        state.master_key = None;
        state.space_id = None;
        state.protection_group_id = None;
        state.current_content_key_id = None;
        state.current_epoch = None;
        state.content_keys.clear();
    }
}

impl Default for InMemorySession {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use uc_core::ids::SpaceId;
    use uc_core::membership::{ContentKeyId, ContentKeyPurpose, GroupEpoch, ProtectionGroupId};

    use super::*;

    #[tokio::test]
    async fn wait_until_ready_unblocks_when_space_material_is_installed() {
        let session = InMemorySession::new();
        let waiting = session.wait_until_ready();
        tokio::pin!(waiting);

        assert!(
            tokio::time::timeout(Duration::from_millis(10), &mut waiting)
                .await
                .is_err()
        );

        session.set_master_key_for_space(SpaceId::from_str("space-a"), key(1));

        tokio::time::timeout(Duration::from_millis(10), waiting)
            .await
            .expect("waiting session must unblock when it becomes ready");
    }

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
}
