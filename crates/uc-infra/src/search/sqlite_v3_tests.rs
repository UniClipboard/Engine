use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use diesel::prelude::*;
use serde::Serialize;
use uc_core::ids::{EntryId, ProfileId, SpaceId};
use uc_core::membership::{
    ContentKeyId, GroupEpoch, ProtectionGroupId, SpaceKeyMaterial, SpaceKeyState,
};
use uc_core::ports::search::search_index::SearchIndexPort;
use uc_core::ports::security::current_profile::{CurrentProfileError, CurrentProfilePort};
use uc_core::ports::{SecureStorageError, SecureStoragePort};
use uc_core::search::document::{ContentType, SearchDocument, SearchPosting};
use uc_core::search::query::{QueryOperator, SearchQuery};

use super::search_key_derivation::term_tag;
use super::{SqliteSearchIndex, V3SearchProtection, V3_INDEX_VERSION};
use crate::db::pool::init_db_pool;
use crate::db::schema::search_document;
use crate::security::{MasterKey, ProfileContentKeyVault};
use crate::space::InMemorySession;

const PROFILE_ID: &str = "v3-profile";

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

struct FixedProfile;

#[async_trait]
impl CurrentProfilePort for FixedProfile {
    async fn current_profile(&self) -> Result<ProfileId, CurrentProfileError> {
        Ok(ProfileId::from(PROFILE_ID))
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
    group_id: &str,
    content_key_id: &str,
    epoch: u64,
    key_byte: u8,
) -> SpaceKeyMaterial {
    let state = SpaceKeyState::ready_for_admission(
        SpaceId::from_str(space_id),
        GroupEpoch::new(epoch),
        ContentKeyId::from_string(content_key_id).unwrap(),
        ProtectionGroupId::from_string(group_id).unwrap(),
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

fn document(entry_id: &str, preview: &str) -> SearchDocument {
    SearchDocument {
        entry_id: EntryId::from(entry_id),
        event_id: format!("event-{entry_id}").into(),
        active_time_ms: 1,
        captured_at_ms: 1,
        content_type: ContentType::Text,
        tags: Vec::new(),
        file_extensions: Vec::new(),
        mime_type: "text/plain".to_owned(),
        indexed_at_ms: 1,
        index_version: V3_INDEX_VERSION.to_owned(),
        text_preview: Some(preview.to_owned()),
        file_names: Vec::new(),
        file_paths: Vec::new(),
        link_urls: Vec::new(),
        source_device: None,
        payload_state: None,
        char_count: Some(preview.chars().count() as i64),
    }
}

async fn postings(
    protection: &V3SearchProtection,
    entry_id: &str,
    terms: &[&str],
) -> Vec<SearchPosting> {
    let context = protection.active_key_context().await.unwrap();
    terms
        .iter()
        .map(|term| SearchPosting {
            term_tag: term_tag(context.key(), term).unwrap(),
            entry_id: EntryId::from(entry_id),
            field_mask: 1,
            term_freq: 1,
            protection_ref: context.protection_ref().cloned(),
        })
        .collect()
}

fn and_query(query_string: &str) -> SearchQuery {
    SearchQuery {
        query_string: query_string.to_owned(),
        operator: QueryOperator::And,
        time_range: None,
        content_types: Vec::new(),
        tags: Vec::new(),
        extensions: Vec::new(),
        source_devices: Vec::new(),
        limit: 50,
        offset: 0,
    }
}

#[tokio::test]
async fn sqlite_v12_searches_multiple_groups_without_plaintext_persistence() {
    let directory = tempfile::tempdir().unwrap();
    let pool = init_db_pool(directory.path().join("search.sqlite").to_str().unwrap()).unwrap();
    let session = Arc::new(InMemorySession::new());
    let vault = Arc::new(ProfileContentKeyVault::new(
        directory.path().join("vault"),
        Arc::new(MemorySecureStorage::default()),
        [0xA1; 16],
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
    let protection = Arc::new(V3SearchProtection::new(
        Arc::clone(&session),
        Arc::clone(&vault),
    ));
    let index = SqliteSearchIndex::new_v3(
        pool.clone(),
        Arc::new(FixedProfile),
        Arc::clone(&protection),
    );

    let preview_a = "private preview alpha";
    index
        .index_entry(
            document("entry-a", preview_a),
            postings(&protection, "entry-a", &["shared", "alpha"]).await,
        )
        .await
        .unwrap();

    activate(&session, &material_b);
    let preview_b = "private preview beta";
    index
        .index_entry(
            document("entry-b", preview_b),
            postings(&protection, "entry-b", &["shared", "alpha"]).await,
        )
        .await
        .unwrap();

    let page = index.search(and_query("shared alpha")).await.unwrap();
    assert_eq!(page.total, 2);
    let mut previews = page
        .items
        .iter()
        .filter_map(|item| item.text_preview.clone())
        .collect::<Vec<_>>();
    previews.sort();
    assert_eq!(previews, vec![preview_a.to_owned(), preview_b.to_owned()]);

    let mut conn = pool.get().unwrap();
    let rows = search_document::table
        .filter(search_document::profile_id.eq(PROFILE_ID))
        .select((
            search_document::index_version,
            search_document::render_payload,
            search_document::protection_group_ref,
        ))
        .load::<(String, Option<Vec<u8>>, Option<Vec<u8>>)>(&mut conn)
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|(version, _, group_ref)| {
        version == V3_INDEX_VERSION && group_ref.as_ref().is_some_and(|value| value.len() == 32)
    }));
    assert_ne!(rows[0].2, rows[1].2);
    for (_, payload, _) in rows {
        let payload = payload.unwrap();
        for plaintext in [preview_a.as_bytes(), preview_b.as_bytes()] {
            assert!(!payload
                .windows(plaintext.len())
                .any(|window| window == plaintext));
        }
    }
    drop(conn);

    let (progress_tx, _progress_rx) = tokio::sync::mpsc::channel(8);
    index
        .rebuild(
            vec![(
                document("rebuilt-entry", "rebuilt private preview"),
                postings(&protection, "rebuilt-entry", &["rebuilt"]).await,
            )],
            progress_tx,
        )
        .await
        .unwrap();
    let rebuilt = index.search(and_query("rebuilt")).await.unwrap();
    assert_eq!(rebuilt.total, 1);
    assert_eq!(
        rebuilt.items[0].text_preview.as_deref(),
        Some("rebuilt private preview")
    );
}

#[tokio::test]
async fn sqlite_v12_rejects_postings_from_a_stale_active_group() {
    let directory = tempfile::tempdir().unwrap();
    let pool = init_db_pool(directory.path().join("search.sqlite").to_str().unwrap()).unwrap();
    let session = Arc::new(InMemorySession::new());
    let vault = Arc::new(ProfileContentKeyVault::new(
        directory.path().join("vault"),
        Arc::new(MemorySecureStorage::default()),
        [0xA2; 16],
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
    let protection = Arc::new(V3SearchProtection::new(
        Arc::clone(&session),
        Arc::clone(&vault),
    ));
    let stale = postings(&protection, "stale-entry", &["shared"]).await;
    activate(&session, &material_b);
    let index = SqliteSearchIndex::new_v3(
        pool.clone(),
        Arc::new(FixedProfile),
        Arc::clone(&protection),
    );

    let error = index
        .index_entry(document("stale-entry", "must not persist"), stale)
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("protection context changed before persistence"));
    let mut conn = pool.get().unwrap();
    let count = search_document::table
        .filter(search_document::profile_id.eq(PROFILE_ID))
        .count()
        .get_result::<i64>(&mut conn)
        .unwrap();
    assert_eq!(count, 0);
}
