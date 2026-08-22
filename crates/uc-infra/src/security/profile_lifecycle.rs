use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use uc_application::deps::{
    FactoryResetPhase, ProfileGeneration, ProfileLifecycle, ProfileLifecycleRepositoryError,
    ProfileLifecycleRepositoryPort, ProfileLifecycleState,
};
use uc_core::ports::SecureStoragePort;

const PROFILE_LIFECYCLE_MARKER_NAME: &str = "profile_lifecycle_marker:v1";
const PROFILE_LIFECYCLE_MARKER_FORMAT_V1: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FactoryResetPhaseV1 {
    None,
    WipingKeys,
    ClearingState,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct ProfileLifecycleMarkerV1 {
    marker_format_version: u16,
    profile_generation: [u8; 16],
    factory_reset_phase: FactoryResetPhaseV1,
}

pub struct ProfileLifecycleRepository {
    secure_storage: Arc<dyn SecureStoragePort>,
    write_lock: Mutex<()>,
}

impl ProfileLifecycleRepository {
    pub fn new(secure_storage: Arc<dyn SecureStoragePort>) -> Self {
        Self {
            secure_storage,
            write_lock: Mutex::new(()),
        }
    }

    fn load_marker(
        &self,
    ) -> Result<Option<ProfileLifecycleMarkerV1>, ProfileLifecycleRepositoryError> {
        let Some(bytes) = self
            .secure_storage
            .get(PROFILE_LIFECYCLE_MARKER_NAME)
            .map_err(|_| ProfileLifecycleRepositoryError::Unavailable)?
        else {
            return Ok(None);
        };
        let marker: ProfileLifecycleMarkerV1 =
            postcard::from_bytes(&bytes).map_err(|_| ProfileLifecycleRepositoryError::Corrupt)?;
        if marker.marker_format_version != PROFILE_LIFECYCLE_MARKER_FORMAT_V1 {
            return Err(ProfileLifecycleRepositoryError::Corrupt);
        }
        Ok(Some(marker))
    }

    fn persist(&self, lifecycle: &ProfileLifecycle) -> Result<(), ProfileLifecycleRepositoryError> {
        let marker = marker_from_lifecycle(lifecycle);
        let bytes =
            postcard::to_stdvec(&marker).map_err(|_| ProfileLifecycleRepositoryError::Corrupt)?;
        self.secure_storage
            .set(PROFILE_LIFECYCLE_MARKER_NAME, &bytes)
            .map_err(|_| ProfileLifecycleRepositoryError::Unavailable)?;
        let reopened = self
            .load_marker()?
            .ok_or(ProfileLifecycleRepositoryError::Unavailable)?;
        if reopened != marker {
            return Err(ProfileLifecycleRepositoryError::Corrupt);
        }
        Ok(())
    }
}

impl ProfileLifecycleRepositoryPort for ProfileLifecycleRepository {
    fn load(&self) -> Result<Option<ProfileLifecycle>, ProfileLifecycleRepositoryError> {
        Ok(self.load_marker()?.map(lifecycle_from_marker))
    }

    fn compare_and_swap(
        &self,
        expected: Option<&ProfileLifecycle>,
        next: &ProfileLifecycle,
    ) -> Result<(), ProfileLifecycleRepositoryError> {
        let _guard = self
            .write_lock
            .lock()
            .map_err(|_| ProfileLifecycleRepositoryError::Unavailable)?;
        let current = self.load()?;
        if current.as_ref() != expected {
            return Err(ProfileLifecycleRepositoryError::Conflict);
        }
        self.persist(next)
    }
}

fn lifecycle_from_marker(marker: ProfileLifecycleMarkerV1) -> ProfileLifecycle {
    let state = match marker.factory_reset_phase {
        FactoryResetPhaseV1::None => ProfileLifecycleState::Ready,
        FactoryResetPhaseV1::WipingKeys => {
            ProfileLifecycleState::FactoryReset(FactoryResetPhase::Started)
        }
        FactoryResetPhaseV1::ClearingState => {
            ProfileLifecycleState::FactoryReset(FactoryResetPhase::KeysWiped)
        }
    };
    ProfileLifecycle::restore(
        ProfileGeneration::from_bytes(marker.profile_generation),
        state,
    )
}

fn marker_from_lifecycle(lifecycle: &ProfileLifecycle) -> ProfileLifecycleMarkerV1 {
    let factory_reset_phase = match lifecycle.state() {
        ProfileLifecycleState::Ready => FactoryResetPhaseV1::None,
        ProfileLifecycleState::FactoryReset(FactoryResetPhase::Started) => {
            FactoryResetPhaseV1::WipingKeys
        }
        ProfileLifecycleState::FactoryReset(FactoryResetPhase::KeysWiped) => {
            FactoryResetPhaseV1::ClearingState
        }
    };
    ProfileLifecycleMarkerV1 {
        marker_format_version: PROFILE_LIFECYCLE_MARKER_FORMAT_V1,
        profile_generation: lifecycle.generation().into_bytes(),
        factory_reset_phase,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use uc_core::ports::{SecureStorageError, SecureStoragePort};

    use super::*;

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
    fn repository_preserves_each_checkpoint_across_restart() {
        let storage = Arc::new(MemorySecureStorage::default());
        let repository = ProfileLifecycleRepository::new(storage.clone());
        let generation = ProfileGeneration::from_bytes([1; 16]);
        let ready = ProfileLifecycle::new(generation);
        repository.compare_and_swap(None, &ready).unwrap();

        let mut started = ready.clone();
        started.begin_factory_reset(generation).unwrap();
        repository.compare_and_swap(Some(&ready), &started).unwrap();
        assert_eq!(
            ProfileLifecycleRepository::new(storage.clone())
                .load()
                .unwrap(),
            Some(started.clone())
        );

        let mut keys_wiped = started.clone();
        keys_wiped.mark_keys_wiped(generation).unwrap();
        repository
            .compare_and_swap(Some(&started), &keys_wiped)
            .unwrap();
        assert_eq!(
            ProfileLifecycleRepository::new(storage).load().unwrap(),
            Some(keys_wiped)
        );
    }

    #[test]
    fn repository_rejects_a_stale_expected_state() {
        let storage = Arc::new(MemorySecureStorage::default());
        let repository = ProfileLifecycleRepository::new(storage);
        let generation = ProfileGeneration::from_bytes([1; 16]);
        let ready = ProfileLifecycle::new(generation);
        repository.compare_and_swap(None, &ready).unwrap();

        let stale = ProfileLifecycle::new(ProfileGeneration::from_bytes([9; 16]));
        assert_eq!(
            repository.compare_and_swap(Some(&stale), &ready),
            Err(ProfileLifecycleRepositoryError::Conflict)
        );
    }
}
