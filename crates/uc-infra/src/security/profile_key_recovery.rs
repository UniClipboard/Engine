use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use uc_core::app_dirs::AppPaths;
use uc_core::crypto::domain::Passphrase;
use uc_core::crypto::model::EncryptionError;
use uc_core::ports::{SecureStorageError, SecureStoragePort};
use zeroize::Zeroize;

use super::admission_key_manager::PROFILE_ADMISSION_KEY_NAME;
use super::crypto_model::{EncryptedBlob, KeyScope};
use super::key_migration_adapter::{DefaultKeyMigrationAdapter, KEYRING_PREFIX};
use super::profile_content_key_vault::PROFILE_CONTENT_VAULT_KEY_NAME;
use super::profile_lifecycle::PROFILE_LIFECYCLE_MARKER_NAME;
use super::profile_upgrade_backup::PROFILE_UPGRADE_BACKUP_RECORD_KEY;
use super::{v1_aead, Kek, MasterKey};
use crate::fs::durability::{replace_file, sync_directory};
use crate::fs::key_slot_store::JsonKeySlotStore;
use crate::migration_state::{decode_legacy_migration_run_id, DEFAULT_MIGRATION_STATE_FILE};
use crate::network::iroh::IDENTITY_STORE_KEY;
use crate::space::KeyMaterialStore;
use crate::FileSecureStorage;

