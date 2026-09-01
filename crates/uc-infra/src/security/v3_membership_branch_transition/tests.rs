use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use sha2::{Digest as _, Sha256};
use tempfile::tempdir;
use uc_application::deps::{
    AdvanceMembershipBranchTransitionInput, AdvanceMembershipBranchTransitionPort,
    CommitMembershipLedgerPort, LoadMembershipLedgerPort, LoadedMembershipLedger,
    MembershipLedgerMutation, PrepareMembershipBranchRecoveryMaterialInput,
    PrepareMembershipBranchRecoveryMaterialPort, PrepareMembershipBranchRecoveryRecipientPort,
    PrepareMembershipBranchTransitionInput, PrepareMembershipBranchTransitionPort,
};
use uc_core::crypto::domain::Passphrase;
use uc_core::ids::{DeviceId, SpaceId};
use uc_core::membership::{
    ActiveRuntimeLayout, ActiveSpaceGenerationManifestV2, AdmissionChangeFacts, MembershipBranchId,
    MembershipBranchRecoveryPackageV1, MembershipBranchTransitionPhaseV1,
    MembershipBranchTransitionV1, MembershipConflictId, MembershipCredential,
    RevocationRepositoryPort, VersionedMembershipHistory, ED25519_SIGNATURE_ALGORITHM_V1,
};
use uc_core::ports::security::current_profile::CurrentProfilePort;
use uc_core::ports::space::SpaceAccessStore;
use uc_core::ports::{SecureStorageError, SecureStoragePort};

