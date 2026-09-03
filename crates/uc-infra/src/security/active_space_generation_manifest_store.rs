use std::path::PathBuf;
use std::sync::Arc;

use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use uc_core::ids::SpaceId;
use uc_core::membership::{ActiveRuntimeLayout, ActiveSpaceGenerationManifestV2};

use super::{AdmissionKeyError, AdmissionKeyManager};

const ACTIVE_GENERATION_MANIFEST_FILE: &str = ".active-space-manifest-v2";
const ACTIVE_GENERATION_MANIFEST_PURPOSE: &[u8] = b"active-space-manifest-v2";
const DEVICE_RESET_JOURNAL_FILE: &str = ".device-management-reset-v1";
const DEVICE_RESET_JOURNAL_PURPOSE: &[u8] = b"device-management-reset-v1";
const ACTIVE_RUNTIME_MANIFEST_FORMAT_V3: u16 = 3;
const ACTIVE_RUNTIME_MANIFEST_DIGEST_DOMAIN_V3: &[u8] =
    b"uniclipboard/active-runtime-manifest/v3\0";

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct PersistedActiveRuntimeManifestV3 {
    format_version: u16,
    space_id: String,
    keyslot_generation: [u8; 16],
    profile_data_generation: [u8; 16],
    space_control_generation: [u8; 16],
    manifest_digest: [u8; 32],
}

impl PersistedActiveRuntimeManifestV3 {
    fn from_manifest(manifest: &ActiveRuntimeManifestV3) -> Self {
        let mut persisted = Self {
            format_version: ACTIVE_RUNTIME_MANIFEST_FORMAT_V3,
            space_id: manifest.layout.space_id().as_ref().to_owned(),
            keyslot_generation: manifest.keyslot_generation,
            profile_data_generation: *manifest.layout.profile_data_generation(),
            space_control_generation: *manifest.layout.space_control_generation(),
            manifest_digest: [0; 32],
        };
        persisted.manifest_digest = persisted.expected_digest();
        persisted
    }

    fn expected_digest(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(ACTIVE_RUNTIME_MANIFEST_DIGEST_DOMAIN_V3);
        hasher.update(self.format_version.to_be_bytes());
        hasher.update((self.space_id.len() as u64).to_be_bytes());
        hasher.update(self.space_id.as_bytes());
        hasher.update(self.keyslot_generation);
        hasher.update(self.profile_data_generation);
        hasher.update(self.space_control_generation);
        hasher.finalize().into()
    }
}

fn decode_v3_manifest(
    plaintext: &[u8],
) -> Result<ActiveRuntimeManifestV3, ActiveSpaceGenerationManifestStoreError> {
    let persisted: PersistedActiveRuntimeManifestV3 = postcard::from_bytes(plaintext)
        .map_err(|_| ActiveSpaceGenerationManifestStoreError::Corrupt)?;
    if persisted.format_version != ACTIVE_RUNTIME_MANIFEST_FORMAT_V3
        || persisted.keyslot_generation == [0; 16]
        || persisted.manifest_digest != persisted.expected_digest()
    {
        return Err(ActiveSpaceGenerationManifestStoreError::Corrupt);
    }
    let layout = ActiveRuntimeLayout::new(
        SpaceId::from_string(persisted.space_id),
        persisted.profile_data_generation,
        persisted.space_control_generation,
    )
    .map_err(|_| ActiveSpaceGenerationManifestStoreError::Corrupt)?;
    ActiveRuntimeManifestV3::new(layout, persisted.keyslot_generation)
        .ok_or(ActiveSpaceGenerationManifestStoreError::Corrupt)
}

fn manifest_format_version(
    plaintext: &[u8],
) -> Result<u16, ActiveSpaceGenerationManifestStoreError> {
    postcard::take_from_bytes::<u16>(plaintext)
        .map(|(format_version, _)| format_version)
        .map_err(|_| ActiveSpaceGenerationManifestStoreError::Corrupt)
}

/// 已验证的 V3 活动运行布局及其 MasterKey keyslot generation。
#[derive(Clone, PartialEq, Eq)]
pub struct ActiveRuntimeManifestV3 {
    layout: ActiveRuntimeLayout,
    keyslot_generation: [u8; 16],
}

impl ActiveRuntimeManifestV3 {
    pub fn new(layout: ActiveRuntimeLayout, keyslot_generation: [u8; 16]) -> Option<Self> {
        (keyslot_generation != [0; 16]).then_some(Self {
            layout,
            keyslot_generation,
        })
    }

    pub const fn layout(&self) -> &ActiveRuntimeLayout {
        &self.layout
    }

    pub const fn keyslot_generation(&self) -> &[u8; 16] {
        &self.keyslot_generation
    }
}

impl std::fmt::Debug for ActiveRuntimeManifestV3 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ActiveRuntimeManifestV3")
            .field("identifiers", &"[REDACTED]")
            .finish()
    }
}

