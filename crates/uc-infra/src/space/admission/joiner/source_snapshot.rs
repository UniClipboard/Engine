use serde::{Deserialize, Serialize};
use uc_core::membership::{ActiveSpaceGenerationManifestV2, AdmissionSourceSnapshot};

use crate::db::ports::DbExecutor;
use crate::security::{ActiveRuntimeManifest, ActiveSpaceGenerationManifestStoreError};

use super::super::repository::{SpaceAdmissionStateStoreError, SqliteSpaceAdmissionState};

const SOURCE_SNAPSHOT_FORMAT_V1: u16 = 1;
const SOURCE_SNAPSHOT_FORMAT_V2: u16 = 2;

#[derive(Serialize, Deserialize)]
struct PersistedAdmissionSourceSnapshotV1 {
    format_version: u16,
    active_manifest: Option<ActiveSpaceGenerationManifestV2>,
}

#[derive(Serialize, Deserialize)]
struct PersistedAdmissionSourceSnapshotV2 {
    format_version: u16,
    space_id: String,
    keyslot_generation: [u8; 16],
    profile_data_generation: [u8; 16],
    space_control_generation: [u8; 16],
}

impl<E: DbExecutor> SqliteSpaceAdmissionState<E> {
    pub(super) async fn load_source_snapshot(
        &self,
    ) -> Result<(AdmissionSourceSnapshot, bool), SpaceAdmissionStateStoreError> {
        let active_runtime = self
            .manifests
            .load_runtime()
            .await
            .map_err(map_manifest_error)?;
        let requires_session_transition = active_runtime.is_some();
        let encoded = match active_runtime {
            Some(ActiveRuntimeManifest::V3(manifest)) => {
                postcard::to_stdvec(&PersistedAdmissionSourceSnapshotV2 {
                    format_version: SOURCE_SNAPSHOT_FORMAT_V2,
                    space_id: manifest.layout().space_id().as_ref().to_owned(),
                    keyslot_generation: *manifest.keyslot_generation(),
                    profile_data_generation: *manifest.layout().profile_data_generation(),
                    space_control_generation: *manifest.layout().space_control_generation(),
                })
            }
            Some(ActiveRuntimeManifest::V2(manifest)) => {
                postcard::to_stdvec(&PersistedAdmissionSourceSnapshotV1 {
                    format_version: SOURCE_SNAPSHOT_FORMAT_V1,
                    active_manifest: Some(manifest),
                })
            }
            None => postcard::to_stdvec(&PersistedAdmissionSourceSnapshotV1 {
                format_version: SOURCE_SNAPSHOT_FORMAT_V1,
                active_manifest: None,
            }),
        }
        .map_err(|_| SpaceAdmissionStateStoreError::Corrupt)?;
        let snapshot = AdmissionSourceSnapshot::from_bytes(encoded)
            .map_err(|_| SpaceAdmissionStateStoreError::Corrupt)?;
        Ok((snapshot, requires_session_transition))
    }
}

