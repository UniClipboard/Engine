use std::sync::Arc;

use rand::RngCore;
use uc_core::ports::SecureStoragePort;
pub use uc_core::ports::{FactoryResetPhaseV1, ProfileLifecycleError, ProfileLifecycleMarkerV1};
use uc_core::ports::{ProfileLifecyclePort, PROFILE_LIFECYCLE_MARKER_FORMAT_V1};

const PROFILE_LIFECYCLE_MARKER_NAME: &str = "profile_lifecycle_marker:v1";

pub struct ProfileLifecycleManager {
    secure_storage: Arc<dyn SecureStoragePort>,
}

impl ProfileLifecycleManager {
    pub fn new(secure_storage: Arc<dyn SecureStoragePort>) -> Self {
        Self { secure_storage }
    }

    pub fn load_or_initialize(&self) -> Result<ProfileLifecycleMarkerV1, ProfileLifecycleError> {
        if let Some(marker) = self.load()? {
            return Ok(marker);
        }
        let marker = ProfileLifecycleMarkerV1 {
            marker_format_version: PROFILE_LIFECYCLE_MARKER_FORMAT_V1,
            profile_generation: random_generation(),
            factory_reset_phase: FactoryResetPhaseV1::None,
        };
        self.persist(marker)
    }

    pub fn begin_factory_reset(
        &self,
        expected_generation: [u8; 16],
    ) -> Result<ProfileLifecycleMarkerV1, ProfileLifecycleError> {
        let current = self.load()?.ok_or(ProfileLifecycleError::PhaseConflict)?;
        if current.profile_generation != expected_generation {
            return Err(ProfileLifecycleError::PhaseConflict);
        }
        match current.factory_reset_phase {
            FactoryResetPhaseV1::None => self.persist(ProfileLifecycleMarkerV1 {
                factory_reset_phase: FactoryResetPhaseV1::WipingKeys,
                ..current
            }),
            FactoryResetPhaseV1::WipingKeys => Ok(current),
            FactoryResetPhaseV1::ClearingState => Err(ProfileLifecycleError::PhaseConflict),
        }
    }

    pub fn mark_keys_wiped(
        &self,
        expected_generation: [u8; 16],
    ) -> Result<ProfileLifecycleMarkerV1, ProfileLifecycleError> {
        let current = self.load()?.ok_or(ProfileLifecycleError::PhaseConflict)?;
        if current.profile_generation != expected_generation
            || current.factory_reset_phase != FactoryResetPhaseV1::WipingKeys
        {
            return Err(ProfileLifecycleError::PhaseConflict);
        }
        self.persist(ProfileLifecycleMarkerV1 {
            factory_reset_phase: FactoryResetPhaseV1::ClearingState,
            ..current
        })
    }

    pub fn complete_state_clear(
        &self,
        expected_generation: [u8; 16],
    ) -> Result<ProfileLifecycleMarkerV1, ProfileLifecycleError> {
        let current = self.load()?.ok_or(ProfileLifecycleError::PhaseConflict)?;
        if current.profile_generation != expected_generation
            || current.factory_reset_phase != FactoryResetPhaseV1::ClearingState
        {
            return Err(ProfileLifecycleError::PhaseConflict);
        }
        self.persist(ProfileLifecycleMarkerV1 {
            marker_format_version: PROFILE_LIFECYCLE_MARKER_FORMAT_V1,
            profile_generation: random_generation(),
            factory_reset_phase: FactoryResetPhaseV1::None,
        })
    }

    fn load(&self) -> Result<Option<ProfileLifecycleMarkerV1>, ProfileLifecycleError> {
        let Some(bytes) = self
            .secure_storage
            .get(PROFILE_LIFECYCLE_MARKER_NAME)
            .map_err(|_| ProfileLifecycleError::SecureStorage)?
        else {
            return Ok(None);
        };
        let marker: ProfileLifecycleMarkerV1 =
            postcard::from_bytes(&bytes).map_err(|_| ProfileLifecycleError::Corrupt)?;
        if marker.marker_format_version != PROFILE_LIFECYCLE_MARKER_FORMAT_V1 {
            return Err(ProfileLifecycleError::Corrupt);
        }
        Ok(Some(marker))
    }

