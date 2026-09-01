use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use uc_application::deps::{
    CurrentSpaceIdentityError, CurrentSpaceIdentityPort, InitialSpaceActivationPort,
    PortableCurrentSpaceIdentityPort,
};
use uc_core::ids::SpaceId;

use crate::security::{
    ActiveRuntimeManifest, ActiveSpaceGenerationManifestStore,
    ActiveSpaceGenerationManifestStoreError, AdmissionKeyError, AdmissionKeyManager,
};

const LEGACY_ID_PURPOSE: &[u8] = b"legacy-current-space-id-v1";
const LEGACY_ID_FORMAT_VERSION: u16 = 1;

#[derive(serde::Serialize, serde::Deserialize)]
struct PersistedLegacyCurrentSpaceIdV1 {
    format_version: u16,
    space_id: String,
}

struct EncryptedLegacyCurrentSpaceIdStore {
    path: PathBuf,
    keys: Arc<AdmissionKeyManager>,
    write_lock: Mutex<()>,
}

impl EncryptedLegacyCurrentSpaceIdStore {
    fn new(path: PathBuf, keys: Arc<AdmissionKeyManager>) -> Self {
        Self {
            path,
            keys,
            write_lock: Mutex::new(()),
        }
    }

    async fn load(&self) -> Result<Option<SpaceId>, CurrentSpaceIdentityError> {
        let ciphertext = match fs::read(&self.path).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(CurrentSpaceIdentityError::Unavailable),
        };
        let plaintext = self
            .keys
            .open_profile_payload(LEGACY_ID_PURPOSE, &ciphertext)
            .map_err(map_key_error)?;
        let state: PersistedLegacyCurrentSpaceIdV1 = postcard::from_bytes(&plaintext)
            .map_err(|_| CurrentSpaceIdentityError::Inconsistent)?;
        if state.format_version != LEGACY_ID_FORMAT_VERSION || state.space_id.is_empty() {
            return Err(CurrentSpaceIdentityError::Inconsistent);
        }
        Ok(Some(SpaceId::from_str(&state.space_id)))
    }

    async fn activate(&self, space_id: &SpaceId) -> Result<(), CurrentSpaceIdentityError> {
        let _guard = self.write_lock.lock().await;
        if let Some(current) = self.load().await? {
            return (current == *space_id)
                .then_some(())
                .ok_or(CurrentSpaceIdentityError::Inconsistent);
        }
        let state = PersistedLegacyCurrentSpaceIdV1 {
            format_version: LEGACY_ID_FORMAT_VERSION,
            space_id: space_id.as_str().to_owned(),
        };
        let plaintext =
            postcard::to_stdvec(&state).map_err(|_| CurrentSpaceIdentityError::Inconsistent)?;
        let ciphertext = self
            .keys
            .seal_profile_payload(LEGACY_ID_PURPOSE, &plaintext)
            .map_err(map_key_error)?;
        let parent = self
            .path
            .parent()
            .ok_or(CurrentSpaceIdentityError::Unavailable)?;
        fs::create_dir_all(parent)
            .await
            .map_err(|_| CurrentSpaceIdentityError::Unavailable)?;
        let mut file = fs::File::create(&self.path)
            .await
            .map_err(|_| CurrentSpaceIdentityError::Unavailable)?;
        file.write_all(&ciphertext)
            .await
            .map_err(|_| CurrentSpaceIdentityError::Unavailable)?;
        file.sync_all()
            .await
            .map_err(|_| CurrentSpaceIdentityError::Unavailable)
    }

    async fn replace(&self, space_id: &SpaceId) -> Result<(), CurrentSpaceIdentityError> {
        let _guard = self.write_lock.lock().await;
        let state = PersistedLegacyCurrentSpaceIdV1 {
            format_version: LEGACY_ID_FORMAT_VERSION,
            space_id: space_id.as_str().to_owned(),
        };
        let plaintext =
            postcard::to_stdvec(&state).map_err(|_| CurrentSpaceIdentityError::Inconsistent)?;
        let ciphertext = self
            .keys
            .seal_profile_payload(LEGACY_ID_PURPOSE, &plaintext)
            .map_err(map_key_error)?;
        let parent = self
            .path
            .parent()
            .ok_or(CurrentSpaceIdentityError::Unavailable)?;
        fs::create_dir_all(parent)
            .await
            .map_err(|_| CurrentSpaceIdentityError::Unavailable)?;
        let mut file = fs::File::create(&self.path)
            .await
            .map_err(|_| CurrentSpaceIdentityError::Unavailable)?;
        file.write_all(&ciphertext)
            .await
            .map_err(|_| CurrentSpaceIdentityError::Unavailable)?;
        file.sync_all()
            .await
            .map_err(|_| CurrentSpaceIdentityError::Unavailable)
    }
}

