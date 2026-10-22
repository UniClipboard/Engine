//! 生产V3保护路径下的逐项保存和产物恢复，不使用明文仓储替身。
use super::*;
use std::collections::BTreeMap;
use std::sync::Mutex;
use uc_core::crypto::{
    aad,
    domain::{Aad, Ciphertext},
};
use uc_core::ids::SpaceId;
use uc_core::ports::security::BlobCipherPort;
use uc_core::ports::{ListUnsettledReceiveArtifactsPort, SecureStorageError, SecureStoragePort};
use uc_infra_profile::security::EncryptingInboundReceiveCommit;
use uc_infra_security::{
    ContentProtection, InMemorySession, MasterKey, ProfileContentKeyVault, V3InlinePayloadCipher,
};
use uc_infra_storage::db::schema::clipboard_snapshot_representation;

#[derive(Default)]
struct Storage(Mutex<BTreeMap<String, Vec<u8>>>);
impl SecureStoragePort for Storage {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SecureStorageError> {
        Ok(self.0.lock().unwrap().get(key).cloned())
    }
    fn set(&self, key: &str, value: &[u8]) -> Result<(), SecureStorageError> {
        self.0.lock().unwrap().insert(key.into(), value.to_vec());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<(), SecureStorageError> {
        self.0.lock().unwrap().remove(key);
        Ok(())
    }
}

#[tokio::test]
async fn v3_progress_atomically_saves_encrypted_record_and_reseals_only_remaining_artifacts() {
    let fixture = Fixture::new().await;
    let space = SpaceId::from_str("test-space");
    let session = Arc::new(InMemorySession::new());
    session.set_master_key_for_space(space.clone(), MasterKey::from_bytes(&[9; 32]).unwrap());
    let material = session.create_migrated_space_material(&space, 1).unwrap();
    let vault = Arc::new(ProfileContentKeyVault::new(
        fixture.directory.path().join("vault"),
        Arc::new(Storage::default()),
        [0x31; 16],
    ));
    vault
        .install_verified_space_material(&material)
        .await
        .unwrap();
    session.install_space_material(&material).unwrap();
    let protection = Arc::new(ContentProtection::for_content(session, vault));
    let journal =
        DieselReceiveArtifactLogRepository::new_v3(fixture.executor.clone(), protection.clone());
    journal
        .record_receive_artifacts(&ReceiveArtifactRecord {
            entry_id: "entry".into(),
            attempt_id: "a1".into(),
            phase: ReceiveArtifactPhase::Preparing,
            resolution: ReceiveArtifactResolution::Pending,
            artifacts: fixture.artifacts.clone(),
            updated_at_ms: 1,
        })
        .await
        .unwrap();
    let cipher = Arc::new(V3InlinePayloadCipher::new(protection.clone()));
    let commit = EncryptingInboundReceiveCommit::new(
        Arc::new(DieselInboundReceiveCommitRepository::new_v3(
            fixture.executor.clone(),
            protection.clone(),
        )),
        cipher.clone(),
    );
    commit
        .commit_inbound_receive(&fixture.progress(0))
        .await
        .unwrap();
    let ciphertext = fixture
        .executor
        .run(|conn| {
            clipboard_snapshot_representation::table
                .select(clipboard_snapshot_representation::inline_data)
                .first::<Option<Vec<u8>>>(conn)
                .map_err(Into::into)
        })
        .unwrap()
        .unwrap();
    assert_eq!(&ciphertext[..4], b"UCP3");
    assert!(!ciphertext.windows(5).any(|part| part == b"hello"));
    let aad = Aad::new(aad::for_inline(
        &EventId::from("event-entry"),
        &RepresentationId::from("rep-entry"),
    ));
    assert_eq!(
        cipher
            .decrypt(&Ciphertext::new(ciphertext), &aad)
            .await
            .unwrap()
            .as_bytes(),
        b"hello"
    );
    let (_, _, bindings, artifacts, state) = fixture.snapshot();
    assert_eq!(bindings, ["entry_bound", "attempt_bound"]);
    assert_eq!(state, "receiving");
    assert_eq!(&artifacts[..4], b"UCP3");
    assert!(!artifacts
        .windows(b"private-output".len())
        .any(|part| part == b"private-output"));
    let path = fixture.directory.path().join("progress.sqlite");
    let reopened = DieselReceiveArtifactLogRepository::new_v3(
        DieselSqliteExecutor::new(init_db_pool(path.to_str().unwrap()).unwrap()),
        protection,
    );
    assert_eq!(
        reopened.list_unsettled_receive_artifacts().await.unwrap()[0].artifacts,
        vec![fixture.artifacts[1].clone()]
    );
}