fn map_manifest_error(
    error: ActiveSpaceGenerationManifestStoreError,
) -> SpaceAdmissionStateStoreError {
    match error {
        ActiveSpaceGenerationManifestStoreError::Storage => {
            SpaceAdmissionStateStoreError::Unavailable
        }
        ActiveSpaceGenerationManifestStoreError::Corrupt
        | ActiveSpaceGenerationManifestStoreError::UnsupportedVersion => {
            SpaceAdmissionStateStoreError::Corrupt
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use uc_application::deps::{
        LoadMembershipLedgerPort, LoadedMembershipLedger, MembershipLedgerError,
    };
    use uc_core::ids::SpaceId;
    use uc_core::membership::{ActiveRuntimeLayout, ActiveSpaceGenerationManifestV2};
    use uc_core::ports::{SecureStorageError, SecureStoragePort};

    use crate::db::executor::DieselSqliteExecutor;
    use crate::db::pool::init_db_pool;
    use crate::security::{
        ActiveRuntimeManifestV3, ActiveSpaceGenerationManifestStore, AdmissionKeyManager,
    };

    use super::super::super::repository::SqliteSpaceAdmissionState;
    use super::{PersistedAdmissionSourceSnapshotV1, SOURCE_SNAPSHOT_FORMAT_V1};

    #[derive(Default)]
    struct MemorySecureStorage(Mutex<BTreeMap<String, Vec<u8>>>);

    impl SecureStoragePort for MemorySecureStorage {
        fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SecureStorageError> {
            Ok(self
                .0
                .lock()
                .expect("secure storage lock")
                .get(key)
                .cloned())
        }

        fn set(&self, key: &str, value: &[u8]) -> Result<(), SecureStorageError> {
            self.0
                .lock()
                .expect("secure storage lock")
                .insert(key.to_owned(), value.to_vec());
            Ok(())
        }

        fn delete(&self, key: &str) -> Result<(), SecureStorageError> {
            self.0.lock().expect("secure storage lock").remove(key);
            Ok(())
        }
    }

    struct UnusedMembershipLedger;

    #[async_trait]
    impl LoadMembershipLedgerPort for UnusedMembershipLedger {
        async fn load(&self) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
            Err(MembershipLedgerError::Unavailable)
        }
    }

    #[tokio::test]
    async fn v2_runtime_manifest_retains_the_v1_source_snapshot_encoding() {
        let temp = tempfile::tempdir().expect("temp directory");
        let database = temp.path().join("profile.sqlite");
        let pool = init_db_pool(database.to_str().expect("database path")).expect("database pool");
        let executor = Arc::new(DieselSqliteExecutor::new(pool));
        let keys = Arc::new(AdmissionKeyManager::new(
            Arc::new(MemorySecureStorage::default()),
            [0x11; 16],
        ));
        let manifests = Arc::new(ActiveSpaceGenerationManifestStore::new(
            temp.path().join("vault"),
            Arc::clone(&keys),
        ));
        let manifest = ActiveSpaceGenerationManifestV2::new(
            "space-v2".to_owned(),
            [0x12; 16],
            [0x13; 16],
            [0x14; 16],
        )
        .expect("v2 manifest");
        manifests
            .promote(&manifest)
            .await
            .expect("promote v2 manifest");
        let state = SqliteSpaceAdmissionState::new(
            executor,
            keys,
            manifests,
            Arc::new(UnusedMembershipLedger),
        );

        let (snapshot, requires_transition) = state
            .load_source_snapshot()
            .await
            .expect("load v2 source snapshot");
        let legacy_encoding = postcard::to_stdvec(&PersistedAdmissionSourceSnapshotV1 {
            format_version: SOURCE_SNAPSHOT_FORMAT_V1,
            active_manifest: Some(manifest),
        })
        .expect("encode legacy source snapshot");

        assert!(requires_transition);
        assert_eq!(snapshot.as_bytes(), legacy_encoding);
    }

    #[tokio::test]
    async fn v3_runtime_manifest_is_a_valid_joiner_source_snapshot() {
        let temp = tempfile::tempdir().expect("temp directory");
        let database = temp.path().join("profile.sqlite");
        let pool = init_db_pool(database.to_str().expect("database path")).expect("database pool");
        let executor = Arc::new(DieselSqliteExecutor::new(pool));
        let keys = Arc::new(AdmissionKeyManager::new(
            Arc::new(MemorySecureStorage::default()),
            [0x21; 16],
        ));
        let manifests = Arc::new(ActiveSpaceGenerationManifestStore::new(
            temp.path().join("vault"),
            Arc::clone(&keys),
        ));
        let layout = ActiveRuntimeLayout::new(
            SpaceId::from_string("space-v3".to_owned()),
            [0x31; 16],
            [0x32; 16],
        )
        .expect("runtime layout");
        let manifest = ActiveRuntimeManifestV3::new(layout, [0x33; 16]).expect("v3 manifest");
        manifests
            .promote_initial_v3(&manifest)
            .await
            .expect("promote v3 manifest");
        let state = SqliteSpaceAdmissionState::new(
            executor,
            keys,
            manifests,
            Arc::new(UnusedMembershipLedger),
        );

        let (snapshot, requires_transition) = state
            .load_source_snapshot()
            .await
            .expect("load v3 source snapshot");

        assert!(requires_transition);
        assert!(!snapshot.as_bytes().is_empty());
    }
}