pub struct CurrentSpaceResolver {
    generation_manifest: Arc<ActiveSpaceGenerationManifestStore>,
    legacy_id: EncryptedLegacyCurrentSpaceIdStore,
}

impl CurrentSpaceResolver {
    pub fn new(
        generation_manifest: Arc<ActiveSpaceGenerationManifestStore>,
        legacy_id_path: PathBuf,
        keys: Arc<AdmissionKeyManager>,
    ) -> Self {
        Self {
            generation_manifest,
            legacy_id: EncryptedLegacyCurrentSpaceIdStore::new(legacy_id_path, keys),
        }
    }
}

#[async_trait]
impl CurrentSpaceIdentityPort for CurrentSpaceResolver {
    async fn current_space_id(&self) -> Result<Option<SpaceId>, CurrentSpaceIdentityError> {
        match self
            .generation_manifest
            .load_runtime()
            .await
            .map_err(map_generation_manifest_error)?
        {
            Some(ActiveRuntimeManifest::V2(manifest)) => {
                Ok(Some(SpaceId::from_str(&manifest.space_id)))
            }
            Some(ActiveRuntimeManifest::V3(manifest)) => {
                Ok(Some(manifest.layout().space_id().clone()))
            }
            None => self.legacy_id.load().await,
        }
    }

    async fn requires_legacy_profile_isolation(&self) -> Result<bool, CurrentSpaceIdentityError> {
        if self
            .generation_manifest
            .load_runtime()
            .await
            .map_err(map_generation_manifest_error)?
            .is_some()
        {
            return Ok(false);
        }
        Ok(self.legacy_id.load().await?.is_some())
    }
}

#[async_trait]
impl InitialSpaceActivationPort for CurrentSpaceResolver {
    async fn activate_initial_space(
        &self,
        space_id: &SpaceId,
    ) -> Result<(), CurrentSpaceIdentityError> {
        if self
            .generation_manifest
            .load_runtime()
            .await
            .map_err(map_generation_manifest_error)?
            .is_some()
        {
            return Err(CurrentSpaceIdentityError::Inconsistent);
        }
        self.legacy_id.activate(space_id).await
    }
}

#[async_trait]
impl PortableCurrentSpaceIdentityPort for CurrentSpaceResolver {
    async fn prepare_portable_identity(&self) -> Result<(), CurrentSpaceIdentityError> {
        let space_id = self
            .current_space_id()
            .await?
            .ok_or(CurrentSpaceIdentityError::Inconsistent)?;
        self.legacy_id.replace(&space_id).await
    }
}

#[cfg(not(windows))]
fn sync_parent_directory(parent: &std::path::Path) -> std::io::Result<()> {
    std::fs::File::open(parent)?.sync_all()
}

#[cfg(windows)]
fn sync_parent_directory(_parent: &std::path::Path) -> std::io::Result<()> {
    Ok(())
}

fn map_generation_manifest_error(
    error: ActiveSpaceGenerationManifestStoreError,
) -> CurrentSpaceIdentityError {
    match error {
        ActiveSpaceGenerationManifestStoreError::Storage => CurrentSpaceIdentityError::Unavailable,
        ActiveSpaceGenerationManifestStoreError::Corrupt
        | ActiveSpaceGenerationManifestStoreError::UnsupportedVersion => {
            CurrentSpaceIdentityError::Inconsistent
        }
    }
}

