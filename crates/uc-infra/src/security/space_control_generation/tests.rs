use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tempfile::tempdir;
use uc_application::deps::AdmissionSpaceTransitionPreparationV2;
use uc_core::crypto::domain::Passphrase;
use uc_core::ids::{DeviceId, SpaceId};
use uc_core::membership::{
    ActiveRuntimeLayout, AdmissionChangeFacts, AdmissionContentKeyCatalogV1,
    AdmissionContentKeyEntryV1, AdmissionSecurityCommitmentV1, BaseMembershipHistoryPosition,
    LegacyBootstrapRepositoryPort, MembershipCredential, PendingGroupUpdate,
    RevocationRepositoryPort, SpaceAdmissionId, ADMISSION_SECURITY_COMMITMENT_FORMAT_V1,
    ED25519_SIGNATURE_ALGORITHM_V1,
};
use uc_core::ports::security::current_profile::CurrentProfilePort;
use uc_core::ports::space::PrepareAdmissionTargetAccessPort;
use uc_core::ports::{SecureStorageError, SecureStoragePort};

use super::{acquire_lease, SpaceControlGeneration, SpaceControlGenerationError};
use crate::db::executor::DieselSqliteExecutor;
use crate::db::pool::init_db_pool;
use crate::db::repositories::DieselSpaceSecurityStore;
use crate::fs::key_slot_store::JsonKeySlotStore;
use crate::security::{
    ActiveRuntimeManifestV3, AdmissionKeyManager, DefaultCurrentProfile, ProfileContentKeyVault,
    ProfileRuntimeLayout,
};
use crate::space::{
    prepare_registration, InMemorySession, KeyMaterialStore, RuntimeSpaceAccessAdapter,
};

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
async fn complete_admission_generation_is_published_once_and_reused() {
    let directory = tempdir().unwrap();
    let root = directory.path().join("profile");
    std::fs::create_dir_all(&root).unwrap();
    let secure_storage: Arc<dyn SecureStoragePort> = Arc::new(MemorySecureStorage::default());
    let current_profile: Arc<dyn CurrentProfilePort> = Arc::new(DefaultCurrentProfile::new());
    let session = Arc::new(InMemorySession::new());
    let security_pool =
        init_db_pool(root.join("fixture-security.sqlite").to_str().unwrap()).unwrap();
    let security_store = Arc::new(DieselSpaceSecurityStore::new(
        Arc::new(DieselSqliteExecutor::new(security_pool)),
        session.as_ref().clone(),
    ));
    let access = Arc::new(RuntimeSpaceAccessAdapter::new(
        Arc::new(KeyMaterialStore::new(
            Arc::clone(&secure_storage),
            Arc::new(JsonKeySlotStore::new(root.join("keys"))),
        )),
        Arc::clone(&current_profile),
        session,
        security_store.clone() as Arc<dyn RevocationRepositoryPort>,
        security_store as Arc<dyn LegacyBootstrapRepositoryPort>,
        Arc::new(ProfileContentKeyVault::new(
            root.join("profile-content-vault"),
            Arc::clone(&secure_storage),
            [0x40; 16],
        )),
    ));
    let keys = Arc::new(AdmissionKeyManager::new(
        Arc::clone(&secure_storage),
        [0x41; 16],
    ));
    let space = SpaceId::from_str("target-space");
    let target_access = PrepareAdmissionTargetAccessPort::prepare_target_access(
        access.as_ref(),
        &space,
        &Passphrase::new("target passphrase"),
    )
    .await
    .unwrap();
    let manifest = ActiveRuntimeManifestV3::new(
        ActiveRuntimeLayout::new(space.clone(), [0x42; 16], [0x43; 16]).unwrap(),
        [0x44; 16],
    )
    .unwrap();
    let input = preparation(&space, target_access.into_bytes());
    let generations = SpaceControlGeneration::new(root.clone(), access, current_profile, keys);

    let mismatched_manifest = ActiveRuntimeManifestV3::new(
        ActiveRuntimeLayout::new(SpaceId::from_str("different-space"), [0x42; 16], [0x43; 16])
            .unwrap(),
        [0x44; 16],
    )
    .unwrap();
    assert!(matches!(
        generations
            .prepare_admission(&input, &mismatched_manifest)
            .await,
        Err(SpaceControlGenerationError::Inconsistent { .. })
    ));
    assert!(!ProfileRuntimeLayout::v3(&root, &manifest)
        .control_database()
        .exists());

    let layout = ProfileRuntimeLayout::v3(&root, &manifest);
    let generation_parent = layout
        .control_database()
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    std::fs::create_dir_all(generation_parent).unwrap();
    let held_lease = acquire_lease(generation_parent).unwrap();
    assert!(matches!(
        generations.prepare_admission(&input, &manifest).await,
        Err(SpaceControlGenerationError::Busy { .. })
    ));
    drop(held_lease);

    let first = generations
        .prepare_admission(&input, &manifest)
        .await
        .unwrap();
    let second = generations
        .prepare_admission(&input, &manifest)
        .await
        .unwrap();

    assert_eq!(first.manifest(), &manifest);
    assert_eq!(first.database_digest(), second.database_digest());
    assert_ne!(first.database_digest(), &[0; 32]);
    let database = ProfileRuntimeLayout::v3(&root, &manifest)
        .control_database()
        .to_path_buf();
    assert!(database.is_file());
    assert!(!layout.profile_database().exists());
    assert!(!layout.blob_root().exists());
    assert!(std::fs::read_dir(generation_parent)
        .unwrap()
        .filter_map(Result::ok)
        .all(|entry| !entry
            .file_name()
            .to_string_lossy()
            .starts_with(".space-control-generation-")
            || !entry.file_name().to_string_lossy().ends_with(".tmp")));
    let bytes = std::fs::read(&database).unwrap();
    for plaintext in [
        b"target-space".as_slice(),
        b"verified membership history".as_slice(),
        b"verified MLS security state".as_slice(),
        b"target local".as_slice(),
        b"target peer".as_slice(),
        b"sealed group update".as_slice(),
    ] {
        assert!(!bytes
            .windows(plaintext.len())
            .any(|window| window == plaintext));
    }

    std::fs::write(&database, b"not a sqlite database").unwrap();
    assert!(matches!(
        generations.prepare_admission(&input, &manifest).await,
        Err(SpaceControlGenerationError::Inconsistent { .. })
    ));
}

