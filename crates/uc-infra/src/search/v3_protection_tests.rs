use std::collections::BTreeMap;
use std::error::Error as _;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use uc_core::ids::{EntryId, SpaceId};
use uc_core::membership::{
    ContentKeyId, GroupEpoch, ProtectionGroupId, SpaceKeyMaterial, SpaceKeyState,
};
use uc_core::ports::{SecureStorageError, SecureStoragePort};

use super::{RenderFields, SearchGroupRef, V3SearchProtection, V3SearchProtectionError};
use crate::security::{MasterKey, ProfileContentKeyVault};
use crate::space::InMemorySession;

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

#[derive(Serialize)]
struct CatalogFixture {
    version: u8,
    entries: Vec<CatalogEntryFixture>,
}

#[derive(Serialize)]
struct CatalogEntryFixture {
    content_key_id: String,
    epoch: u64,
    key: Vec<u8>,
}

fn ready_material(
    space_id: &str,
    protection_group_id: &str,
    content_key_id: &str,
    epoch: u64,
    key_byte: u8,
) -> SpaceKeyMaterial {
    let state = SpaceKeyState::ready_for_admission(
        SpaceId::from_str(space_id),
        GroupEpoch::new(epoch),
        ContentKeyId::from_string(content_key_id).unwrap(),
        ProtectionGroupId::from_string(protection_group_id).unwrap(),
    )
    .unwrap();
    let catalog = CatalogFixture {
        version: 2,
        entries: vec![
            CatalogEntryFixture {
                content_key_id: "legacy-v1".to_owned(),
                epoch: 0,
                key: vec![0x20; 32],
            },
            CatalogEntryFixture {
                content_key_id: content_key_id.to_owned(),
                epoch,
                key: vec![key_byte; 32],
            },
        ],
    };
    SpaceKeyMaterial::new(
        state,
        b"verified-group-state".to_vec(),
        serde_json::to_vec(&catalog).unwrap(),
        1,
    )
}

fn activate(session: &InMemorySession, material: &SpaceKeyMaterial) {
    session.set_master_key_for_space(
        material.state().space_id().clone(),
        MasterKey::from_bytes(&[0x20; 32]).unwrap(),
    );
    session.install_space_material(material).unwrap();
}

#[tokio::test]
async fn tags_are_group_separated_and_queries_keep_alternatives_per_term() {
    let directory = tempfile::tempdir().unwrap();
    let secure_storage = Arc::new(MemorySecureStorage::default());
    let session = Arc::new(InMemorySession::new());
    let vault = Arc::new(ProfileContentKeyVault::new(
        directory.path().join("vault"),
        secure_storage,
        [0x91; 16],
    ));
    let material_a = ready_material("space-a", "group-a", "key-a", 7, 0x41);
    let material_b = ready_material("space-b", "group-b", "key-b", 11, 0x42);
    vault
        .install_verified_space_material(&material_a)
        .await
        .unwrap();
    vault
        .install_verified_space_material(&material_b)
        .await
        .unwrap();
    activate(&session, &material_a);
    let protection = V3SearchProtection::new(Arc::clone(&session), Arc::clone(&vault));
    let terms = vec!["shared".to_owned(), "alpha".to_owned()];
    let indexed_a = protection.index_terms(&terms).await.unwrap();
    let entry_id = EntryId::from("entry-a");
    let fields = RenderFields::new(
        Some("private preview".to_owned()),
        vec!["private-name.txt".to_owned()],
        Vec::new(),
        Vec::new(),
        Some(15),
    );
    let render = protection.seal_render(&entry_id, &fields).await.unwrap();

    activate(&session, &material_b);
    let indexed_b = protection.index_terms(&terms).await.unwrap();

    assert_ne!(indexed_a.group_ref(), indexed_b.group_ref());
    assert_ne!(indexed_a.term_tags()[0], indexed_b.term_tags()[0]);
    assert_ne!(indexed_a.term_tags()[1], indexed_b.term_tags()[1]);
    let query = protection
        .query_terms(
            &[indexed_a.group_ref().clone(), indexed_b.group_ref().clone()],
            &terms,
        )
        .await
        .unwrap();
    assert_eq!(query.alternatives_by_term().len(), 2);
    assert_eq!(query.alternatives_by_term()[0].len(), 2);
    assert_eq!(query.alternatives_by_term()[1].len(), 2);
    assert!(query.alternatives_by_term()[0].contains(&indexed_a.term_tags()[0]));
    assert!(query.alternatives_by_term()[0].contains(&indexed_b.term_tags()[0]));
    assert!(query.alternatives_by_term()[1].contains(&indexed_a.term_tags()[1]));
    assert!(query.alternatives_by_term()[1].contains(&indexed_b.term_tags()[1]));

    let only_indexed_a = protection
        .query_terms(&[indexed_a.group_ref().clone()], &terms)
        .await
        .unwrap();
    assert!(only_indexed_a
        .alternatives_by_term()
        .iter()
        .all(|alternatives| alternatives.len() == 1));

    assert_eq!(
        protection.open_render(&entry_id, &render).await.unwrap(),
        fields
    );
    for forbidden in [
        b"private preview".as_slice(),
        b"private-name.txt",
        b"space-a",
        b"group-a",
        b"search",
    ] {
        assert!(!render
            .windows(forbidden.len())
            .any(|window| window == forbidden));
    }
    let error = protection
        .open_render(&EntryId::from("entry-b"), &render)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        V3SearchProtectionError::RenderDecode { .. }
    ));
    assert!(error.source().is_some());
}