use super::V3MembershipBranchTransition;
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
    DefaultMembershipBranchTransitionPreparation, DefaultSpaceAccessAdapter, InMemorySession,
    KeyMaterialStore, SqliteMembershipLedger,
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
async fn v3_membership_branch_replaces_only_the_control_generation() {
    let directory = tempdir().unwrap();
    let root = directory.path().join("profile");
    let vault_root = root.join("vault");
    let secure_storage: Arc<dyn SecureStoragePort> = Arc::new(MemorySecureStorage::default());
    let current_profile: Arc<dyn CurrentProfilePort> = Arc::new(DefaultCurrentProfile::new());
    let admission_keys = Arc::new(AdmissionKeyManager::new(
        Arc::clone(&secure_storage),
        [0x91; 16],
    ));
    let manifests = Arc::new(ActiveSpaceGenerationManifestStore::new(
        vault_root.clone(),
        Arc::clone(&admission_keys),
    ));
    let space = SpaceId::from_str("branch-transition-space");
    let source = ActiveRuntimeManifestV3::new(
        ActiveRuntimeLayout::new(space.clone(), [0x92; 16], [0x93; 16]).unwrap(),
        [0x94; 16],
    )
    .unwrap();
    let legacy = ActiveSpaceGenerationManifestV2::new(
        space.as_ref().to_owned(),
        [0x94; 16],
        [0x95; 16],
        [0x96; 16],
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
    std::fs::write(source_layout.profile_database(), b"branch-retained-profile").unwrap();
    std::fs::create_dir_all(source_layout.blob_root()).unwrap();
    std::fs::write(
        source_layout.blob_root().join("history.ucbl"),
        b"branch-retained-blob",
    )
    .unwrap();
    std::fs::create_dir_all(source_layout.control_database().parent().unwrap()).unwrap();
    let control_pool = init_db_pool(source_layout.control_database().to_str().unwrap()).unwrap();
    let recipient_session = Arc::new(InMemorySession::new());
    let recipient_repository = Arc::new(DieselSpaceSecurityStore::new(
        Arc::new(DieselSqliteExecutor::new(control_pool.clone())),
        recipient_session.as_ref().clone(),
    ));
    let recipient_access = Arc::new(DefaultSpaceAccessAdapter::new_with_key_epoch_repository(
        Arc::new(KeyMaterialStore::new(
            Arc::clone(&secure_storage),
            Arc::new(JsonKeySlotStore::new(root.join("recipient-keys"))),
        )),
        Arc::clone(&current_profile),
        Arc::clone(&recipient_session),
        recipient_repository.clone() as Arc<dyn RevocationRepositoryPort>,
        Arc::new(ProfileContentKeyVault::new(
            vault_root.join("recipient-content"),
            Arc::clone(&secure_storage),
            [0x97; 16],
        )),
    ));

    let sponsor_storage: Arc<dyn SecureStoragePort> = Arc::new(MemorySecureStorage::default());
    let sponsor_pool = init_db_pool(root.join("sponsor.sqlite").to_str().unwrap()).unwrap();
    let sponsor_session = Arc::new(InMemorySession::new());
    let sponsor_repository = Arc::new(DieselSpaceSecurityStore::new(
        Arc::new(DieselSqliteExecutor::new(sponsor_pool)),
        sponsor_session.as_ref().clone(),
    ));
    let sponsor_access = DefaultSpaceAccessAdapter::new_with_security_repositories(
        Arc::new(KeyMaterialStore::new(
            Arc::clone(&sponsor_storage),
            Arc::new(JsonKeySlotStore::new(root.join("sponsor-keys"))),
        )),
        Arc::new(DefaultCurrentProfile::new()),
        Arc::clone(&sponsor_session),
        sponsor_repository.clone() as Arc<dyn RevocationRepositoryPort>,
        sponsor_repository as Arc<dyn uc_core::membership::LegacyBootstrapRepositoryPort>,
        Arc::new(ProfileContentKeyVault::new(
            vault_root.join("sponsor-content"),
            Arc::clone(&sponsor_storage),
            [0x98; 16],
        )),
    );
    SpaceAccessStore::initialize(
        &sponsor_access,
        &space,
        &Passphrase::new("sponsor transition passphrase"),
    )
    .await
    .unwrap();
    uc_core::membership::GroupBootstrapPort::bootstrap_legacy_space(
        &sponsor_access,
        &DeviceId::new("target-member"),
        &[],
        1,
    )
    .await
    .unwrap();

    let recipient_device = DeviceId::new("recipient-member");
    let pending = recipient_access
        .prepare_group_join(&recipient_device)
        .await
        .unwrap();
    let admission = sponsor_access
        .admit_group_member(
            &space,
            &DeviceId::new("target-member"),
            &recipient_device,
            &[],
            &pending.key_package,
        )
        .await
        .unwrap();
    recipient_access
        .install_group_join(
            &space,
            &Passphrase::new("recipient transition passphrase"),
            pending,
            &admission.welcome,
            &admission.encrypted_key_catalog,
            admission.group_epoch,
        )
        .await
        .unwrap();
    let group_info = sponsor_access
        .export_membership_branch_recovery_group_info()
        .await
        .unwrap();
    let recipient_recovery = recipient_access
        .prepare_membership_branch_recovery_recipient(group_info)
        .await
        .unwrap();
    let credential = MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0x99; 32]);
    let recipient_member = credential.member_instance_id(&recipient_device);
    let facts = AdmissionChangeFacts {
        member_instance: recipient_member,
        device_id: recipient_device.clone(),
        device_name: "recipient member".to_owned(),
        identity_fingerprint: uc_core::security::IdentityFingerprint::from_display_string(
            "ABCD-EFGH-IJKL-MNOP",
        )
        .unwrap(),
        transport_public_key: vec![0x9a],
        transport_address_blob: vec![0x9b],
        identity_signature: vec![0x9c],
    };
    let history = VersionedMembershipHistory::new_single_member_root(
        space.as_ref().to_owned(),
        facts,
        credential,
    )
    .unwrap();
    let target_history = history.encode_persisted_v2().unwrap();
    let conflict_id = MembershipConflictId::from_bytes([0x9d; 32]);
    let target_branch_id = MembershipBranchId::from_bytes([0x9e; 32]);
    let transition_id = MembershipBranchTransitionV1::derive_id(conflict_id, target_branch_id);
    let prepared_target = sponsor_access
        .prepare_membership_branch_recovery_material(PrepareMembershipBranchRecoveryMaterialInput {
            conflict_id,
            target_branch_id,
            recipient_member,
            target_history: history.clone(),
            external_commit: recipient_recovery.external_commit,
        })
        .await
        .unwrap();
    let package = MembershipBranchRecoveryPackageV1::new_unsigned(
        conflict_id,
        target_branch_id,
        recipient_member,
        recipient_member,
        10_000,
        [0x9f; 32],
        target_history.clone(),
        prepared_target.sealed_mls_recovery_material,
        prepared_target.encrypted_content_key_catalog,
    )
    .unwrap();
    let transition = DefaultMembershipBranchTransitionPreparation::new(Arc::clone(&manifests))
        .prepare_membership_branch_transition(PrepareMembershipBranchTransitionInput {
            transition_id,
            conflict_id,
            target_branch_id,
            package: package.clone(),
        })
        .await
        .unwrap();
    assert_eq!(transition.source_generation(), &[0x93; 16]);

    let executor = Arc::new(DieselSqliteExecutor::new(control_pool.clone()));
    let ledger = SqliteMembershipLedger::new(Arc::clone(&executor), Arc::clone(&admission_keys));
    ledger
        .compare_and_commit(MembershipLedgerMutation {
            expected_revision: 0,
            expected_history_digest: None,
            replacement: LoadedMembershipLedger {
                revision: 1,
                lineage_id: Some(space.as_ref().to_owned()),
                membership_history: Some(target_history),
                local_device_id: Some(recipient_device),
                local_member_instance: Some(recipient_member),
                local_join_active: true,
                peer_reconciliation: Default::default(),
                history_sync_cursor: None,
                inbound_transfers: Default::default(),
                completed_inbound_transfers: Default::default(),
                pending_effects: Default::default(),
                membership_conflicts: Default::default(),
                membership_branch_transitions: [(transition_id, transition.clone())]
                    .into_iter()
                    .collect(),
                consumed_membership_recovery_nonces: Default::default(),
                membership_branch_recovery_sessions: Default::default(),
            },
        })
        .await
        .unwrap();
    let generations = Arc::new(SpaceControlGeneration::new(
        root.clone(),
        Arc::clone(&recipient_access),
        current_profile,
        Arc::clone(&admission_keys),
    ));
    let activation = Arc::new(SpaceTransitionActivation::new(
        root.clone(),
        control_pool.clone(),
        Arc::clone(&manifests),
        Arc::clone(&generations),
        recipient_access,
    ));
    let transitioner = V3MembershipBranchTransition::new(
        control_pool,
        Arc::clone(&manifests),
        generations,
        activation,
    );

    let mut current = transition;
    for expected_phase in [
        MembershipBranchTransitionPhaseV1::SourceBackedUp,
        MembershipBranchTransitionPhaseV1::TargetVerified,
        MembershipBranchTransitionPhaseV1::TargetStaged,
        MembershipBranchTransitionPhaseV1::Promoted,
        MembershipBranchTransitionPhaseV1::RuntimeRestored,
        MembershipBranchTransitionPhaseV1::Completed,
    ] {
        let next = transitioner
            .advance_membership_branch_transition(AdvanceMembershipBranchTransitionInput {
                transition: current.clone(),
                recipient_staged_mls_state: recipient_recovery.staged_mls_state.clone(),
                recovery_package: package.clone(),
                target_history: history.clone(),
            })
            .await
            .unwrap_or_else(|error| {
                match error {
                uc_application::deps::AdvanceMembershipBranchTransitionError::Unavailable {
                    source,
                }
                | uc_application::deps::AdvanceMembershipBranchTransitionError::Invalid {
                    source,
                }
                | uc_application::deps::AdvanceMembershipBranchTransitionError::RecoveryRequired {
                    source,
                } => panic!(
                    "branch transition failed from {:?} to {expected_phase:?}: {source:#}",
                    current.phase()
                ),
            }
            });
        assert_eq!(next.phase(), expected_phase);
        let loaded = ledger.load().await.unwrap();
        let mut replacement = loaded.clone();
        replacement.revision += 1;
        replacement
            .membership_branch_transitions
            .insert(transition_id, next.clone());
        ledger
            .compare_and_commit(MembershipLedgerMutation {
                expected_revision: loaded.revision,
                expected_history_digest: loaded
                    .membership_history
                    .as_deref()
                    .map(|bytes| <[u8; 32]>::from(Sha256::digest(bytes))),
                replacement,
            })
            .await
            .unwrap();
        current = next;
    }

    let Some(ActiveRuntimeManifest::V3(active)) = manifests.load_runtime().await.unwrap() else {
        panic!("branch target manifest is not active");
    };
    assert_eq!(active.layout().space_id(), &space);
    assert_eq!(active.layout().profile_data_generation(), &[0x92; 16]);
    assert_eq!(active.keyslot_generation(), &[0x94; 16]);
    assert_eq!(
        active.layout().space_control_generation(),
        current.target_generation()
    );
    assert_eq!(
        std::fs::read(source_layout.profile_database()).unwrap(),
        b"branch-retained-profile"
    );
    assert_eq!(
        std::fs::read(source_layout.blob_root().join("history.ucbl")).unwrap(),
        b"branch-retained-blob"
    );
    assert!(!source_layout.control_database().exists());
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