    fn persist(
        &self,
        marker: ProfileLifecycleMarkerV1,
    ) -> Result<ProfileLifecycleMarkerV1, ProfileLifecycleError> {
        let bytes = postcard::to_stdvec(&marker).map_err(|_| ProfileLifecycleError::Corrupt)?;
        self.secure_storage
            .set(PROFILE_LIFECYCLE_MARKER_NAME, &bytes)
            .map_err(|_| ProfileLifecycleError::SecureStorage)?;
        let reopened = self.load()?.ok_or(ProfileLifecycleError::SecureStorage)?;
        if reopened != marker {
            return Err(ProfileLifecycleError::Corrupt);
        }
        Ok(reopened)
    }
}

impl ProfileLifecyclePort for ProfileLifecycleManager {
    fn load_or_initialize(&self) -> Result<ProfileLifecycleMarkerV1, ProfileLifecycleError> {
        ProfileLifecycleManager::load_or_initialize(self)
    }

    fn begin_factory_reset(
        &self,
        expected_generation: [u8; 16],
    ) -> Result<ProfileLifecycleMarkerV1, ProfileLifecycleError> {
        ProfileLifecycleManager::begin_factory_reset(self, expected_generation)
    }

    fn mark_keys_wiped(
        &self,
        expected_generation: [u8; 16],
    ) -> Result<ProfileLifecycleMarkerV1, ProfileLifecycleError> {
        ProfileLifecycleManager::mark_keys_wiped(self, expected_generation)
    }

    fn complete_state_clear(
        &self,
        expected_generation: [u8; 16],
    ) -> Result<ProfileLifecycleMarkerV1, ProfileLifecycleError> {
        ProfileLifecycleManager::complete_state_clear(self, expected_generation)
    }
}

fn random_generation() -> [u8; 16] {
    let mut generation = [0u8; 16];
    rand::rng().fill_bytes(&mut generation);
    generation
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use uc_core::ports::{SecureStorageError, SecureStoragePort};

    use super::*;
    use crate::security::AdmissionKeyManager;

    #[derive(Default)]
    struct MemorySecureStorage {
        values: Mutex<HashMap<String, Vec<u8>>>,
    }

    impl SecureStoragePort for MemorySecureStorage {
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
            self.values.lock().unwrap().remove(key);
            Ok(())
        }
    }

    #[test]
    fn factory_reset_phase_survives_restart_and_generation_changes_only_after_clear() {
        let storage = Arc::new(MemorySecureStorage::default());
        let lifecycle = ProfileLifecycleManager::new(storage.clone());
        let initial = lifecycle.load_or_initialize().unwrap();
        let keys = AdmissionKeyManager::new(storage.clone(), initial.profile_generation);
        let _ = keys.create_wrapped_attempt_key([0x41; 32]).unwrap();
        assert!(keys.profile_key_exists().unwrap());

        let wiping = lifecycle
            .begin_factory_reset(initial.profile_generation)
            .unwrap();
        assert_eq!(wiping.factory_reset_phase, FactoryResetPhaseV1::WipingKeys);
        let reopened = ProfileLifecycleManager::new(storage.clone())
            .load_or_initialize()
            .unwrap();
        assert_eq!(reopened, wiping);

        keys.delete_profile_key().unwrap();
        let clearing = lifecycle
            .mark_keys_wiped(initial.profile_generation)
            .unwrap();
        assert_eq!(
            clearing.factory_reset_phase,
            FactoryResetPhaseV1::ClearingState
        );
        let fresh = lifecycle
            .complete_state_clear(initial.profile_generation)
            .unwrap();
        assert_ne!(fresh.profile_generation, initial.profile_generation);
        assert_eq!(fresh.factory_reset_phase, FactoryResetPhaseV1::None);
        assert!(!keys.profile_key_exists().unwrap());
    }
}
