use std::sync::Arc;

use async_trait::async_trait;
use uc_application::deps::{CurrentSpaceIdentityError, InitialSpaceActivationPort};
use uc_core::ids::SpaceId;
use uc_core::membership::ActiveRuntimeLayout;

use super::active_space_generation_manifest_store::V3ManifestPromotionOutcome;
use super::{
    ActiveRuntimeManifestV3, ActiveSpaceGenerationManifestStore,
    ActiveSpaceGenerationManifestStoreError,
};

const INITIAL_KEYSLOT_GENERATION_DOMAIN: &[u8] = b"uniclipboard/v3-initial-keyslot-generation/v1\0";

/// 把 Fresh profile 已准备的双 generation 作为首个 V3 runtime 原子公开。
pub struct V3InitialSpaceActivation {
    profile_data_generation: [u8; 16],
    space_control_generation: [u8; 16],
    manifests: Arc<ActiveSpaceGenerationManifestStore>,
}

impl V3InitialSpaceActivation {
    pub fn new(
        profile_data_generation: [u8; 16],
        space_control_generation: [u8; 16],
        manifests: Arc<ActiveSpaceGenerationManifestStore>,
    ) -> Option<Self> {
        (profile_data_generation != [0; 16]
            && space_control_generation != [0; 16]
            && profile_data_generation != space_control_generation)
            .then_some(Self {
                profile_data_generation,
                space_control_generation,
                manifests,
            })
    }

    fn keyslot_generation(&self, space_id: &SpaceId) -> [u8; 16] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(INITIAL_KEYSLOT_GENERATION_DOMAIN);
        hasher.update(&self.profile_data_generation);
        hasher.update(&self.space_control_generation);
        hasher.update(&(space_id.as_ref().len() as u64).to_be_bytes());
        hasher.update(space_id.as_ref().as_bytes());
        let mut generation = [0; 16];
        generation.copy_from_slice(&hasher.finalize().as_bytes()[..16]);
        generation[0] |= 1;
        generation
    }
}

#[async_trait]
impl InitialSpaceActivationPort for V3InitialSpaceActivation {
    async fn activate_initial_space(
        &self,
        space_id: &SpaceId,
    ) -> Result<(), CurrentSpaceIdentityError> {
        let layout = ActiveRuntimeLayout::new(
            space_id.clone(),
            self.profile_data_generation,
            self.space_control_generation,
        )
        .map_err(|_| CurrentSpaceIdentityError::Inconsistent)?;
        let target = ActiveRuntimeManifestV3::new(layout, self.keyslot_generation(space_id))
            .ok_or(CurrentSpaceIdentityError::Inconsistent)?;
        match self
            .manifests
            .promote_initial_v3(&target)
            .await
            .map_err(map_manifest_error)?
        {
            V3ManifestPromotionOutcome::Promoted | V3ManifestPromotionOutcome::AlreadyActive => {
                Ok(())
            }
            V3ManifestPromotionOutcome::SourceChanged => {
                Err(CurrentSpaceIdentityError::Inconsistent)
            }
        }
    }
}

fn map_manifest_error(error: ActiveSpaceGenerationManifestStoreError) -> CurrentSpaceIdentityError {
    match error {
        ActiveSpaceGenerationManifestStoreError::Storage => CurrentSpaceIdentityError::Unavailable,
        ActiveSpaceGenerationManifestStoreError::Corrupt
        | ActiveSpaceGenerationManifestStoreError::UnsupportedVersion => {
            CurrentSpaceIdentityError::Inconsistent
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    use uc_application::deps::InitialSpaceActivationPort;
    use uc_core::ids::SpaceId;
    use uc_core::ports::{SecureStorageError, SecureStoragePort};

    use super::V3InitialSpaceActivation;
    use crate::security::{ActiveSpaceGenerationManifestStore, AdmissionKeyManager};

    #[derive(Default)]
    struct MemorySecureStorage(Mutex<BTreeMap<String, Vec<u8>>>);

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
    async fn fresh_activation_promotes_the_prepared_pair_idempotently() {
        let directory = tempfile::tempdir().unwrap();
        let keys = Arc::new(AdmissionKeyManager::new(
            Arc::new(MemorySecureStorage::default()),
            [0x31; 16],
        ));
        let manifests = Arc::new(ActiveSpaceGenerationManifestStore::new(
            directory.path().to_path_buf(),
            keys,
        ));
        let activation =
            V3InitialSpaceActivation::new([0x32; 16], [0x33; 16], Arc::clone(&manifests)).unwrap();
        let space_id = SpaceId::from_str("first-space");

        activation.activate_initial_space(&space_id).await.unwrap();
        activation.activate_initial_space(&space_id).await.unwrap();

        let active = manifests.load_v3_sync().unwrap().unwrap();
        assert_eq!(active.layout().space_id(), &space_id);
        assert_eq!(active.layout().profile_data_generation(), &[0x32; 16]);
        assert_eq!(active.layout().space_control_generation(), &[0x33; 16]);
    }
}