pub const PROFILE_SECRET_FILE_NAME: &str = "profile-secrets-v1";
const FORMAT_VERSION: u16 = 1;
const MAX_FILE_BYTES: u64 = 1024 * 1024;
const MAX_SECRET_BYTES: usize = 16 * 1024;
const PAYLOAD_AAD: &[u8] = b"uniclipboard/profile-secret-payload/v1";
const MANAGED_KEYS: [&str; 5] = [
    PROFILE_ADMISSION_KEY_NAME,
    PROFILE_CONTENT_VAULT_KEY_NAME,
    PROFILE_LIFECYCLE_MARKER_NAME,
    PROFILE_UPGRADE_BACKUP_RECORD_KEY,
    IDENTITY_STORE_KEY,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileRecoveryPreparation {
    Ready,
    AwaitingPassphrase { losses: ProfileRecoveryLosses },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProfileRecoveryLosses {
    pub local_history: bool,
    pub local_control_state: bool,
    pub device_identity: bool,
}

impl ProfileRecoveryLosses {
    pub const fn is_empty(self) -> bool {
        !self.local_history && !self.local_control_state && !self.device_identity
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileRecoveryOutcome {
    Ready,
    PartiallyRecoverable(ProfileRecoveryLosses),
}

#[derive(Debug, thiserror::Error)]
pub enum ProfileKeyRecoveryError {
    #[error("profile recovery passphrase was rejected")]
    WrongPassphrase,
    #[error("profile recovery data is corrupt")]
    Corrupt,
    #[error("profile recovery format is unsupported")]
    Unsupported,
    #[error("profile recovery storage is unavailable")]
    Storage(#[source] anyhow::Error),
}

impl From<SecureStorageError> for ProfileKeyRecoveryError {
    fn from(source: SecureStorageError) -> Self {
        Self::Storage(source.into())
    }
}

impl From<std::io::Error> for ProfileKeyRecoveryError {
    fn from(source: std::io::Error) -> Self {
        Self::Storage(source.into())
    }
}

impl From<EncryptionError> for ProfileKeyRecoveryError {
    fn from(source: EncryptionError) -> Self {
        match source {
            EncryptionError::WrongPassphrase => Self::WrongPassphrase,
            EncryptionError::UnsupportedKeySlotVersion
            | EncryptionError::UnsupportedBlobVersion
            | EncryptionError::UnsupportedVersion
            | EncryptionError::UnsupportedKdfAlgorithm => Self::Unsupported,
            EncryptionError::CorruptedKeySlot
            | EncryptionError::CorruptedBlob
            | EncryptionError::KeyMaterialCorrupt
            | EncryptionError::InvalidKey => Self::Corrupt,
            other => Self::Storage(other.into()),
        }
    }
}

impl From<v1_aead::AeadError> for ProfileKeyRecoveryError {
    fn from(source: v1_aead::AeadError) -> Self {
        Self::Storage(anyhow::Error::new(source))
    }
}

#[derive(Serialize, Deserialize)]
struct ProfileSecretFile {
    format_version: u16,
    wrapped_root: EncryptedBlob,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending_wrapped_root: Option<EncryptedBlob>,
    encrypted_payload: EncryptedBlob,
}

#[derive(Serialize, Deserialize)]
struct ProfileSecretPayload {
    format_version: u16,
    cleanup_authorized: bool,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    removed_names: BTreeSet<String>,
    secrets: BTreeMap<String, Vec<u8>>,
}

struct ActiveVault {
    root: MasterKey,
    wrapped_root: EncryptedBlob,
    pending_wrapped_root: Option<EncryptedBlob>,
    cleanup_authorized: bool,
    removed_names: BTreeSet<String>,
    secrets: BTreeMap<String, Vec<u8>>,
}

impl Drop for ActiveVault {
    fn drop(&mut self) {
        for secret in self.secrets.values_mut() {
            secret.zeroize();
        }
    }
}

#[derive(Default)]
struct RecoveryState {
    active: Option<ActiveVault>,
    cleanup_pending: bool,
}

/// Secure-storage facade whose independent profile secrets live encrypted in userdata.
/// The platform store retains only the passphrase-derived KEK.
pub struct ProfileKeyRecoveryStore {
    backing: Arc<dyn SecureStoragePort>,
    material: KeyMaterialStore,
    scope: KeyScope,
    kek_name: String,
    file: PathBuf,
    paths: AppPaths,
    state: Mutex<RecoveryState>,
}

pub trait ProfilePassphraseRecoveryPort: Send + Sync {
    fn prepare_passphrase_change(&self, kek: &[u8]) -> Result<(), ProfileKeyRecoveryError>;
    fn finish_passphrase_change(&self, kek: &[u8]) -> Result<(), ProfileKeyRecoveryError>;
}

impl ProfilePassphraseRecoveryPort for ProfileKeyRecoveryStore {
    fn prepare_passphrase_change(&self, kek: &[u8]) -> Result<(), ProfileKeyRecoveryError> {
        let kek = Kek::from_bytes(kek)?;
        self.prepare_passphrase_change(&kek)
    }

    fn finish_passphrase_change(&self, kek: &[u8]) -> Result<(), ProfileKeyRecoveryError> {
        let kek = Kek::from_bytes(kek)?;
        self.finish_passphrase_change(&kek)
    }
}

impl ProfileKeyRecoveryStore {
    pub fn new(paths: AppPaths, profile_id: String, backing: Arc<dyn SecureStoragePort>) -> Self {
        let kek_name = format!("kek:v1:profile:{profile_id}");
        let file = paths.vault_dir.join(PROFILE_SECRET_FILE_NAME);
        Self {
            material: KeyMaterialStore::new(
                Arc::clone(&backing),
                Arc::new(JsonKeySlotStore::new(paths.vault_dir.clone())),
            ),
            backing,
            scope: KeyScope { profile_id },
            kek_name,
            file,
            paths,
            state: Mutex::new(RecoveryState::default()),
        }
    }

    pub fn vault_file(&self) -> &Path {
        &self.file
    }

    pub fn forget_after_factory_reset(&self) {
        let mut state = self.lock_state();
        state.active = None;
        state.cleanup_pending = false;
    }

    /// Removes decrypted profile secrets from memory after the runtime suspends.
    pub fn suspend(&self) {
        self.lock_state().active = None;
    }

    pub fn cleanup_pending(&self) -> bool {
        self.lock_state().cleanup_pending
    }

    pub async fn prepare_startup(
        &self,
    ) -> Result<ProfileRecoveryPreparation, ProfileKeyRecoveryError> {
        if !self.material.keyslot_exists().await? {
            return Ok(ProfileRecoveryPreparation::Ready);
        }
        let keyslot = self.material.load_keyslot(&self.scope).await?;
        let losses = self.legacy_material_losses()?;
        if !losses.is_empty() {
            return Ok(ProfileRecoveryPreparation::AwaitingPassphrase { losses });
        }
        match self.material.load_kek(&self.scope).await {
            Ok(kek) => {
                let Some(wrapped) = keyslot.wrapped_master_key.as_ref() else {
                    return Err(ProfileKeyRecoveryError::Corrupt);
                };
                if v1_aead::unwrap_master_key_xchacha(&kek, &wrapped.blob).is_err() {
                    return Ok(ProfileRecoveryPreparation::AwaitingPassphrase { losses });
                }
                self.activate_or_migrate(&kek)?;
                Ok(ProfileRecoveryPreparation::Ready)
            }
            Err(EncryptionError::KeyNotFound | EncryptionError::KeyMaterialCorrupt) => {
                Ok(ProfileRecoveryPreparation::AwaitingPassphrase { losses })
            }
            Err(error) => Err(error.into()),
        }
    }

    pub async fn recover(
        &self,
        passphrase: &Passphrase,
    ) -> Result<ProfileRecoveryOutcome, ProfileKeyRecoveryError> {
        let (_master, kek) = self
            .material
            .authenticate_kek(&self.scope, passphrase)
            .await?;
        let losses = self.legacy_material_losses()?;
        if !losses.is_empty() {
            return Ok(ProfileRecoveryOutcome::PartiallyRecoverable(losses));
        }
        self.material
            .persist_authenticated_kek(&self.scope, &kek)
            .await?;
        self.activate_or_migrate(&kek)?;
        self.authorize_cleanup()?;
        self.cleanup_legacy_entries()?;
        Ok(ProfileRecoveryOutcome::Ready)
    }

    pub async fn refresh_after_authentication(&self) -> Result<(), ProfileKeyRecoveryError> {
        if !self.material.keyslot_exists().await? {
            return Ok(());
        }
        let kek = self.material.load_kek(&self.scope).await?;
        if self.file.exists() && self.activate_existing(&kek).is_ok() {
            self.authorize_cleanup()?;
            self.cleanup_legacy_entries()?;
            return Ok(());
        }
        if self.file.exists() && self.rewrap_active(&kek)? {
            self.authorize_cleanup()?;
            self.cleanup_legacy_entries()?;
            return Ok(());
        }
        let secrets = self.current_or_legacy_secrets()?;
        self.create_and_activate(&kek, secrets, true)?;
        self.cleanup_legacy_entries()?;
        Ok(())
    }

    pub(crate) fn prepare_passphrase_change(
        &self,
        kek: &super::Kek,
    ) -> Result<(), ProfileKeyRecoveryError> {
        let mut state = self.lock_state();
        let Some(active) = &mut state.active else {
            return Err(ProfileKeyRecoveryError::Corrupt);
        };
        if wrapped_root_matches(kek, &active.wrapped_root, &active.root) {
            return Ok(());
        }
        if let Some(pending) = &active.pending_wrapped_root {
            if wrapped_root_matches(kek, pending, &active.root) {
                return Ok(());
            }
            return Err(ProfileKeyRecoveryError::Corrupt);
        }
        let pending = v1_aead::wrap_master_key_xchacha(kek, &active.root)?;
        let file = encode_file(
            &active.root,
            active.wrapped_root.clone(),
            Some(pending.clone()),
            active.cleanup_authorized,
            &active.removed_names,
            &active.secrets,
        )?;
        self.write_file(&file)?;
        self.verify_file(
            kek,
            &active.secrets,
            &active.removed_names,
            active.cleanup_authorized,
        )?;
        active.pending_wrapped_root = Some(pending);
        Ok(())
    }

    pub(crate) fn finish_passphrase_change(
        &self,
        kek: &super::Kek,
    ) -> Result<(), ProfileKeyRecoveryError> {
        let mut state = self.lock_state();
        let Some(active) = &mut state.active else {
            return Err(ProfileKeyRecoveryError::Corrupt);
        };
        if active.pending_wrapped_root.is_none()
            && wrapped_root_matches(kek, &active.wrapped_root, &active.root)
        {
            return Ok(());
        }
        let Some(pending) = active.pending_wrapped_root.clone() else {
            return Err(ProfileKeyRecoveryError::Corrupt);
        };
        if !wrapped_root_matches(kek, &pending, &active.root) {
            return Err(ProfileKeyRecoveryError::Corrupt);
        }
        let file = encode_file(
            &active.root,
            pending.clone(),
            None,
            active.cleanup_authorized,
            &active.removed_names,
            &active.secrets,
        )?;
        self.write_file(&file)?;
        self.verify_file(
            kek,
            &active.secrets,
            &active.removed_names,
            active.cleanup_authorized,
        )?;
        active.wrapped_root = pending;
        active.pending_wrapped_root = None;
        Ok(())
    }

    fn activate_or_migrate(&self, kek: &super::Kek) -> Result<(), ProfileKeyRecoveryError> {
        if self.file.exists() {
            self.activate_existing(kek)?;
        } else {
            self.create_and_activate(kek, self.read_legacy_secrets()?, false)?;
        }
        if self
            .lock_state()
            .active
            .as_ref()
            .is_some_and(|active| active.cleanup_authorized)
        {
            self.cleanup_legacy_entries()?;
        } else {
            self.lock_state().cleanup_pending = true;
        }
        Ok(())
    }

    fn activate_existing(&self, kek: &super::Kek) -> Result<(), ProfileKeyRecoveryError> {
        let mut file = self.read_file()?;
        let mut root = unwrap_file_root(kek, &file)?;
        let mut payload = decode_payload(&root, &file.encrypted_payload)?;
        let mut merged = false;
        for name in self.managed_names()? {
            if payload.secrets.contains_key(&name) || payload.removed_names.contains(&name) {
                continue;
            }
            let Some(value) = self.backing.get(&name)? else {
                continue;
            };
            if value.len() > MAX_SECRET_BYTES {
                return Err(ProfileKeyRecoveryError::Corrupt);
            }
            payload.secrets.insert(name, value);
            merged = true;
        }
        if merged {
            self.write_file(&encode_file(
                &root,
                file.wrapped_root.clone(),
                file.pending_wrapped_root.clone(),
                payload.cleanup_authorized,
                &payload.removed_names,
                &payload.secrets,
            )?)?;
            file = self.read_file()?;
            root = unwrap_file_root(kek, &file)?;
            let verified = decode_payload(&root, &file.encrypted_payload)?;
            if verified.cleanup_authorized != payload.cleanup_authorized
                || verified.removed_names != payload.removed_names
                || verified.secrets != payload.secrets
            {
                return Err(ProfileKeyRecoveryError::Corrupt);
            }
            payload = verified;
        }
        self.lock_state().active = Some(ActiveVault {
            root,
            wrapped_root: file.wrapped_root,
            pending_wrapped_root: file.pending_wrapped_root,
            cleanup_authorized: payload.cleanup_authorized,
            removed_names: payload.removed_names,
            secrets: payload.secrets,
        });
        Ok(())
    }

    fn create_and_activate(
        &self,
        kek: &super::Kek,
        secrets: BTreeMap<String, Vec<u8>>,
        cleanup_authorized: bool,
    ) -> Result<(), ProfileKeyRecoveryError> {
        let root = MasterKey::generate()?;
        let wrapped_root = v1_aead::wrap_master_key_xchacha(kek, &root)?;
        self.write_file(&encode_file(
            &root,
            wrapped_root.clone(),
            None,
            cleanup_authorized,
            &BTreeSet::new(),
            &secrets,
        )?)?;
        let reread = self.read_file()?;
        let verified_root = match v1_aead::unwrap_master_key_xchacha(kek, &reread.wrapped_root) {
            Ok(root) => root,
            Err(_) => return Err(ProfileKeyRecoveryError::Corrupt),
        };
        let verified = decode_payload(&verified_root, &reread.encrypted_payload)?;
        if verified.secrets != secrets
            || !verified.removed_names.is_empty()
            || verified.cleanup_authorized != cleanup_authorized
        {
            return Err(ProfileKeyRecoveryError::Corrupt);
        }
        self.lock_state().active = Some(ActiveVault {
            root: verified_root,
            wrapped_root: reread.wrapped_root,
            pending_wrapped_root: reread.pending_wrapped_root,
            cleanup_authorized: verified.cleanup_authorized,
            removed_names: verified.removed_names,
            secrets: verified.secrets,
        });
        Ok(())
    }

    fn rewrap_active(&self, kek: &super::Kek) -> Result<bool, ProfileKeyRecoveryError> {
        let mut state = self.lock_state();
        let Some(active) = &mut state.active else {
            return Ok(false);
        };
        let wrapped_root = v1_aead::wrap_master_key_xchacha(kek, &active.root)?;
        let file = encode_file(
            &active.root,
            wrapped_root.clone(),
            None,
            active.cleanup_authorized,
            &active.removed_names,
            &active.secrets,
        )?;
        self.write_file(&file)?;
        active.wrapped_root = wrapped_root;
        active.pending_wrapped_root = None;
        Ok(true)
    }

    fn authorize_cleanup(&self) -> Result<(), ProfileKeyRecoveryError> {
        let mut state = self.lock_state();
        let Some(active) = &mut state.active else {
            return Err(ProfileKeyRecoveryError::Corrupt);
        };
        if active.cleanup_authorized {
            return Ok(());
        }
        let file = encode_file(
            &active.root,
            active.wrapped_root.clone(),
            active.pending_wrapped_root.clone(),
            true,
            &active.removed_names,
            &active.secrets,
        )?;
        self.write_file(&file)?;
        let reread = self.read_file()?;
        let verified = decode_payload(&active.root, &reread.encrypted_payload)?;
        if !verified.cleanup_authorized
            || verified.removed_names != active.removed_names
            || verified.secrets != active.secrets
        {
            return Err(ProfileKeyRecoveryError::Corrupt);
        }
        active.cleanup_authorized = true;
        active.wrapped_root = reread.wrapped_root;
        Ok(())
    }

    fn current_or_legacy_secrets(
        &self,
    ) -> Result<BTreeMap<String, Vec<u8>>, ProfileKeyRecoveryError> {
        if let Some(active) = &self.lock_state().active {
            return Ok(active.secrets.clone());
        }
        self.read_legacy_secrets()
    }

    fn read_legacy_secrets(&self) -> Result<BTreeMap<String, Vec<u8>>, ProfileKeyRecoveryError> {
        let mut secrets = BTreeMap::new();
        for name in self.managed_names()? {
            if let Some(value) = self.backing.get(&name)? {
                if value.len() > MAX_SECRET_BYTES {
                    return Err(ProfileKeyRecoveryError::Corrupt);
                }
                secrets.insert(name, value);
            }
        }
        Ok(secrets)
    }

    fn legacy_material_losses(&self) -> Result<ProfileRecoveryLosses, ProfileKeyRecoveryError> {
        if self.file.exists() {
            return Ok(ProfileRecoveryLosses::default());
        }
        let protected_profile = self
            .paths
            .vault_dir
            .join(".active-space-manifest-v2")
            .try_exists()?;
        let protected_history = self
            .paths
            .vault_dir
            .join("profile-content-key-vault-v1.json")
            .try_exists()?;
        let lifecycle_missing = (protected_profile || protected_history)
            && self.backing.get(PROFILE_LIFECYCLE_MARKER_NAME)?.is_none();
        let local_control_state = protected_profile
            && (lifecycle_missing || self.backing.get(PROFILE_ADMISSION_KEY_NAME)?.is_none());
        let local_history = protected_history
            && (lifecycle_missing || self.backing.get(PROFILE_CONTENT_VAULT_KEY_NAME)?.is_none());
        let current_identity = FileSecureStorage::with_base_dir(self.paths.iroh_identity_dir())
            .get(IDENTITY_STORE_KEY)?;
        let device_identity = (protected_profile || protected_history)
            && self.backing.get(IDENTITY_STORE_KEY)?.is_none()
            && current_identity.is_none();
        Ok(ProfileRecoveryLosses {
            local_history,
            local_control_state,
            device_identity,
        })
    }

    fn cleanup_legacy_entries(&self) -> Result<bool, ProfileKeyRecoveryError> {
        let mut pending = false;
        for name in self.managed_names()? {
            match self.backing.get(&name) {
                Ok(Some(_)) => {
                    if self.backing.delete(&name).is_err() {
                        pending = true;
                        continue;
                    }
                }
                Ok(None) => continue,
                Err(_) => {
                    pending = true;
                    continue;
                }
            }
            if !matches!(self.backing.get(&name), Ok(None)) {
                pending = true;
            }
        }
        self.lock_state().cleanup_pending = pending;
        Ok(pending)
    }

    fn managed_names(&self) -> Result<Vec<String>, ProfileKeyRecoveryError> {
        let mut names = MANAGED_KEYS
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let state_file = self.paths.vault_dir.join(DEFAULT_MIGRATION_STATE_FILE);
        let bytes = match fs::read(&state_file) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(names),
            Err(error) => return Err(error.into()),
        };
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(ProfileKeyRecoveryError::Corrupt);
        }
        if bytes.iter().all(u8::is_ascii_whitespace) {
            return Ok(names);
        }
        let run = match decode_legacy_migration_run_id(&bytes) {
            Ok(run) => run,
            Err(_) => return Err(ProfileKeyRecoveryError::Corrupt),
        };
        if let Some(run) = run {
            names.push(DefaultKeyMigrationAdapter::keyring_name(&run));
        }
        Ok(names)
    }

    fn persist_active(&self, active: &ActiveVault) -> Result<(), SecureStorageError> {
        let file = match encode_file(
            &active.root,
            active.wrapped_root.clone(),
            active.pending_wrapped_root.clone(),
            active.cleanup_authorized,
            &active.removed_names,
            &active.secrets,
        ) {
            Ok(file) => file,
            Err(error) => return Err(SecureStorageError::Other(error.to_string())),
        };
        match self.write_file(&file) {
            Ok(()) => Ok(()),
            Err(error) => Err(SecureStorageError::Other(error.to_string())),
        }
    }

    fn read_file(&self) -> Result<ProfileSecretFile, ProfileKeyRecoveryError> {
        let input = OpenOptions::new().read(true).open(&self.file)?;
        if input.metadata()?.len() > MAX_FILE_BYTES {
            return Err(ProfileKeyRecoveryError::Corrupt);
        }
        let mut bytes = Vec::new();
        input.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(ProfileKeyRecoveryError::Corrupt);
        }
        let file: ProfileSecretFile = match serde_json::from_slice(&bytes) {
            Ok(file) => file,
            Err(_) => return Err(ProfileKeyRecoveryError::Corrupt),
        };
        if file.format_version != FORMAT_VERSION {
            return Err(ProfileKeyRecoveryError::Unsupported);
        }
        Ok(file)
    }

    fn write_file(&self, file: &ProfileSecretFile) -> Result<(), ProfileKeyRecoveryError> {
        fs::create_dir_all(&self.paths.vault_dir)?;
        let bytes = match serde_json::to_vec(file) {
            Ok(bytes) => bytes,
            Err(source) => return Err(ProfileKeyRecoveryError::Storage(source.into())),
        };
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(ProfileKeyRecoveryError::Corrupt);
        }
        let temporary = self.file.with_extension("preparing");
        let mut output = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)?;
        output.write_all(&bytes)?;
        output.sync_all()?;
        replace_file(&temporary, &self.file)?;
        sync_directory(&self.paths.vault_dir)?;
        Ok(())
    }

    fn verify_file(
        &self,
        kek: &super::Kek,
        secrets: &BTreeMap<String, Vec<u8>>,
        removed_names: &BTreeSet<String>,
        cleanup_authorized: bool,
    ) -> Result<(), ProfileKeyRecoveryError> {
        let file = self.read_file()?;
        let root = unwrap_file_root(kek, &file)?;
        let payload = decode_payload(&root, &file.encrypted_payload)?;
        if payload.cleanup_authorized != cleanup_authorized
            || payload.removed_names != *removed_names
            || payload.secrets != *secrets
        {
            return Err(ProfileKeyRecoveryError::Corrupt);
        }
        Ok(())
    }

    fn lock_state(&self) -> MutexGuard<'_, RecoveryState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn activate_from_backing_if_available(&self) -> Result<bool, SecureStorageError> {
        {
            let mut state = self.lock_state();
            if state.active.is_some() {
                if self.file.exists() {
                    return Ok(true);
                }
                if self.backing.get(&self.kek_name)?.is_some() {
                    return Err(SecureStorageError::Corrupt(
                        "profile recovery data disappeared while active".to_owned(),
                    ));
                }
                if let Some(marker) = state
                    .active
                    .as_ref()
                    .and_then(|active| active.secrets.get(PROFILE_LIFECYCLE_MARKER_NAME))
                {
                    self.backing.set(PROFILE_LIFECYCLE_MARKER_NAME, marker)?;
                }
                state.active = None;
                state.cleanup_pending = false;
            }
        }
        if !self.file.exists() {
            return Ok(false);
        }
        let Some(bytes) = self.backing.get(&self.kek_name)? else {
            return Ok(false);
        };
        let kek = match super::Kek::from_bytes(&bytes) {
            Ok(kek) => kek,
            Err(_) => {
                return Err(SecureStorageError::Corrupt(
                    "automatic unlock material is invalid".to_owned(),
                ))
            }
        };
        if self.activate_existing(&kek).is_err() {
            return Err(SecureStorageError::Corrupt(
                "profile recovery data cannot be opened".to_owned(),
            ));
        }
        Ok(true)
    }
}