/// 已认证 manifest 的完整版本选择。
///
/// 旧的 `load`/`load_sync` 继续只接受 V2，供尚未 clean cutover 的旧流程
/// 失败关闭；启动 gate 与 V3-aware runtime 只能使用这个显式版本和。
#[derive(Clone, PartialEq, Eq)]
pub enum ActiveRuntimeManifest {
    V2(ActiveSpaceGenerationManifestV2),
    V3(ActiveRuntimeManifestV3),
}

impl std::fmt::Debug for ActiveRuntimeManifest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ActiveRuntimeManifest")
            .field(
                "format",
                &match self {
                    Self::V2(_) => "V2",
                    Self::V3(_) => "V3",
                },
            )
            .field("identifiers", &"[REDACTED]")
            .finish()
    }
}

/// 从 V2 source 提升 V3 manifest 的稳定比较结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum V3ManifestPromotionOutcome {
    Promoted,
    AlreadyActive,
    SourceChanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum DeviceManagementResetPhaseV3 {
    Allocated,
    Prepared,
    Staged,
    Promoted,
    CleanupPending,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct DeviceManagementResetJournalV3 {
    pub(crate) format_version: u16,
    pub(crate) phase: DeviceManagementResetPhaseV3,
    pub(crate) source_space_id: String,
    pub(crate) source_keyslot_generation: [u8; 16],
    pub(crate) profile_data_generation: [u8; 16],
    pub(crate) source_control_generation: [u8; 16],
    pub(crate) target_space_id: String,
    pub(crate) target_control_generation: [u8; 16],
    pub(crate) prepared_database_digest: [u8; 32],
}

impl DeviceManagementResetJournalV3 {
    pub(crate) fn validate(&self) -> bool {
        self.format_version == 3
            && !self.source_space_id.is_empty()
            && !self.target_space_id.is_empty()
            && self.source_space_id != self.target_space_id
            && self.source_keyslot_generation != [0; 16]
            && self.profile_data_generation != [0; 16]
            && self.source_control_generation != [0; 16]
            && self.target_control_generation != [0; 16]
            && self.source_control_generation != self.target_control_generation
            && self.profile_data_generation != self.source_control_generation
            && self.profile_data_generation != self.target_control_generation
            && match self.phase {
                DeviceManagementResetPhaseV3::Allocated => self.prepared_database_digest == [0; 32],
                DeviceManagementResetPhaseV3::Prepared
                | DeviceManagementResetPhaseV3::Staged
                | DeviceManagementResetPhaseV3::Promoted
                | DeviceManagementResetPhaseV3::CleanupPending => {
                    self.prepared_database_digest != [0; 32]
                }
            }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ActiveSpaceGenerationManifestStoreError {
    #[error("active space generation manifest storage is unavailable")]
    Storage,
    #[error("active space generation manifest is corrupt")]
    Corrupt,
    #[error("active space generation manifest version is not active yet")]
    UnsupportedVersion,
}

pub struct ActiveSpaceGenerationManifestStore {
    path: PathBuf,
    reset_journal_path: PathBuf,
    keys: Arc<AdmissionKeyManager>,
    write_lock: Mutex<()>,
}

impl ActiveSpaceGenerationManifestStore {
    pub fn new(base_dir: PathBuf, keys: Arc<AdmissionKeyManager>) -> Self {
        Self {
            path: base_dir.join(ACTIVE_GENERATION_MANIFEST_FILE),
            reset_journal_path: base_dir.join(DEVICE_RESET_JOURNAL_FILE),
            keys,
            write_lock: Mutex::new(()),
        }
    }

    pub async fn load(
        &self,
    ) -> Result<Option<ActiveSpaceGenerationManifestV2>, ActiveSpaceGenerationManifestStoreError>
    {
        let ciphertext = match tokio::fs::read(&self.path).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(ActiveSpaceGenerationManifestStoreError::Storage),
        };
        self.decode(&ciphertext).map(Some)
    }

    /// 读取并认证任一受支持的活动 runtime manifest。
    pub async fn load_runtime(
        &self,
    ) -> Result<Option<ActiveRuntimeManifest>, ActiveSpaceGenerationManifestStoreError> {
        let ciphertext = match tokio::fs::read(&self.path).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(ActiveSpaceGenerationManifestStoreError::Storage),
        };
        self.decode_runtime(&ciphertext).map(Some)
    }

    pub fn load_sync(
        &self,
    ) -> Result<Option<ActiveSpaceGenerationManifestV2>, ActiveSpaceGenerationManifestStoreError>
    {
        let ciphertext = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(ActiveSpaceGenerationManifestStoreError::Storage),
        };
        self.decode(&ciphertext).map(Some)
    }

    /// `load_runtime` 的同步启动版本；不会把 V3 降级解释成 V2。
    pub fn load_runtime_sync(
        &self,
    ) -> Result<Option<ActiveRuntimeManifest>, ActiveSpaceGenerationManifestStoreError> {
        let ciphertext = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(ActiveSpaceGenerationManifestStoreError::Storage),
        };
        self.decode_runtime(&ciphertext).map(Some)
    }

    /// 只读取已提升的 V3 runtime manifest；V2 保持显式不支持。
    pub fn load_v3_sync(
        &self,
    ) -> Result<Option<ActiveRuntimeManifestV3>, ActiveSpaceGenerationManifestStoreError> {
        let ciphertext = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(ActiveSpaceGenerationManifestStoreError::Storage),
        };
        let plaintext = self.open_manifest(&ciphertext)?;
        let format_version = manifest_format_version(&plaintext)?;
        if format_version != ACTIVE_RUNTIME_MANIFEST_FORMAT_V3 {
            return Err(ActiveSpaceGenerationManifestStoreError::UnsupportedVersion);
        }
        decode_v3_manifest(&plaintext).map(Some)
    }

    fn decode(
        &self,
        ciphertext: &[u8],
    ) -> Result<ActiveSpaceGenerationManifestV2, ActiveSpaceGenerationManifestStoreError> {
        match self.decode_runtime(ciphertext)? {
            ActiveRuntimeManifest::V2(manifest) => Ok(manifest),
            ActiveRuntimeManifest::V3(_) => {
                Err(ActiveSpaceGenerationManifestStoreError::UnsupportedVersion)
            }
        }
    }

    fn decode_runtime(
        &self,
        ciphertext: &[u8],
    ) -> Result<ActiveRuntimeManifest, ActiveSpaceGenerationManifestStoreError> {
        let plaintext = self.open_manifest(ciphertext)?;
        let format_version = manifest_format_version(&plaintext)?;
        match format_version {
            uc_core::membership::ACTIVE_SPACE_GENERATION_MANIFEST_FORMAT_V2 => {
                let manifest: ActiveSpaceGenerationManifestV2 = postcard::from_bytes(&plaintext)
                    .map_err(|_| ActiveSpaceGenerationManifestStoreError::Corrupt)?;
                manifest
                    .validate()
                    .then_some(ActiveRuntimeManifest::V2(manifest))
                    .ok_or(ActiveSpaceGenerationManifestStoreError::Corrupt)
            }
            ACTIVE_RUNTIME_MANIFEST_FORMAT_V3 => {
                decode_v3_manifest(&plaintext).map(ActiveRuntimeManifest::V3)
            }
            _ => Err(ActiveSpaceGenerationManifestStoreError::Corrupt),
        }
    }

    pub async fn promote(
        &self,
        manifest: &ActiveSpaceGenerationManifestV2,
    ) -> Result<(), ActiveSpaceGenerationManifestStoreError> {
        if !manifest.validate() {
            return Err(ActiveSpaceGenerationManifestStoreError::Corrupt);
        }
        let _guard = self.write_lock.lock().await;
        let plaintext = postcard::to_stdvec(manifest)
            .map_err(|_| ActiveSpaceGenerationManifestStoreError::Corrupt)?;
        self.persist_manifest(&plaintext).await
    }

    /// 仅当活动 manifest 仍是指定 V2 source 时原子提升为 V3。
    ///
    /// 重启遇到同一 V3 target 返回 `AlreadyActive`；任何其他已验证 manifest
    /// 返回 `SourceChanged`，不会覆盖后来状态。
    pub(crate) async fn promote_v3_from_v2(
        &self,
        expected_source: &ActiveSpaceGenerationManifestV2,
        target: &ActiveRuntimeManifestV3,
    ) -> Result<V3ManifestPromotionOutcome, ActiveSpaceGenerationManifestStoreError> {
        if !expected_source.validate()
            || target.layout.space_id().as_ref() != expected_source.space_id
            || target.keyslot_generation != expected_source.keyslot_generation
        {
            return Err(ActiveSpaceGenerationManifestStoreError::Corrupt);
        }
        let _guard = self.write_lock.lock().await;
        let ciphertext = match tokio::fs::read(&self.path).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(V3ManifestPromotionOutcome::SourceChanged);
            }
            Err(_) => return Err(ActiveSpaceGenerationManifestStoreError::Storage),
        };
        let plaintext = self.open_manifest(&ciphertext)?;
        match manifest_format_version(&plaintext)? {
            uc_core::membership::ACTIVE_SPACE_GENERATION_MANIFEST_FORMAT_V2 => {
                let current: ActiveSpaceGenerationManifestV2 = postcard::from_bytes(&plaintext)
                    .map_err(|_| ActiveSpaceGenerationManifestStoreError::Corrupt)?;
                if !current.validate() {
                    return Err(ActiveSpaceGenerationManifestStoreError::Corrupt);
                }
                if current != *expected_source {
                    return Ok(V3ManifestPromotionOutcome::SourceChanged);
                }
            }
            ACTIVE_RUNTIME_MANIFEST_FORMAT_V3 => {
                let current = decode_v3_manifest(&plaintext)?;
                return Ok(if current == *target {
                    V3ManifestPromotionOutcome::AlreadyActive
                } else {
                    V3ManifestPromotionOutcome::SourceChanged
                });
            }
            _ => return Err(ActiveSpaceGenerationManifestStoreError::Corrupt),
        }
        let persisted = PersistedActiveRuntimeManifestV3::from_manifest(target);
        let plaintext = postcard::to_stdvec(&persisted)
            .map_err(|_| ActiveSpaceGenerationManifestStoreError::Corrupt)?;
        self.persist_manifest(&plaintext).await?;
        Ok(V3ManifestPromotionOutcome::Promoted)
    }

    /// 只在活动 V3 manifest 仍与 source 完全一致时替换控制世代。
    ///
    /// profile data generation 必须保持不变；同一 target 可在 manifest 已写入、
    /// 运行期尚未重绑的崩溃窗口中幂等恢复。
    pub(crate) async fn promote_v3_control_generation(
        &self,
        expected_source: &ActiveRuntimeManifestV3,
        target: &ActiveRuntimeManifestV3,
    ) -> Result<V3ManifestPromotionOutcome, ActiveSpaceGenerationManifestStoreError> {
        if expected_source.layout.profile_data_generation()
            != target.layout.profile_data_generation()
            || expected_source.layout.space_control_generation()
                == target.layout.space_control_generation()
            || expected_source == target
        {
            return Err(ActiveSpaceGenerationManifestStoreError::Corrupt);
        }
        let _guard = self.write_lock.lock().await;
        let ciphertext = match tokio::fs::read(&self.path).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(V3ManifestPromotionOutcome::SourceChanged);
            }
            Err(_) => return Err(ActiveSpaceGenerationManifestStoreError::Storage),
        };
        let plaintext = self.open_manifest(&ciphertext)?;
        if manifest_format_version(&plaintext)? != ACTIVE_RUNTIME_MANIFEST_FORMAT_V3 {
            return Ok(V3ManifestPromotionOutcome::SourceChanged);
        }
        let current = decode_v3_manifest(&plaintext)?;
        if current == *target {
            return Ok(V3ManifestPromotionOutcome::AlreadyActive);
        }
        if current != *expected_source {
            return Ok(V3ManifestPromotionOutcome::SourceChanged);
        }
        let persisted = PersistedActiveRuntimeManifestV3::from_manifest(target);
        let plaintext = postcard::to_stdvec(&persisted)
            .map_err(|_| ActiveSpaceGenerationManifestStoreError::Corrupt)?;
        self.persist_manifest(&plaintext).await?;
        Ok(V3ManifestPromotionOutcome::Promoted)
    }

    /// 只在尚无活动 manifest 时建立首个 V3 runtime。
    ///
    /// 同一 target 可从 manifest 已写入、运行期尚未恢复的崩溃窗口继续；任何
    /// 既有 V2 或其他 V3 manifest 都视为来源已经变化，绝不覆盖。
    pub(crate) async fn promote_initial_v3(
        &self,
        target: &ActiveRuntimeManifestV3,
    ) -> Result<V3ManifestPromotionOutcome, ActiveSpaceGenerationManifestStoreError> {
        let _guard = self.write_lock.lock().await;
        match tokio::fs::read(&self.path).await {
            Ok(ciphertext) => {
                let current = self.decode_runtime(&ciphertext)?;
                return Ok(match current {
                    ActiveRuntimeManifest::V3(current) if current == *target => {
                        V3ManifestPromotionOutcome::AlreadyActive
                    }
                    ActiveRuntimeManifest::V2(_) | ActiveRuntimeManifest::V3(_) => {
                        V3ManifestPromotionOutcome::SourceChanged
                    }
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(ActiveSpaceGenerationManifestStoreError::Storage),
        }
        let persisted = PersistedActiveRuntimeManifestV3::from_manifest(target);
        let plaintext = postcard::to_stdvec(&persisted)
            .map_err(|_| ActiveSpaceGenerationManifestStoreError::Corrupt)?;
        self.persist_manifest(&plaintext).await?;
        Ok(V3ManifestPromotionOutcome::Promoted)
    }

    fn open_manifest(
        &self,
        ciphertext: &[u8],
    ) -> Result<Vec<u8>, ActiveSpaceGenerationManifestStoreError> {
        self.keys
            .open_profile_payload(ACTIVE_GENERATION_MANIFEST_PURPOSE, ciphertext)
            .map_err(map_key_error)
    }

    async fn persist_manifest(
        &self,
        plaintext: &[u8],
    ) -> Result<(), ActiveSpaceGenerationManifestStoreError> {
        let parent = self
            .path
            .parent()
            .ok_or(ActiveSpaceGenerationManifestStoreError::Storage)?;
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|_| ActiveSpaceGenerationManifestStoreError::Storage)?;
        let ciphertext = self
            .keys
            .seal_profile_payload(ACTIVE_GENERATION_MANIFEST_PURPOSE, plaintext)
            .map_err(map_key_error)?;
        let temporary = self.path.with_extension("tmp");
        let mut file = tokio::fs::File::create(&temporary)
            .await
            .map_err(|_| ActiveSpaceGenerationManifestStoreError::Storage)?;
        file.write_all(&ciphertext)
            .await
            .map_err(|_| ActiveSpaceGenerationManifestStoreError::Storage)?;
        file.sync_all()
            .await
            .map_err(|_| ActiveSpaceGenerationManifestStoreError::Storage)?;
        drop(file);
        replace_file_atomically(&temporary, &self.path)
            .map_err(|_| ActiveSpaceGenerationManifestStoreError::Storage)?;
        sync_parent_directory(parent).map_err(|_| ActiveSpaceGenerationManifestStoreError::Storage)
    }

    pub async fn clear(&self) -> Result<(), ActiveSpaceGenerationManifestStoreError> {
        let _guard = self.write_lock.lock().await;
        match tokio::fs::remove_file(&self.path).await {
            Ok(()) => {
                let parent = self
                    .path
                    .parent()
                    .ok_or(ActiveSpaceGenerationManifestStoreError::Storage)?;
                sync_parent_directory(parent)
                    .map_err(|_| ActiveSpaceGenerationManifestStoreError::Storage)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(ActiveSpaceGenerationManifestStoreError::Storage),
        }
    }

    pub(crate) async fn load_device_reset_journal_v3(
        &self,
    ) -> Result<Option<DeviceManagementResetJournalV3>, ActiveSpaceGenerationManifestStoreError>
    {
        let ciphertext = match tokio::fs::read(&self.reset_journal_path).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(ActiveSpaceGenerationManifestStoreError::Storage),
        };
        let plaintext = self
            .keys
            .open_profile_payload(DEVICE_RESET_JOURNAL_PURPOSE, &ciphertext)
            .map_err(map_key_error)?;
        let journal: DeviceManagementResetJournalV3 = postcard::from_bytes(&plaintext)
            .map_err(|_| ActiveSpaceGenerationManifestStoreError::Corrupt)?;
        journal
            .validate()
            .then_some(Some(journal))
            .ok_or(ActiveSpaceGenerationManifestStoreError::Corrupt)
    }

    pub(crate) async fn save_device_reset_journal_v3(
        &self,
        journal: &DeviceManagementResetJournalV3,
    ) -> Result<(), ActiveSpaceGenerationManifestStoreError> {
        if !journal.validate() {
            return Err(ActiveSpaceGenerationManifestStoreError::Corrupt);
        }
        let _guard = self.write_lock.lock().await;
        let parent = self
            .reset_journal_path
            .parent()
            .ok_or(ActiveSpaceGenerationManifestStoreError::Storage)?;
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|_| ActiveSpaceGenerationManifestStoreError::Storage)?;
        let plaintext = postcard::to_stdvec(journal)
            .map_err(|_| ActiveSpaceGenerationManifestStoreError::Corrupt)?;
        let ciphertext = self
            .keys
            .seal_profile_payload(DEVICE_RESET_JOURNAL_PURPOSE, &plaintext)
            .map_err(map_key_error)?;
        let temporary = self.reset_journal_path.with_extension("tmp");
        let mut file = tokio::fs::File::create(&temporary)
            .await
            .map_err(|_| ActiveSpaceGenerationManifestStoreError::Storage)?;
        file.write_all(&ciphertext)
            .await
            .map_err(|_| ActiveSpaceGenerationManifestStoreError::Storage)?;
        file.sync_all()
            .await
            .map_err(|_| ActiveSpaceGenerationManifestStoreError::Storage)?;
        drop(file);
        replace_file_atomically(&temporary, &self.reset_journal_path)
            .map_err(|_| ActiveSpaceGenerationManifestStoreError::Storage)?;
        sync_parent_directory(parent).map_err(|_| ActiveSpaceGenerationManifestStoreError::Storage)
    }

    pub(crate) async fn clear_device_reset_journal(
        &self,
    ) -> Result<(), ActiveSpaceGenerationManifestStoreError> {
        let _guard = self.write_lock.lock().await;
        match tokio::fs::remove_file(&self.reset_journal_path).await {
            Ok(()) => {
                let parent = self
                    .reset_journal_path
                    .parent()
                    .ok_or(ActiveSpaceGenerationManifestStoreError::Storage)?;
                sync_parent_directory(parent)
                    .map_err(|_| ActiveSpaceGenerationManifestStoreError::Storage)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(ActiveSpaceGenerationManifestStoreError::Storage),
        }
    }
}

#[cfg(not(windows))]
fn replace_file_atomically(
    source: &std::path::Path,
    destination: &std::path::Path,
) -> std::io::Result<()> {
    std::fs::rename(source, destination)
}

#[cfg(not(windows))]
fn sync_parent_directory(parent: &std::path::Path) -> std::io::Result<()> {
    std::fs::File::open(parent)?.sync_all()
}

#[cfg(windows)]
fn sync_parent_directory(_parent: &std::path::Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(windows)]
fn replace_file_atomically(
    source: &std::path::Path,
    destination: &std::path::Path,
) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let wide = |path: &std::path::Path| {
        let mut value: Vec<u16> = path.as_os_str().encode_wide().collect();
        value.push(0);
        value
    };
    let source = wide(source);
    let destination = wide(destination);
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn map_key_error(error: AdmissionKeyError) -> ActiveSpaceGenerationManifestStoreError {
    match error {
        AdmissionKeyError::Corrupt | AdmissionKeyError::OpenFailed => {
            ActiveSpaceGenerationManifestStoreError::Corrupt
        }
        AdmissionKeyError::SecureStorage => ActiveSpaceGenerationManifestStoreError::Storage,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex as StdMutex;

    use uc_core::ports::{SecureStorageError, SecureStoragePort};

    use super::*;

    #[derive(Default)]
    struct MemorySecureStorage(StdMutex<HashMap<String, Vec<u8>>>);

    impl SecureStoragePort for MemorySecureStorage {
        fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SecureStorageError> {
            Ok(self.0.lock().unwrap().get(key).cloned())
        }

        fn set(&self, key: &str, value: &[u8]) -> Result<(), SecureStorageError> {
            self.0
                .lock()
                .unwrap()
                .insert(key.to_owned(), value.to_vec());
            Ok(())
        }

        fn delete(&self, key: &str) -> Result<(), SecureStorageError> {
            self.0.lock().unwrap().remove(key);
            Ok(())
        }
    }

    #[tokio::test]
    async fn promotes_one_encrypted_self_verifying_manifest() {
        let directory = tempfile::tempdir().unwrap();
        let keys = Arc::new(AdmissionKeyManager::new(
            Arc::new(MemorySecureStorage::default()),
            [0x21; 16],
        ));
        let store = ActiveSpaceGenerationManifestStore::new(directory.path().to_path_buf(), keys);
        let first = ActiveSpaceGenerationManifestV2::new(
            "space-a".to_owned(),
            [0x22; 16],
            [0x23; 16],
            [0x24; 16],
        )
        .unwrap();
        store.promote(&first).await.unwrap();
        assert_eq!(store.load().await.unwrap(), Some(first));
        assert!(matches!(
            store.load_runtime().await.unwrap(),
            Some(ActiveRuntimeManifest::V2(_))
        ));
        let bytes = tokio::fs::read(directory.path().join(ACTIVE_GENERATION_MANIFEST_FILE))
            .await
            .unwrap();
        assert!(!bytes
            .windows(b"space-a".len())
            .any(|window| window == b"space-a"));

        let second = ActiveSpaceGenerationManifestV2::new(
            "space-b".to_owned(),
            [0x25; 16],
            [0x26; 16],
            [0x27; 16],
        )
        .unwrap();
        store.promote(&second).await.unwrap();
        assert_eq!(store.load().await.unwrap(), Some(second));
        assert_eq!(store.load_sync().unwrap(), store.load().await.unwrap());
    }

    #[test]
    fn v3_manifest_codec_matches_canonical_digest_and_round_trips_layout() {
        let layout = uc_core::membership::ActiveRuntimeLayout::new(
            uc_core::ids::SpaceId::from_str("space-a"),
            [0x22; 16],
            [0x33; 16],
        )
        .unwrap();
        let manifest = ActiveRuntimeManifestV3::new(layout.clone(), [0x11; 16]).unwrap();
        let persisted = PersistedActiveRuntimeManifestV3::from_manifest(&manifest);

        assert_eq!(
            hex::encode(persisted.manifest_digest),
            "c312dedfa771d511759acd84474db8ee823c775e1f64aa36d514d17fec6abb24"
        );

        let encoded = postcard::to_stdvec(&persisted).unwrap();
        let decoded = decode_v3_manifest(&encoded).unwrap();
        assert_eq!(decoded.layout(), &layout);
        assert_eq!(decoded.keyslot_generation(), &[0x11; 16]);
    }

    #[tokio::test]
    async fn production_loader_recognizes_v3_without_activating_it() {
        let directory = tempfile::tempdir().unwrap();
        let keys = Arc::new(AdmissionKeyManager::new(
            Arc::new(MemorySecureStorage::default()),
            [0x41; 16],
        ));
        let store = ActiveSpaceGenerationManifestStore::new(
            directory.path().to_path_buf(),
            Arc::clone(&keys),
        );
        let layout = uc_core::membership::ActiveRuntimeLayout::new(
            uc_core::ids::SpaceId::from_str("space-v3"),
            [0x42; 16],
            [0x43; 16],
        )
        .unwrap();
        let manifest = ActiveRuntimeManifestV3::new(layout, [0x44; 16]).unwrap();
        let persisted = PersistedActiveRuntimeManifestV3::from_manifest(&manifest);
        let plaintext = postcard::to_stdvec(&persisted).unwrap();
        let ciphertext = keys
            .seal_profile_payload(ACTIVE_GENERATION_MANIFEST_PURPOSE, &plaintext)
            .unwrap();
        tokio::fs::write(
            directory.path().join(ACTIVE_GENERATION_MANIFEST_FILE),
            &ciphertext,
        )
        .await
        .unwrap();
        assert!(!ciphertext
            .windows(b"space-v3".len())
            .any(|window| window == b"space-v3"));

        assert!(matches!(
            store.load().await,
            Err(ActiveSpaceGenerationManifestStoreError::UnsupportedVersion)
        ));
        assert!(matches!(
            store.load_sync(),
            Err(ActiveSpaceGenerationManifestStoreError::UnsupportedVersion)
        ));
        assert_eq!(
            store.load_runtime_sync().unwrap(),
            Some(ActiveRuntimeManifest::V3(manifest))
        );
    }

    #[tokio::test]
    async fn v3_promotion_compares_source_and_is_idempotently_loadable() {
        let directory = tempfile::tempdir().unwrap();
        let keys = Arc::new(AdmissionKeyManager::new(
            Arc::new(MemorySecureStorage::default()),
            [0x71; 16],
        ));
        let store = ActiveSpaceGenerationManifestStore::new(
            directory.path().to_path_buf(),
            Arc::clone(&keys),
        );
        let source = ActiveSpaceGenerationManifestV2::new(
            "source-space".to_owned(),
            [0x72; 16],
            [0x73; 16],
            [0x74; 16],
        )
        .unwrap();
        store.promote(&source).await.unwrap();
        let target = ActiveRuntimeManifestV3::new(
            ActiveRuntimeLayout::new(SpaceId::from_str("source-space"), [0x75; 16], [0x76; 16])
                .unwrap(),
            [0x72; 16],
        )
        .unwrap();

        assert_eq!(
            store.promote_v3_from_v2(&source, &target).await.unwrap(),
            V3ManifestPromotionOutcome::Promoted
        );
        assert_eq!(store.load_v3_sync().unwrap(), Some(target.clone()));
        assert_eq!(
            store.promote_v3_from_v2(&source, &target).await.unwrap(),
            V3ManifestPromotionOutcome::AlreadyActive
        );
        assert!(matches!(
            store.load_sync(),
            Err(ActiveSpaceGenerationManifestStoreError::UnsupportedVersion)
        ));
        let ciphertext =
            std::fs::read(directory.path().join(ACTIVE_GENERATION_MANIFEST_FILE)).unwrap();
        assert!(!ciphertext
            .windows(b"source-space".len())
            .any(|window| window == b"source-space"));
    }

    #[tokio::test]
    async fn v3_control_promotion_keeps_profile_generation_and_is_forward_idempotent() {
        let directory = tempfile::tempdir().unwrap();
        let store = ActiveSpaceGenerationManifestStore::new(
            directory.path().to_path_buf(),
            Arc::new(AdmissionKeyManager::new(
                Arc::new(MemorySecureStorage::default()),
                [0xa1; 16],
            )),
        );
        let legacy = ActiveSpaceGenerationManifestV2::new(
            "source-space".to_owned(),
            [0xa2; 16],
            [0xa3; 16],
            [0xa4; 16],
        )
        .unwrap();
        let source = ActiveRuntimeManifestV3::new(
            ActiveRuntimeLayout::new(SpaceId::from_str("source-space"), [0xa5; 16], [0xa6; 16])
                .unwrap(),
            [0xa2; 16],
        )
        .unwrap();
        store.promote(&legacy).await.unwrap();
        store.promote_v3_from_v2(&legacy, &source).await.unwrap();

        let target = ActiveRuntimeManifestV3::new(
            ActiveRuntimeLayout::new(SpaceId::from_str("target-space"), [0xa5; 16], [0xa7; 16])
                .unwrap(),
            [0xa8; 16],
        )
        .unwrap();
        assert_eq!(
            store
                .promote_v3_control_generation(&source, &target)
                .await
                .unwrap(),
            V3ManifestPromotionOutcome::Promoted
        );
        assert_eq!(
            store
                .promote_v3_control_generation(&source, &target)
                .await
                .unwrap(),
            V3ManifestPromotionOutcome::AlreadyActive
        );
        assert_eq!(
            store.load_runtime().await.unwrap(),
            Some(ActiveRuntimeManifest::V3(target.clone()))
        );

        let later = ActiveRuntimeManifestV3::new(
            ActiveRuntimeLayout::new(SpaceId::from_str("later-space"), [0xa5; 16], [0xa9; 16])
                .unwrap(),
            [0xaa; 16],
        )
        .unwrap();
        assert_eq!(
            store
                .promote_v3_control_generation(&source, &later)
                .await
                .unwrap(),
            V3ManifestPromotionOutcome::SourceChanged
        );
        assert_eq!(
            store.load_runtime().await.unwrap(),
            Some(ActiveRuntimeManifest::V3(target))
        );
    }

    #[tokio::test]
    async fn v3_control_promotion_rejects_profile_generation_replacement() {
        let directory = tempfile::tempdir().unwrap();
        let store = ActiveSpaceGenerationManifestStore::new(
            directory.path().to_path_buf(),
            Arc::new(AdmissionKeyManager::new(
                Arc::new(MemorySecureStorage::default()),
                [0xb1; 16],
            )),
        );
        let source = ActiveRuntimeManifestV3::new(
            ActiveRuntimeLayout::new(SpaceId::from_str("source-space"), [0xb2; 16], [0xb3; 16])
                .unwrap(),
            [0xb4; 16],
        )
        .unwrap();
        let target = ActiveRuntimeManifestV3::new(
            ActiveRuntimeLayout::new(SpaceId::from_str("target-space"), [0xb5; 16], [0xb6; 16])
                .unwrap(),
            [0xb7; 16],
        )
        .unwrap();

        assert!(matches!(
            store.promote_v3_control_generation(&source, &target).await,
            Err(ActiveSpaceGenerationManifestStoreError::Corrupt)
        ));
        assert_eq!(store.load_runtime().await.unwrap(), None);
    }

    #[tokio::test]
    async fn v3_promotion_rejects_a_changed_v2_source() {
        let directory = tempfile::tempdir().unwrap();
        let store = ActiveSpaceGenerationManifestStore::new(
            directory.path().to_path_buf(),
            Arc::new(AdmissionKeyManager::new(
                Arc::new(MemorySecureStorage::default()),
                [0x81; 16],
            )),
        );
        let source = ActiveSpaceGenerationManifestV2::new(
            "source-space".to_owned(),
            [0x82; 16],
            [0x83; 16],
            [0x84; 16],
        )
        .unwrap();
        store.promote(&source).await.unwrap();
        let stale = ActiveSpaceGenerationManifestV2::new(
            "stale-space".to_owned(),
            [0x85; 16],
            [0x86; 16],
            [0x87; 16],
        )
        .unwrap();
        let target = ActiveRuntimeManifestV3::new(
            ActiveRuntimeLayout::new(SpaceId::from_str("stale-space"), [0x88; 16], [0x89; 16])
                .unwrap(),
            [0x85; 16],
        )
        .unwrap();

        assert_eq!(
            store.promote_v3_from_v2(&stale, &target).await.unwrap(),
            V3ManifestPromotionOutcome::SourceChanged
        );
        assert_eq!(store.load_sync().unwrap(), Some(source));
    }

    #[tokio::test]
    async fn v3_promotion_cannot_change_space_or_keyslot_identity() {
        let directory = tempfile::tempdir().unwrap();
        let store = ActiveSpaceGenerationManifestStore::new(
            directory.path().to_path_buf(),
            Arc::new(AdmissionKeyManager::new(
                Arc::new(MemorySecureStorage::default()),
                [0x91; 16],
            )),
        );
        let source = ActiveSpaceGenerationManifestV2::new(
            "source-space".to_owned(),
            [0x92; 16],
            [0x93; 16],
            [0x94; 16],
        )
        .unwrap();
        store.promote(&source).await.unwrap();
        let wrong_space = ActiveRuntimeManifestV3::new(
            ActiveRuntimeLayout::new(SpaceId::from_str("other-space"), [0x95; 16], [0x96; 16])
                .unwrap(),
            [0x92; 16],
        )
        .unwrap();
        let wrong_keyslot = ActiveRuntimeManifestV3::new(
            ActiveRuntimeLayout::new(SpaceId::from_str("source-space"), [0x95; 16], [0x96; 16])
                .unwrap(),
            [0x97; 16],
        )
        .unwrap();

        assert!(matches!(
            store.promote_v3_from_v2(&source, &wrong_space).await,
            Err(ActiveSpaceGenerationManifestStoreError::Corrupt)
        ));
        assert!(matches!(
            store.promote_v3_from_v2(&source, &wrong_keyslot).await,
            Err(ActiveSpaceGenerationManifestStoreError::Corrupt)
        ));
        assert_eq!(store.load_sync().unwrap(), Some(source));
    }

    #[test]
    fn v3_manifest_codec_rejects_authenticated_field_tampering() {
        let layout = uc_core::membership::ActiveRuntimeLayout::new(
            uc_core::ids::SpaceId::from_str("space-v3"),
            [0x52; 16],
            [0x53; 16],
        )
        .unwrap();
        let manifest = ActiveRuntimeManifestV3::new(layout, [0x51; 16]).unwrap();
        let original = PersistedActiveRuntimeManifestV3::from_manifest(&manifest);

        let mut candidates = Vec::new();
        let mut changed = original.clone();
        changed.space_id = "space-other".to_owned();
        candidates.push(changed);
        let mut changed = original.clone();
        changed.keyslot_generation = [0x61; 16];
        candidates.push(changed);
        let mut changed = original.clone();
        changed.profile_data_generation = [0x62; 16];
        candidates.push(changed);
        let mut changed = original.clone();
        changed.space_control_generation = [0x63; 16];
        candidates.push(changed);
        let mut changed = original;
        changed.manifest_digest = [0x64; 32];
        candidates.push(changed);

        for candidate in candidates {
            let encoded = postcard::to_stdvec(&candidate).unwrap();
            assert!(matches!(
                decode_v3_manifest(&encoded),
                Err(ActiveSpaceGenerationManifestStoreError::Corrupt)
            ));
        }
    }
}
