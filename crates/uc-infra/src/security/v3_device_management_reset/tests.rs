use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tempfile::tempdir;
use uc_application::deps::DeviceManagementResetDataPort;
use uc_core::ids::SpaceId;
use uc_core::membership::{
    ActiveRuntimeLayout, ActiveSpaceGenerationManifestV2, RevocationRepositoryPort,
};
use uc_core::ports::security::current_profile::CurrentProfilePort;
use uc_core::ports::{SecureStorageError, SecureStoragePort};

use super::V3DeviceManagementReset;
use crate::db::executor::DieselSqliteExecutor;
use crate::db::pool::init_db_pool;
use crate::db::repositories::DieselSpaceSecurityStore;
use crate::fs::key_slot_store::JsonKeySlotStore;
use crate::security::active_space_generation_manifest_store::V3ManifestPromotionOutcome;
use crate::security::{
    ActiveRuntimeManifest, ActiveRuntimeManifestV3, ActiveSpaceGenerationManifestStore,
    AdmissionKeyManager, DefaultCurrentProfile, MasterKey, ProfileContentKeyVault,
    ProfileRuntimeLayout, SpaceControlGeneration, SpaceTransitionActivation,
};
use crate::space::{DefaultSpaceAccessAdapter, InMemorySession, KeyMaterialStore};

#[derive(Default)]
struct MemorySecureStorage(Mutex<HashMap<String, Vec<u8>>>);

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
async fn v3_device_reset_replaces_only_the_control_generation() {
    let directory = tempdir().unwrap();
    let root = directory.path().join("profile");
    let secure_storage: Arc<dyn SecureStoragePort> = Arc::new(MemorySecureStorage::default());
    let current_profile: Arc<dyn CurrentProfilePort> = Arc::new(DefaultCurrentProfile::new());
    let admission_keys = Arc::new(AdmissionKeyManager::new(
        Arc::clone(&secure_storage),
        [0x81; 16],
    ));
    let manifests = Arc::new(ActiveSpaceGenerationManifestStore::new(
        root.join("vault"),
        Arc::clone(&admission_keys),
    ));
    let source_space = SpaceId::from_str("reset-source-space");
    let source = ActiveRuntimeManifestV3::new(
        ActiveRuntimeLayout::new(source_space.clone(), [0x82; 16], [0x83; 16]).unwrap(),
        [0x84; 16],
    )
    .unwrap();
    let legacy = ActiveSpaceGenerationManifestV2::new(
        source_space.as_ref().to_owned(),
        [0x84; 16],
        [0x85; 16],
        [0x86; 16],
    )
    .unwrap();
    manifests.promote(&legacy).await.unwrap();
    assert_eq!(
        manifests
            .promote_v3_from_v2(&legacy, &source)
            .await
            .unwrap(),
        V3ManifestPromotionOutcome::Promoted
    );

    let source_layout = ProfileRuntimeLayout::v3(&root, &source);
    std::fs::create_dir_all(source_layout.profile_database().parent().unwrap()).unwrap();
    std::fs::write(source_layout.profile_database(), b"reset-retained-profile").unwrap();
    std::fs::create_dir_all(source_layout.blob_root()).unwrap();
    std::fs::write(
        source_layout.blob_root().join("history.ucbl"),
        b"reset-retained-blob",
    )
    .unwrap();
    std::fs::create_dir_all(source_layout.control_database().parent().unwrap()).unwrap();
    let control_pool = init_db_pool(source_layout.control_database().to_str().unwrap()).unwrap();
    let session = Arc::new(InMemorySession::new());
    let master_key = MasterKey::from_bytes(&[0x87; 32]).unwrap();
    session.set_master_key_for_space(source_space.clone(), master_key.clone());
    let repository = Arc::new(DieselSpaceSecurityStore::new(
        Arc::new(DieselSqliteExecutor::new(control_pool.clone())),
        session.as_ref().clone(),
    ));
    let source_material = session
        .create_legacy_bootstrap_material(&source_space, b"source-mls-state".to_vec(), 1)
        .unwrap();
    repository
        .save_space_material(&source_material)
        .await
        .unwrap();
    let vault = Arc::new(ProfileContentKeyVault::new(
        root.join("vault"),
        Arc::clone(&secure_storage),
        [0x81; 16],
    ));
    let access = Arc::new(DefaultSpaceAccessAdapter::new_with_key_epoch_repository(
        Arc::new(KeyMaterialStore::new(
            Arc::clone(&secure_storage),
            Arc::new(JsonKeySlotStore::new(root.join("keys"))),
        )),
        Arc::clone(&current_profile),
        Arc::clone(&session),
        repository.clone(),
        vault,
    ));
    let generations = Arc::new(SpaceControlGeneration::new(
        root.clone(),
        access.clone(),
        current_profile,
        Arc::clone(&admission_keys),
    ));
    let activation = Arc::new(SpaceTransitionActivation::new(
        root.clone(),
        control_pool.clone(),
        Arc::clone(&manifests),
        Arc::clone(&generations),
        access,
    ));
    let reset = V3DeviceManagementReset::new(
        root.clone(),
        control_pool,
        Arc::clone(&manifests),
        generations,
        activation,
    );
    let target_space = SpaceId::from_str("reset-target-space");

    reset
        .prepare_device_management_reset(&target_space)
        .await
        .unwrap();
    reset
        .stage_device_management_reset_mutations(&target_space)
        .await
        .unwrap();

    // Application 的既有 rebuild 流程在 stage 后保留 MasterKey、切换 Space，
    // 并通过正式 control repository 写入新的本机 MLS/security material。
    session.set_master_key_for_space(target_space.clone(), master_key.clone());
    let target_material = session
        .create_legacy_bootstrap_material(&target_space, b"target-mls-state".to_vec(), 2)
        .unwrap();
    repository
        .save_space_material(&target_material)
        .await
        .unwrap();

    reset
        .promote_device_management_reset(&target_space)
        .await
        .unwrap();
    reset
        .finalize_device_management_reset(&target_space)
        .await
        .unwrap();

    let Some(ActiveRuntimeManifest::V3(active)) = manifests.load_runtime().await.unwrap() else {
        panic!("reset target manifest is not active");
    };
    assert_eq!(active.layout().space_id(), &target_space);
    assert_eq!(active.layout().profile_data_generation(), &[0x82; 16]);
    assert_eq!(active.keyslot_generation(), &[0x84; 16]);
    assert_ne!(active.layout().space_control_generation(), &[0x83; 16]);
    assert_eq!(session.current_space_id().unwrap(), target_space);
    assert_eq!(session.get_master_key().unwrap(), master_key);
    assert_eq!(
        std::fs::read(source_layout.profile_database()).unwrap(),
        b"reset-retained-profile"
    );
    assert_eq!(
        std::fs::read(source_layout.blob_root().join("history.ucbl")).unwrap(),
        b"reset-retained-blob"
    );
    assert!(!source_layout.control_database().exists());
    let target_layout = ProfileRuntimeLayout::v3(&root, &active);
    assert!(target_layout.control_database().is_file());
    assert_no_forbidden_paths(
        &root,
        &[
            "source-backup.sqlite",
            "source-final.sqlite",
            "target.sqlite",
        ],
    );
}

fn assert_no_forbidden_paths(root: &std::path::Path, forbidden: &[&str]) {
    for entry in std::fs::read_dir(root).unwrap().filter_map(Result::ok) {
        assert!(!forbidden.contains(&entry.file_name().to_string_lossy().as_ref()));
        if entry.file_type().unwrap().is_dir() {
            assert_no_forbidden_paths(&entry.path(), forbidden);
        }
    }
}