impl SecureStoragePort for ProfileKeyRecoveryStore {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SecureStorageError> {
        if is_managed_key(key) {
            self.activate_from_backing_if_available()?;
            if let Some(active) = &self.lock_state().active {
                return Ok(active.secrets.get(key).cloned());
            }
        }
        self.backing.get(key)
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<(), SecureStorageError> {
        if is_managed_key(key) {
            self.activate_from_backing_if_available()?;
            let mut state = self.lock_state();
            if let Some(active) = &mut state.active {
                let previous = active.secrets.insert(key.to_owned(), value.to_vec());
                let was_removed = active.removed_names.remove(key);
                if let Err(error) = self.persist_active(active) {
                    match previous {
                        Some(previous) => {
                            active.secrets.insert(key.to_owned(), previous);
                        }
                        None => {
                            active.secrets.remove(key);
                        }
                    }
                    if was_removed {
                        active.removed_names.insert(key.to_owned());
                    }
                    return Err(error);
                }
                return Ok(());
            }
        }
        self.backing.set(key, value)
    }

    fn delete(&self, key: &str) -> Result<(), SecureStorageError> {
        if is_managed_key(key) {
            self.activate_from_backing_if_available()?;
            let mut state = self.lock_state();
            if let Some(active) = &mut state.active {
                let previous = active.secrets.remove(key);
                let newly_removed = active.removed_names.insert(key.to_owned());
                if let Err(error) = self.persist_active(active) {
                    if let Some(previous) = previous {
                        active.secrets.insert(key.to_owned(), previous);
                    }
                    if newly_removed {
                        active.removed_names.remove(key);
                    }
                    return Err(error);
                }
                return Ok(());
            }
        }
        self.backing.delete(key)
    }
}

fn is_managed_key(key: &str) -> bool {
    MANAGED_KEYS.contains(&key) || key.starts_with(KEYRING_PREFIX)
}

fn encode_file(
    root: &MasterKey,
    wrapped_root: EncryptedBlob,
    pending_wrapped_root: Option<EncryptedBlob>,
    cleanup_authorized: bool,
    removed_names: &BTreeSet<String>,
    secrets: &BTreeMap<String, Vec<u8>>,
) -> Result<ProfileSecretFile, ProfileKeyRecoveryError> {
    let payload = match serde_json::to_vec(&ProfileSecretPayload {
        format_version: FORMAT_VERSION,
        cleanup_authorized,
        removed_names: removed_names.clone(),
        secrets: secrets.clone(),
    }) {
        Ok(payload) => payload,
        Err(source) => return Err(ProfileKeyRecoveryError::Storage(source.into())),
    };
    Ok(ProfileSecretFile {
        format_version: FORMAT_VERSION,
        wrapped_root,
        pending_wrapped_root,
        encrypted_payload: v1_aead::encrypt_blob_xchacha(root, &payload, PAYLOAD_AAD)?,
    })
}

fn wrapped_root_matches(kek: &super::Kek, wrapped: &EncryptedBlob, expected: &MasterKey) -> bool {
    v1_aead::unwrap_master_key_xchacha(kek, wrapped).is_ok_and(|root| root == *expected)
}

fn unwrap_file_root(
    kek: &super::Kek,
    file: &ProfileSecretFile,
) -> Result<MasterKey, ProfileKeyRecoveryError> {
    if let Ok(root) = v1_aead::unwrap_master_key_xchacha(kek, &file.wrapped_root) {
        return Ok(root);
    }
    let Some(pending) = &file.pending_wrapped_root else {
        return Err(ProfileKeyRecoveryError::Corrupt);
    };
    match v1_aead::unwrap_master_key_xchacha(kek, pending) {
        Ok(root) => Ok(root),
        Err(_) => Err(ProfileKeyRecoveryError::Corrupt),
    }
}

fn decode_payload(
    root: &MasterKey,
    encrypted: &EncryptedBlob,
) -> Result<ProfileSecretPayload, ProfileKeyRecoveryError> {
    let plaintext = match v1_aead::decrypt_blob_xchacha(
        root,
        &encrypted.nonce,
        &encrypted.ciphertext,
        PAYLOAD_AAD,
    ) {
        Ok(plaintext) => plaintext,
        Err(_) => return Err(ProfileKeyRecoveryError::Corrupt),
    };
    let payload: ProfileSecretPayload = match serde_json::from_slice(&plaintext) {
        Ok(payload) => payload,
        Err(_) => return Err(ProfileKeyRecoveryError::Corrupt),
    };
    if payload.format_version != FORMAT_VERSION
        || payload
            .secrets
            .keys()
            .any(|key| !is_managed_key(key.as_str()))
        || payload
            .removed_names
            .iter()
            .any(|key| !is_managed_key(key.as_str()) || payload.secrets.contains_key(key))
    {
        return Err(ProfileKeyRecoveryError::Corrupt);
    }
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use uc_core::crypto::domain::Passphrase;
    use uc_core::crypto::model::Passphrase as LegacyPassphrase;
    use uc_core::ports::{SecureStorageError, SecureStoragePort};

    use super::*;
    use crate::security::crypto_model::{KeySlot, WrappedMasterKey};

    const UPGRADE_BACKUP_KEY: &str = "profile_upgrade_backup_record_key:v1";

    fn test_paths(directory: &tempfile::TempDir) -> AppPaths {
        AppPaths::with_base_data_local_dir(directory.path().to_path_buf())
    }

    #[derive(Default)]
    struct MemoryStorage {
        values: Mutex<BTreeMap<String, Vec<u8>>>,
        fail_delete: AtomicBool,
    }

    impl SecureStoragePort for MemoryStorage {
        fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SecureStorageError> {
            Ok(self.values.lock().unwrap().get(key).cloned())
        }

        fn set(&self, key: &str, value: &[u8]) -> Result<(), SecureStorageError> {
            self.values
                .lock()
                .unwrap()
                .insert(key.to_owned(), value.to_vec());
            Ok(())
        }

        fn delete(&self, key: &str) -> Result<(), SecureStorageError> {
            if self.fail_delete.load(Ordering::Acquire) {
                return Err(SecureStorageError::Unavailable(
                    "delete temporarily unavailable".to_owned(),
                ));
            }
            self.values.lock().unwrap().remove(key);
            Ok(())
        }
    }

    async fn active_recovery_fixture() -> (
        tempfile::TempDir,
        Arc<MemoryStorage>,
        AppPaths,
        String,
        ProfileKeyRecoveryStore,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let storage = Arc::new(MemoryStorage::default());
        let backing: Arc<dyn SecureStoragePort> = storage.clone();
        let paths = test_paths(&directory);
        let profile_id = uc_core::ids::ProfileId::new().into_inner();
        let scope = KeyScope {
            profile_id: profile_id.clone(),
        };
        let material = KeyMaterialStore::new(
            Arc::clone(&backing),
            Arc::new(JsonKeySlotStore::new(paths.vault_dir.clone())),
        );
        let draft = KeySlot::draft_v1(scope.clone()).unwrap();
        let legacy = LegacyPassphrase("migration passphrase".to_owned());
        let kek = v1_aead::derive_kek_argon2id(&legacy, &draft.salt, &draft.kdf).unwrap();
        let master = MasterKey::generate().unwrap();
        let wrapped = v1_aead::wrap_master_key_xchacha(&kek, &master).unwrap();
        material
            .store_keyslot(&draft.finalize(WrappedMasterKey { blob: wrapped }))
            .await
            .unwrap();
        material.store_kek(&scope, &kek).await.unwrap();
        storage
            .set(PROFILE_ADMISSION_KEY_NAME, &[0x41; 32])
            .unwrap();
        let recovery = ProfileKeyRecoveryStore::new(paths.clone(), profile_id.clone(), backing);
        assert_eq!(
            recovery.prepare_startup().await.unwrap(),
            ProfileRecoveryPreparation::Ready
        );
        (directory, storage, paths, profile_id, recovery)
    }

    #[test]
    fn recovery_error_conversion_preserves_failure_classes() {
        assert!(matches!(
            ProfileKeyRecoveryError::from(SecureStorageError::Unavailable("storage".to_owned())),
            ProfileKeyRecoveryError::Storage(_)
        ));
        assert!(matches!(
            ProfileKeyRecoveryError::from(std::io::Error::other("io")),
            ProfileKeyRecoveryError::Storage(_)
        ));
        assert!(matches!(
            ProfileKeyRecoveryError::from(EncryptionError::WrongPassphrase),
            ProfileKeyRecoveryError::WrongPassphrase
        ));
        assert!(matches!(
            ProfileKeyRecoveryError::from(EncryptionError::UnsupportedKdfAlgorithm),
            ProfileKeyRecoveryError::Unsupported
        ));
        assert!(matches!(
            ProfileKeyRecoveryError::from(EncryptionError::CorruptedKeySlot),
            ProfileKeyRecoveryError::Corrupt
        ));
        assert!(matches!(
            ProfileKeyRecoveryError::from(EncryptionError::KeyNotFound),
            ProfileKeyRecoveryError::Storage(_)
        ));
        assert!(matches!(
            ProfileKeyRecoveryError::from(v1_aead::AeadError::DecryptFailed),
            ProfileKeyRecoveryError::Storage(_)
        ));
    }

    #[tokio::test]
    async fn legacy_secrets_are_verified_before_their_keyring_entries_are_removed() {
        let directory = tempfile::tempdir().unwrap();
        let storage = Arc::new(MemoryStorage::default());
        let backing: Arc<dyn SecureStoragePort> = storage.clone();
        let paths = test_paths(&directory);
        let profile_id = uc_core::ids::ProfileId::new().into_inner();
        let scope = KeyScope {
            profile_id: profile_id.clone(),
        };
        let material = KeyMaterialStore::new(
            Arc::clone(&backing),
            Arc::new(JsonKeySlotStore::new(paths.vault_dir.clone())),
        );
        let draft = KeySlot::draft_v1(scope.clone()).unwrap();
        let legacy = LegacyPassphrase("migration passphrase".to_owned());
        let kek = v1_aead::derive_kek_argon2id(&legacy, &draft.salt, &draft.kdf).unwrap();
        let master = MasterKey::generate().unwrap();
        let wrapped = v1_aead::wrap_master_key_xchacha(&kek, &master).unwrap();
        material
            .store_keyslot(&draft.finalize(WrappedMasterKey { blob: wrapped }))
            .await
            .unwrap();
        material.store_kek(&scope, &kek).await.unwrap();

        let expected = MANAGED_KEYS
            .into_iter()
            .enumerate()
            .map(|(index, name)| (name.to_owned(), vec![index as u8 + 1; 32]))
            .collect::<BTreeMap<_, _>>();
        for (name, value) in &expected {
            storage.set(name, value).unwrap();
        }

        let recovery = ProfileKeyRecoveryStore::new(paths.clone(), profile_id.clone(), backing);
        assert_eq!(
            recovery.prepare_startup().await.unwrap(),
            ProfileRecoveryPreparation::Ready
        );
        assert!(recovery.vault_file().is_file());
        let persisted = std::fs::read(recovery.vault_file()).unwrap();
        for (name, value) in &expected {
            assert!(!persisted
                .windows(name.len())
                .any(|bytes| bytes == name.as_bytes()));
            assert!(!persisted.windows(value.len()).any(|bytes| bytes == value));
        }
        assert!(recovery.cleanup_pending());
        for name in expected.keys() {
            assert!(storage.get(name).unwrap().is_some());
        }

        storage.fail_delete.store(true, Ordering::Release);
        assert_eq!(
            recovery
                .recover(&Passphrase::new("migration passphrase"))
                .await
                .unwrap(),
            ProfileRecoveryOutcome::Ready
        );
        assert!(recovery.cleanup_pending());
        for name in expected.keys() {
            assert!(storage.get(name).unwrap().is_some());
        }

        storage.fail_delete.store(false, Ordering::Release);
        let restarted = ProfileKeyRecoveryStore::new(paths, profile_id, storage.clone());
        assert_eq!(
            restarted.prepare_startup().await.unwrap(),
            ProfileRecoveryPreparation::Ready
        );
        assert!(!restarted.cleanup_pending());
        for (name, value) in expected {
            assert_eq!(restarted.get(&name).unwrap(), Some(value));
            assert!(storage.get(&name).unwrap().is_none());
        }
    }

    #[tokio::test]
    async fn automatic_migration_stops_when_protected_history_has_lost_legacy_material() {
        let directory = tempfile::tempdir().unwrap();
        let storage = Arc::new(MemoryStorage::default());
        let backing: Arc<dyn SecureStoragePort> = storage.clone();
        let paths = test_paths(&directory);
        let profile_id = uc_core::ids::ProfileId::new().into_inner();
        let scope = KeyScope {
            profile_id: profile_id.clone(),
        };
        let material = KeyMaterialStore::new(
            Arc::clone(&backing),
            Arc::new(JsonKeySlotStore::new(paths.vault_dir.clone())),
        );
        let draft = KeySlot::draft_v1(scope.clone()).unwrap();
        let legacy = LegacyPassphrase("migration passphrase".to_owned());
        let kek = v1_aead::derive_kek_argon2id(&legacy, &draft.salt, &draft.kdf).unwrap();
        let master = MasterKey::generate().unwrap();
        let wrapped = v1_aead::wrap_master_key_xchacha(&kek, &master).unwrap();
        material
            .store_keyslot(&draft.finalize(WrappedMasterKey { blob: wrapped }))
            .await
            .unwrap();
        material.store_kek(&scope, &kek).await.unwrap();
        storage
            .set(PROFILE_LIFECYCLE_MARKER_NAME, &[0x31; 32])
            .unwrap();
        storage.set(IDENTITY_STORE_KEY, &[0x32; 32]).unwrap();
        std::fs::create_dir_all(&paths.vault_dir).unwrap();
        std::fs::write(
            paths.vault_dir.join("profile-content-key-vault-v1.json"),
            b"protected history marker",
        )
        .unwrap();

        let recovery = ProfileKeyRecoveryStore::new(paths, profile_id, backing);
        assert_eq!(
            recovery.prepare_startup().await.unwrap(),
            ProfileRecoveryPreparation::AwaitingPassphrase {
                losses: ProfileRecoveryLosses {
                    local_history: true,
                    local_control_state: false,
                    device_identity: false,
                },
            }
        );
        assert!(!recovery.vault_file().exists());
    }

    #[tokio::test]
    async fn current_file_identity_is_not_reported_as_lost_legacy_material() {
        let directory = tempfile::tempdir().unwrap();
        let storage = Arc::new(MemoryStorage::default());
        let backing: Arc<dyn SecureStoragePort> = storage.clone();
        let paths = test_paths(&directory);
        let profile_id = uc_core::ids::ProfileId::new().into_inner();
        let scope = KeyScope {
            profile_id: profile_id.clone(),
        };
        let material = KeyMaterialStore::new(
            Arc::clone(&backing),
            Arc::new(JsonKeySlotStore::new(paths.vault_dir.clone())),
        );
        let draft = KeySlot::draft_v1(scope.clone()).unwrap();
        let legacy = LegacyPassphrase("migration passphrase".to_owned());
        let kek = v1_aead::derive_kek_argon2id(&legacy, &draft.salt, &draft.kdf).unwrap();
        let master = MasterKey::generate().unwrap();
        let wrapped = v1_aead::wrap_master_key_xchacha(&kek, &master).unwrap();
        material
            .store_keyslot(&draft.finalize(WrappedMasterKey { blob: wrapped }))
            .await
            .unwrap();
        material.store_kek(&scope, &kek).await.unwrap();
        storage
            .set(PROFILE_LIFECYCLE_MARKER_NAME, &[0x31; 32])
            .unwrap();
        storage
            .set(PROFILE_ADMISSION_KEY_NAME, &[0x32; 32])
            .unwrap();
        storage
            .set(PROFILE_CONTENT_VAULT_KEY_NAME, &[0x33; 32])
            .unwrap();
        std::fs::create_dir_all(&paths.vault_dir).unwrap();
        std::fs::write(
            paths.vault_dir.join("profile-content-key-vault-v1.json"),
            b"protected history marker",
        )
        .unwrap();
        FileSecureStorage::with_base_dir(paths.iroh_identity_dir())
            .set(IDENTITY_STORE_KEY, &[0x34; 32])
            .unwrap();

        let recovery = ProfileKeyRecoveryStore::new(paths, profile_id, backing);
        assert_eq!(
            recovery.prepare_startup().await.unwrap(),
            ProfileRecoveryPreparation::Ready
        );
    }

    #[tokio::test]
    async fn existing_vault_merges_upgrade_backup_key_before_resumable_cleanup() {
        let directory = tempfile::tempdir().unwrap();
        let storage = Arc::new(MemoryStorage::default());
        let backing: Arc<dyn SecureStoragePort> = storage.clone();
        let paths = test_paths(&directory);
        let profile_id = uc_core::ids::ProfileId::new().into_inner();
        let scope = KeyScope {
            profile_id: profile_id.clone(),
        };
        let material = KeyMaterialStore::new(
            Arc::clone(&backing),
            Arc::new(JsonKeySlotStore::new(paths.vault_dir.clone())),
        );
        let draft = KeySlot::draft_v1(scope.clone()).unwrap();
        let legacy = LegacyPassphrase("migration passphrase".to_owned());
        let kek = v1_aead::derive_kek_argon2id(&legacy, &draft.salt, &draft.kdf).unwrap();
        let master = MasterKey::generate().unwrap();
        let wrapped = v1_aead::wrap_master_key_xchacha(&kek, &master).unwrap();
        material
            .store_keyslot(&draft.finalize(WrappedMasterKey { blob: wrapped }))
            .await
            .unwrap();
        material.store_kek(&scope, &kek).await.unwrap();

        for (index, name) in [
            PROFILE_ADMISSION_KEY_NAME,
            PROFILE_CONTENT_VAULT_KEY_NAME,
            PROFILE_LIFECYCLE_MARKER_NAME,
            IDENTITY_STORE_KEY,
        ]
        .into_iter()
        .enumerate()
        {
            storage.set(name, &[index as u8 + 1; 32]).unwrap();
        }
        let original =
            ProfileKeyRecoveryStore::new(paths.clone(), profile_id.clone(), Arc::clone(&backing));
        assert_eq!(
            original.prepare_startup().await.unwrap(),
            ProfileRecoveryPreparation::Ready
        );
        assert_eq!(
            original
                .recover(&Passphrase::new("migration passphrase"))
                .await
                .unwrap(),
            ProfileRecoveryOutcome::Ready
        );
        let recovery_file = original.vault_file().to_owned();
        drop(original);

        let expected = vec![9; 32];
        storage.set(UPGRADE_BACKUP_KEY, &expected).unwrap();
        let interrupted_path = recovery_file.with_extension("preparing");
        std::fs::create_dir(&interrupted_path).unwrap();
        let interrupted =
            ProfileKeyRecoveryStore::new(paths.clone(), profile_id.clone(), Arc::clone(&backing));
        assert!(interrupted.prepare_startup().await.is_err());
        assert_eq!(
            storage.get(UPGRADE_BACKUP_KEY).unwrap(),
            Some(expected.clone())
        );
        std::fs::remove_dir(&interrupted_path).unwrap();

        storage.fail_delete.store(true, Ordering::Release);
        let pending_cleanup =
            ProfileKeyRecoveryStore::new(paths.clone(), profile_id.clone(), Arc::clone(&backing));
        assert_eq!(
            pending_cleanup.prepare_startup().await.unwrap(),
            ProfileRecoveryPreparation::Ready
        );
        assert_eq!(
            pending_cleanup.get(UPGRADE_BACKUP_KEY).unwrap(),
            Some(expected.clone())
        );
        assert!(pending_cleanup.cleanup_pending());
        assert_eq!(
            storage.get(UPGRADE_BACKUP_KEY).unwrap(),
            Some(expected.clone())
        );
        drop(pending_cleanup);

        storage.fail_delete.store(false, Ordering::Release);
        let completed = ProfileKeyRecoveryStore::new(paths, profile_id, storage.clone());
        assert_eq!(
            completed.prepare_startup().await.unwrap(),
            ProfileRecoveryPreparation::Ready
        );
        assert_eq!(completed.get(UPGRADE_BACKUP_KEY).unwrap(), Some(expected));
        assert!(storage.get(UPGRADE_BACKUP_KEY).unwrap().is_none());
        assert!(!completed.cleanup_pending());
    }

    #[tokio::test]
    async fn managed_mutations_restore_memory_when_the_recovery_file_cannot_be_replaced() {
        let (_directory, _storage, _paths, _profile_id, recovery) = active_recovery_fixture().await;
        let original = recovery.get(PROFILE_ADMISSION_KEY_NAME).unwrap().unwrap();
        let interrupted = recovery.vault_file().with_extension("preparing");
        std::fs::create_dir(&interrupted).unwrap();

        assert!(recovery
            .set(PROFILE_ADMISSION_KEY_NAME, &[0x52; 32])
            .is_err());
        assert_eq!(
            recovery.get(PROFILE_ADMISSION_KEY_NAME).unwrap(),
            Some(original.clone())
        );
        assert!(recovery.delete(PROFILE_ADMISSION_KEY_NAME).is_err());
        assert_eq!(
            recovery.get(PROFILE_ADMISSION_KEY_NAME).unwrap(),
            Some(original)
        );
    }

    #[tokio::test]
    async fn authorized_vault_delete_is_not_undone_by_stale_legacy_storage() {
        let (_directory, storage, paths, profile_id, recovery) = active_recovery_fixture().await;
        recovery
            .recover(&Passphrase::new("migration passphrase"))
            .await
            .unwrap();
        storage
            .set(PROFILE_ADMISSION_KEY_NAME, &[0x65; 32])
            .unwrap();

        recovery.delete(PROFILE_ADMISSION_KEY_NAME).unwrap();
        drop(recovery);

        let restarted =
            ProfileKeyRecoveryStore::new(paths.clone(), profile_id.clone(), storage.clone());
        assert_eq!(
            restarted.prepare_startup().await.unwrap(),
            ProfileRecoveryPreparation::Ready
        );
        assert_eq!(restarted.get(PROFILE_ADMISSION_KEY_NAME).unwrap(), None);
        assert_eq!(storage.get(PROFILE_ADMISSION_KEY_NAME).unwrap(), None);

        storage
            .set(PROFILE_ADMISSION_KEY_NAME, &[0x66; 32])
            .unwrap();
        let interrupted = restarted.vault_file().with_extension("preparing");
        std::fs::create_dir(&interrupted).unwrap();
        assert!(restarted
            .set(PROFILE_ADMISSION_KEY_NAME, &[0x67; 32])
            .is_err());
        std::fs::remove_dir(&interrupted).unwrap();
        drop(restarted);

        let final_restart = ProfileKeyRecoveryStore::new(paths, profile_id, storage.clone());
        assert_eq!(
            final_restart.prepare_startup().await.unwrap(),
            ProfileRecoveryPreparation::Ready
        );
        assert_eq!(final_restart.get(PROFILE_ADMISSION_KEY_NAME).unwrap(), None);
        assert_eq!(storage.get(PROFILE_ADMISSION_KEY_NAME).unwrap(), None);
    }

    #[tokio::test]
    async fn active_recovery_fails_closed_when_its_file_disappears() {
        let (_directory, _storage, _paths, _profile_id, recovery) = active_recovery_fixture().await;
        std::fs::remove_file(recovery.vault_file()).unwrap();

        assert!(matches!(
            recovery.get(PROFILE_ADMISSION_KEY_NAME),
            Err(SecureStorageError::Corrupt(_))
        ));
    }

    #[tokio::test]
    async fn automatic_activation_rejects_invalid_kek_and_unopenable_recovery_file() {
        let (_directory, storage, paths, profile_id, recovery) = active_recovery_fixture().await;
        let kek_name = format!("kek:v1:profile:{profile_id}");
        drop(recovery);
        storage.set(&kek_name, &[0x61; 3]).unwrap();
        let invalid_kek =
            ProfileKeyRecoveryStore::new(paths.clone(), profile_id.clone(), storage.clone());
        assert!(matches!(
            invalid_kek.get(PROFILE_ADMISSION_KEY_NAME),
            Err(SecureStorageError::Corrupt(_))
        ));

        storage.set(&kek_name, &[0x62; 32]).unwrap();
        let unopenable = ProfileKeyRecoveryStore::new(paths, profile_id, storage);
        assert!(matches!(
            unopenable.get(PROFILE_ADMISSION_KEY_NAME),
            Err(SecureStorageError::Corrupt(_))
        ));
    }

    #[test]
    fn passphrase_change_file_survives_restart_at_each_persisted_phase() {
        let directory = tempfile::tempdir().unwrap();
        let storage: Arc<dyn SecureStoragePort> = Arc::new(MemoryStorage::default());
        let profile_id = uc_core::ids::ProfileId::new().into_inner();
        let paths = test_paths(&directory);
        let old_kek = Kek::from_bytes(&[0x51; 32]).unwrap();
        let new_kek = Kek::from_bytes(&[0x52; 32]).unwrap();
        let secrets = BTreeMap::from([(PROFILE_ADMISSION_KEY_NAME.to_owned(), vec![0x53; 32])]);

        let original =
            ProfileKeyRecoveryStore::new(paths.clone(), profile_id.clone(), Arc::clone(&storage));
        original
            .create_and_activate(&old_kek, secrets.clone(), true)
            .unwrap();
        original.prepare_passphrase_change(&new_kek).unwrap();
        drop(original);

        let before_keyslot_change =
            ProfileKeyRecoveryStore::new(paths.clone(), profile_id.clone(), Arc::clone(&storage));
        before_keyslot_change.activate_existing(&old_kek).unwrap();
        drop(before_keyslot_change);

        let after_keyslot_change =
            ProfileKeyRecoveryStore::new(paths.clone(), profile_id.clone(), Arc::clone(&storage));
        after_keyslot_change.activate_existing(&new_kek).unwrap();
        after_keyslot_change
            .finish_passphrase_change(&new_kek)
            .unwrap();
        drop(after_keyslot_change);

        let completed = ProfileKeyRecoveryStore::new(paths, profile_id, storage);
        assert!(completed.activate_existing(&old_kek).is_err());
        completed.activate_existing(&new_kek).unwrap();
        assert_eq!(
            completed.get(PROFILE_ADMISSION_KEY_NAME).unwrap(),
            Some(vec![0x53; 32])
        );
    }

    #[test]
    fn suspended_vault_reopens_from_persisted_material() {
        let directory = tempfile::tempdir().unwrap();
        let storage = Arc::new(MemoryStorage::default());
        let backing: Arc<dyn SecureStoragePort> = storage.clone();
        let profile_id = uc_core::ids::ProfileId::new().into_inner();
        let paths = test_paths(&directory);
        let kek = Kek::from_bytes(&[0x61; 32]).unwrap();
        let expected = vec![0x62; 32];
        let recovery = ProfileKeyRecoveryStore::new(paths, profile_id, backing);
        recovery
            .create_and_activate(
                &kek,
                BTreeMap::from([(PROFILE_CONTENT_VAULT_KEY_NAME.to_owned(), expected.clone())]),
                true,
            )
            .unwrap();
        storage.set(&recovery.kek_name, kek.as_bytes()).unwrap();

        recovery.suspend();

        assert_eq!(
            recovery.get(PROFILE_CONTENT_VAULT_KEY_NAME).unwrap(),
            Some(expected)
        );
    }
}