fn preparation(
    space: &SpaceId,
    target_access_state: Vec<u8>,
) -> AdmissionSpaceTransitionPreparationV2 {
    let attempt = [0x45; 32];
    let catalog = AdmissionContentKeyCatalogV1::new(
        "target-content-key",
        1,
        vec![
            AdmissionContentKeyEntryV1::new("legacy-v1", 0, vec![0x46; 32]).unwrap(),
            AdmissionContentKeyEntryV1::new("target-content-key", 1, vec![0x47; 32]).unwrap(),
        ],
    )
    .unwrap();
    AdmissionSpaceTransitionPreparationV2 {
        attempt_id: SpaceAdmissionId::from_bytes(attempt).unwrap(),
        target_space_id: space.as_ref().to_owned(),
        target_security_commitment: AdmissionSecurityCommitmentV1::new(
            ADMISSION_SECURITY_COMMITMENT_FORMAT_V1,
            space.as_ref().to_owned(),
            b"target-mls-group".to_vec(),
            attempt,
            BaseMembershipHistoryPosition {
                event_id: None,
                depth: 0,
                history_digest: [0x48; 32],
            },
            [0x49; 32],
            1,
            0,
            1,
            [0x4a; 32],
            [0x4b; 32],
            [0x4c; 32],
            catalog.digest(),
            [0x4d; 32],
        )
        .unwrap(),
        target_membership_history: b"verified membership history".to_vec(),
        target_security_state: b"verified MLS security state".to_vec(),
        target_protection_group_id: "target-protection-group".to_owned(),
        target_key_catalog: catalog.encode().unwrap(),
        local_device_id: DeviceId::new("target-local"),
        target_relationships: relationships(),
        relayed_group_updates: vec![PendingGroupUpdate::persistent(
            DeviceId::new("target-peer"),
            b"sealed group update".to_vec(),
        )],
        target_access_state,
        target_admission_credentials: prepare_registration(&Passphrase::new("target passphrase"))
            .unwrap(),
        preserve_unreadable_history: false,
    }
}

fn relationships() -> Vec<AdmissionChangeFacts> {
    [
        ("target-local", "target local", 0x51),
        ("target-peer", "target peer", 0x52),
    ]
    .into_iter()
    .map(|(device, name, key)| {
        let device_id = DeviceId::new(device);
        let credential = MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![key; 32]);
        AdmissionChangeFacts {
            member_instance: credential.member_instance_id(&device_id),
            device_id,
            device_name: name.to_owned(),
            identity_fingerprint: uc_core::security::IdentityFingerprint::from_display_string(
                "ABCD-EFGH-IJKL-MNOP",
            )
            .unwrap(),
            transport_public_key: vec![key],
            transport_address_blob: vec![key, key],
            identity_signature: vec![key, key, key],
        }
    })
    .collect()
}