fn map_key_error(error: AdmissionKeyError) -> CurrentSpaceIdentityError {
    match error {
        AdmissionKeyError::SecureStorage => CurrentSpaceIdentityError::Unavailable,
        AdmissionKeyError::Corrupt | AdmissionKeyError::OpenFailed => {
            CurrentSpaceIdentityError::Inconsistent
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex as StdMutex;

    use uc_core::membership::{ActiveRuntimeLayout, ActiveSpaceGenerationManifestV2};
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

    fn resolver(
        directory: &tempfile::TempDir,
    ) -> (
        CurrentSpaceResolver,
        Arc<ActiveSpaceGenerationManifestStore>,
    ) {
        let keys = Arc::new(AdmissionKeyManager::new(
            Arc::new(MemorySecureStorage::default()),
            [0x71; 16],
        ));
        let generation_manifest = Arc::new(ActiveSpaceGenerationManifestStore::new(
            directory.path().to_path_buf(),
            Arc::clone(&keys),
        ));
        (
            CurrentSpaceResolver::new(
                Arc::clone(&generation_manifest),
                directory.path().join("legacy-current-space"),
                keys,
            ),
            generation_manifest,
        )
    }

    #[tokio::test]
    async fn initial_activation_is_idempotent_for_the_same_space() {
        let directory = tempfile::tempdir().unwrap();
        let (resolver, _) = resolver(&directory);
        let space_id = SpaceId::from_str("space-a");

        resolver.activate_initial_space(&space_id).await.unwrap();
        resolver.activate_initial_space(&space_id).await.unwrap();

        assert_eq!(resolver.current_space_id().await.unwrap(), Some(space_id));
    }

    #[tokio::test]
    async fn generation_manifest_takes_precedence_over_legacy_identity() {
        let directory = tempfile::tempdir().unwrap();
        let (resolver, generation_manifest) = resolver(&directory);
        resolver
            .activate_initial_space(&SpaceId::from_str("legacy-space"))
            .await
            .unwrap();
        generation_manifest
            .promote(
                &ActiveSpaceGenerationManifestV2::new(
                    "generated-space".to_owned(),
                    [0x72; 16],
                    [0x73; 16],
                    [0x74; 16],
                )
                .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            resolver.current_space_id().await.unwrap(),
            Some(SpaceId::from_str("generated-space"))
        );
    }

    #[tokio::test]
    async fn v3_generation_manifest_remains_the_current_space_identity() {
        let directory = tempfile::tempdir().unwrap();
        let (resolver, generation_manifest) = resolver(&directory);
        let source = ActiveSpaceGenerationManifestV2::new(
            "generated-space".to_owned(),
            [0x72; 16],
            [0x73; 16],
            [0x74; 16],
        )
        .unwrap();
        generation_manifest.promote(&source).await.unwrap();
        let target = crate::security::ActiveRuntimeManifestV3::new(
            ActiveRuntimeLayout::new(SpaceId::from_str("generated-space"), [0x75; 16], [0x76; 16])
                .unwrap(),
            [0x72; 16],
        )
        .unwrap();
        generation_manifest
            .promote_v3_from_v2(&source, &target)
            .await
            .unwrap();

        assert_eq!(
            resolver.current_space_id().await.unwrap(),
            Some(SpaceId::from_str("generated-space"))
        );
        assert!(!resolver.requires_legacy_profile_isolation().await.unwrap());
    }

    #[tokio::test]
    async fn corrupt_generation_manifest_never_falls_back_to_legacy_identity() {
        let directory = tempfile::tempdir().unwrap();
        let (resolver, _) = resolver(&directory);
        resolver
            .activate_initial_space(&SpaceId::from_str("legacy-space"))
            .await
            .unwrap();
        fs::write(
            directory.path().join(".active-space-manifest-v2"),
            b"invalid",
        )
        .await
        .unwrap();

        assert_eq!(
            resolver.current_space_id().await.unwrap_err(),
            CurrentSpaceIdentityError::Inconsistent
        );
    }

    #[tokio::test]
    async fn portable_identity_materializes_the_generated_space_id() {
        let directory = tempfile::tempdir().unwrap();
        let (resolver, generation_manifest) = resolver(&directory);
        resolver
            .activate_initial_space(&SpaceId::from_str("stale-legacy-space"))
            .await
            .unwrap();
        generation_manifest
            .promote(
                &ActiveSpaceGenerationManifestV2::new(
                    "generated-space".to_owned(),
                    [0x78; 16],
                    [0x79; 16],
                    [0x7a; 16],
                )
                .unwrap(),
            )
            .await
            .unwrap();

        resolver.prepare_portable_identity().await.unwrap();
        generation_manifest.clear().await.unwrap();

        assert_eq!(
            resolver.current_space_id().await.unwrap(),
            Some(SpaceId::from_str("generated-space"))
        );
    }
}