#[tokio::test]
async fn group_refs_and_tags_survive_restart_but_unknown_index_refs_fail_closed() {
    let directory = tempfile::tempdir().unwrap();
    let secure_storage = Arc::new(MemorySecureStorage::default());
    let material = ready_material("space-a", "group-a", "key-a", 7, 0x41);
    let session = Arc::new(InMemorySession::new());
    let vault = Arc::new(ProfileContentKeyVault::new(
        directory.path().join("vault"),
        secure_storage.clone(),
        [0x92; 16],
    ));
    vault
        .install_verified_space_material(&material)
        .await
        .unwrap();
    activate(&session, &material);
    let before = V3SearchProtection::new(session, vault)
        .index_terms(&["stable".to_owned()])
        .await
        .unwrap();

    let reopened_session = Arc::new(InMemorySession::new());
    activate(&reopened_session, &material);
    let reopened_vault = Arc::new(ProfileContentKeyVault::new(
        directory.path().join("vault"),
        secure_storage,
        [0x92; 16],
    ));
    let reopened = V3SearchProtection::new(reopened_session, reopened_vault);
    let after = reopened.index_terms(&["stable".to_owned()]).await.unwrap();

    assert_eq!(before.group_ref(), after.group_ref());
    assert_eq!(before.term_tags(), after.term_tags());

    let unknown = SearchGroupRef::from_bytes(&[0xFF; 32]).unwrap();
    let error = reopened
        .query_terms(&[unknown], &["stable".to_owned()])
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        V3SearchProtectionError::InvalidGroupReferences { .. }
    ));
    assert!(error.source().is_some());
}

#[tokio::test]
async fn locked_indexing_and_invalid_group_ref_keep_stable_errors_with_sources() {
    let directory = tempfile::tempdir().unwrap();
    let vault = Arc::new(ProfileContentKeyVault::new(
        directory.path().join("vault"),
        Arc::new(MemorySecureStorage::default()),
        [0x93; 16],
    ));
    let protection = V3SearchProtection::new(Arc::new(InMemorySession::new()), vault);

    let locked = protection
        .index_terms(&["term".to_owned()])
        .await
        .unwrap_err();
    assert!(matches!(locked, V3SearchProtectionError::NotActive { .. }));
    assert!(locked.source().is_some());

    let invalid = SearchGroupRef::from_bytes(&[1; 31]).unwrap_err();
    assert!(matches!(
        invalid,
        V3SearchProtectionError::InvalidGroupReferences { .. }
    ));
    assert!(invalid.source().is_some());
}
