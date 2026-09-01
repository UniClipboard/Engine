use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tempfile::tempdir;
use uc_application::deps::{
    AdmissionSpaceTransitionPort, AdmissionSpaceTransitionPreparationV2,
    AdmissionSpaceTransitionStepV2,
};
use uc_core::crypto::domain::Passphrase;
use uc_core::ids::{DeviceId, SpaceId};
use uc_core::membership::{
    ActiveRuntimeLayout, ActiveSpaceGenerationManifestV2, AdmissionChangeFacts,
    AdmissionContentKeyCatalogV1, AdmissionContentKeyEntryV1, AdmissionSecurityCommitmentV1,
    AdmissionSpaceTransitionResultV2, AdmissionSpaceTransitionV2, BaseMembershipHistoryPosition,
    CrossSpaceControlTransitionPhaseV3, MembershipCredential, PendingGroupUpdate, SpaceAdmissionId,
    ADMISSION_SECURITY_COMMITMENT_FORMAT_V1, ED25519_SIGNATURE_ALGORITHM_V1,
};
use uc_core::ports::security::current_profile::CurrentProfilePort;
use uc_core::ports::space::PrepareAdmissionTargetAccessPort;
use uc_core::ports::{SecureStorageError, SecureStoragePort};

use super::V3AdmissionSpaceTransition;
use crate::db::executor::DieselSqliteExecutor;
use crate::db::pool::init_db_pool;
use crate::db::repositories::DieselSpaceSecurityStore;
use crate::fs::key_slot_store::JsonKeySlotStore;
use crate::security::active_space_generation_manifest_store::V3ManifestPromotionOutcome;
use crate::security::{
    ActiveRuntimeManifest, ActiveRuntimeManifestV3, ActiveSpaceGenerationManifestStore,
    AdmissionKeyManager, DefaultCurrentProfile, ProfileContentKeyVault, ProfileRuntimeLayout,
    SpaceControlGeneration, SpaceTransitionActivation,
};
use crate::space::{
    prepare_registration, DefaultSpaceAccessAdapter, InMemorySession, KeyMaterialStore,
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
async fn v3_cross_space_switches_only_the_control_generation() {
    let directory = tempdir().unwrap();
    let root = directory.path().join("profile");
    let secure_storage: Arc<dyn SecureStoragePort> = Arc::new(MemorySecureStorage::default());
    let current_profile: Arc<dyn CurrentProfilePort> = Arc::new(DefaultCurrentProfile::new());
    let admission_keys = Arc::new(AdmissionKeyManager::new(
        Arc::clone(&secure_storage),
        [0x31; 16],
    ));
    let manifests = Arc::new(ActiveSpaceGenerationManifestStore::new(
        root.join("vault"),
        Arc::clone(&admission_keys),
    ));
    let source_space = SpaceId::from_str("source-space");
    let source = ActiveRuntimeManifestV3::new(
        ActiveRuntimeLayout::new(source_space.clone(), [0x32; 16], [0x33; 16]).unwrap(),
        [0x34; 16],
    )
    .unwrap();
    let legacy = ActiveSpaceGenerationManifestV2::new(
        source_space.as_ref().to_owned(),
        [0x34; 16],
        [0x35; 16],
        [0x36; 16],
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
    std::fs::write(
        source_layout.profile_database(),
        b"unchanged profile database",
    )
    .unwrap();
    std::fs::create_dir_all(source_layout.blob_root()).unwrap();
    std::fs::write(
        source_layout.blob_root().join("history.ucbl"),
        b"unchanged encrypted history",
    )
    .unwrap();
    std::fs::create_dir_all(source_layout.control_database().parent().unwrap()).unwrap();
    let control_pool = init_db_pool(source_layout.control_database().to_str().unwrap()).unwrap();
    let session = Arc::new(InMemorySession::new());
    let repository = Arc::new(DieselSpaceSecurityStore::new(
        Arc::new(DieselSqliteExecutor::new(control_pool.clone())),
        session.as_ref().clone(),
    ));
    let vault = Arc::new(ProfileContentKeyVault::new(
        root.join("vault"),
        Arc::clone(&secure_storage),
        [0x31; 16],
    ));
    let access = Arc::new(DefaultSpaceAccessAdapter::new_with_key_epoch_repository(
        Arc::new(KeyMaterialStore::new(
            Arc::clone(&secure_storage),
            Arc::new(JsonKeySlotStore::new(root.join("keys"))),
        )),
        Arc::clone(&current_profile),
        session.clone(),
        repository,
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
        control_pool,
        Arc::clone(&manifests),
        Arc::clone(&generations),
        access.clone(),
    ));
    let transitions = V3AdmissionSpaceTransition::new(
        b"profile-salt".to_vec(),
        Arc::clone(&manifests),
        Arc::clone(&generations),
        Arc::clone(&activation),
    );
    let target_space = SpaceId::from_str("target-space");
    let target_access = PrepareAdmissionTargetAccessPort::prepare_target_access(
        access.as_ref(),
        &target_space,
        &Passphrase::new("target passphrase"),
    )
    .await
    .unwrap();
    let input = preparation(&target_space, target_access.into_bytes());

    let mut transition = transitions.prepare_if_needed(&input).await.unwrap();
    let AdmissionSpaceTransitionV2::CrossSpaceControl(prepared) = &transition else {
        panic!("expected control-only transition");
    };
    assert_eq!(prepared.profile_data_generation, [0x32; 16]);
    assert_eq!(
        prepared.phase,
        CrossSpaceControlTransitionPhaseV3::TargetPrepared
    );

    let prepared_target = ActiveRuntimeManifestV3::new(
        ActiveRuntimeLayout::new(
            target_space.clone(),
            prepared.profile_data_generation,
            prepared.target_control_generation,
        )
        .unwrap(),
        prepared.target_keyslot_generation,
    )
    .unwrap();
    transitions
        .discard_pre_activation(&transition)
        .await
        .unwrap();
    assert!(!ProfileRuntimeLayout::v3(&root, &prepared_target)
        .control_database()
        .exists());
    assert_eq!(
        manifests.load_runtime().await.unwrap(),
        Some(ActiveRuntimeManifest::V3(source.clone()))
    );
    transition = transitions.prepare_if_needed(&input).await.unwrap();

    transition = advance_to(
        &transitions,
        &transition,
        CrossSpaceControlTransitionPhaseV3::ActivationStarted,
    )
    .await;
    let AdmissionSpaceTransitionV2::CrossSpaceControl(activation_started) = &transition else {
        panic!("transition changed format");
    };
    let proof = generations
        .reopen_prepared(
            &prepared_target,
            &activation_started.prepared_database_digest,
        )
        .await
        .unwrap();
    activation
        .activate_cross_space(&source, &proof, &activation_started.target_access_state)
        .await
        .unwrap();
    // 模拟 manifest 已提升、transition phase 尚未保存时进程终止；同一持久 phase
    // 必须只向 target 前向恢复，不能再要求已经转为可写库的 prepared 摘要。
    transition = advance_to(
        &transitions,
        &transition,
        CrossSpaceControlTransitionPhaseV3::TargetPromoted,
    )
    .await;
    transition = advance_to(
        &transitions,
        &transition,
        CrossSpaceControlTransitionPhaseV3::CleanupPending,
    )
    .await;
    let result = match transitions.advance(&transition).await.unwrap() {
        AdmissionSpaceTransitionStepV2::Finished(result) => result,
        AdmissionSpaceTransitionStepV2::Advanced(_) => panic!("transition did not finish"),
    };
    assert!(matches!(
        result,
        AdmissionSpaceTransitionResultV2::CrossSpaceControl(_)
    ));

    let active = manifests.load_runtime().await.unwrap().unwrap();
    let ActiveRuntimeManifest::V3(active) = active else {
        panic!("active manifest regressed");
    };
    assert_eq!(active.layout().space_id(), &target_space);
    assert_eq!(active.layout().profile_data_generation(), &[0x32; 16]);
    assert_ne!(active.layout().space_control_generation(), &[0x33; 16]);
    assert_eq!(
        std::fs::read(source_layout.profile_database()).unwrap(),
        b"unchanged profile database"
    );
    assert_eq!(
        std::fs::read(source_layout.blob_root().join("history.ucbl")).unwrap(),
        b"unchanged encrypted history"
    );
    assert!(!source_layout.control_database().exists());
    assert_eq!(session.current_space_id().unwrap(), target_space);

    let forbidden = [
        "source-final.sqlite",
        "source-backup.sqlite",
        "target.sqlite",
        "workspace-state.bin",
    ];
    assert_no_forbidden_paths(&root, &forbidden);
}

async fn advance_to(
    transitions: &V3AdmissionSpaceTransition,
    transition: &AdmissionSpaceTransitionV2,
    expected: CrossSpaceControlTransitionPhaseV3,
) -> AdmissionSpaceTransitionV2 {
    let next = match transitions
        .advance(transition)
        .await
        .unwrap_or_else(|error| panic!("advance to {expected:?} failed: {error:?}"))
    {
        AdmissionSpaceTransitionStepV2::Advanced(next) => next,
        AdmissionSpaceTransitionStepV2::Finished(_) => panic!("transition finished early"),
    };
    let AdmissionSpaceTransitionV2::CrossSpaceControl(current) = &next else {
        panic!("transition changed format");
    };
    assert_eq!(current.phase, expected);
    assert_eq!(current.profile_data_generation, [0x32; 16]);
    next
}

fn assert_no_forbidden_paths(root: &std::path::Path, forbidden: &[&str]) {
    for entry in std::fs::read_dir(root).unwrap().filter_map(Result::ok) {
        assert!(!forbidden.contains(&entry.file_name().to_string_lossy().as_ref()));
        if entry.file_type().unwrap().is_dir() {
            assert_no_forbidden_paths(&entry.path(), forbidden);
        }
    }
}

fn preparation(
    space: &SpaceId,
    target_access_state: Vec<u8>,
) -> AdmissionSpaceTransitionPreparationV2 {
    let attempt = [0x41; 32];
    let catalog = AdmissionContentKeyCatalogV1::new(
        "target-content-key",
        1,
        vec![
            AdmissionContentKeyEntryV1::new("legacy-v1", 0, vec![0x42; 32]).unwrap(),
            AdmissionContentKeyEntryV1::new("target-content-key", 1, vec![0x43; 32]).unwrap(),
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
                history_digest: [0x43; 32],
            },
            [0x44; 32],
            1,
            0,
            1,
            [0x45; 32],
            [0x46; 32],
            [0x47; 32],
            catalog.digest(),
            [0x48; 32],
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
