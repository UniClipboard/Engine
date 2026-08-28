//! 旧版 switch-space 状态的私有只读恢复器。
//!
//! 新流程不会写 `.migration_state`。这里仅解析旧版本已经留下的文件并
//! 向前完成或清理，恢复结束后删除文件。

use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs;

use uc_core::crypto::domain::{Aad, ActiveSpace, Ciphertext};
use uc_core::ids::SpaceId;
use uc_core::ports::clipboard::BlobMigrationRepoPort;
use uc_core::ports::security::{BlobCipherError, BlobCipherPort, KeyMigrationPort, MigrationRunId};
use uc_core::ports::setup::{LegacyMigrationRecoveryError, LegacyMigrationRecoveryPort};
use uc_observability_contract::analytics::AnalyticsFacade;
use uuid::Uuid;

pub const DEFAULT_MIGRATION_STATE_FILE: &str = ".migration_state";

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum LegacyMigrationPhaseV1 {
    Prepared {
        run_id: MigrationRunId,
        #[serde(default)]
        preserved_unreadable_records: u64,
    },
    HandshakeDone {
        run_id: MigrationRunId,
        target_space_id: SpaceId,
        #[serde(default)]
        sponsor_space_person_id: Option<Uuid>,
        #[serde(default)]
        preserved_unreadable_records: u64,
    },
    Swapped {
        run_id: MigrationRunId,
        target_space_id: SpaceId,
        #[serde(default)]
        sponsor_space_person_id: Option<Uuid>,
        #[serde(default)]
        preserved_unreadable_records: u64,
    },
}

async fn read_legacy_phase(
    state_file_path: &std::path::Path,
) -> Result<Option<LegacyMigrationPhaseV1>, LegacyMigrationRecoveryError> {
    let content = match fs::read_to_string(state_file_path).await {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(LegacyMigrationRecoveryError::Internal(error.to_string())),
    };
    if content.trim().is_empty() {
        return Ok(None);
    }
    serde_json::from_str::<Option<LegacyMigrationPhaseV1>>(&content)
        .map_err(|_| LegacyMigrationRecoveryError::RecoveryRequired)
}

pub(crate) async fn legacy_migration_run_id(
    base_dir: &std::path::Path,
) -> Result<Option<MigrationRunId>, LegacyMigrationRecoveryError> {
    Ok(
        read_legacy_phase(&base_dir.join(DEFAULT_MIGRATION_STATE_FILE))
            .await?
            .map(|phase| match phase {
                LegacyMigrationPhaseV1::Prepared { run_id, .. }
                | LegacyMigrationPhaseV1::HandshakeDone { run_id, .. }
                | LegacyMigrationPhaseV1::Swapped { run_id, .. } => run_id,
            }),
    )
}

pub struct FileLegacyMigrationRecovery {
    state_file_path: PathBuf,
    key_migration: Arc<dyn KeyMigrationPort>,
    blob_migration_repo: Arc<dyn BlobMigrationRepoPort>,
    blob_cipher: Arc<dyn BlobCipherPort>,
    analytics: Arc<dyn AnalyticsFacade>,
}

impl FileLegacyMigrationRecovery {
    pub fn with_defaults(
        base_dir: PathBuf,
        key_migration: Arc<dyn KeyMigrationPort>,
        blob_migration_repo: Arc<dyn BlobMigrationRepoPort>,
        blob_cipher: Arc<dyn BlobCipherPort>,
        analytics: Arc<dyn AnalyticsFacade>,
    ) -> Self {
        Self {
            state_file_path: base_dir.join(DEFAULT_MIGRATION_STATE_FILE),
            key_migration,
            blob_migration_repo,
            blob_cipher,
            analytics,
        }
    }

    async fn read_phase(
        &self,
    ) -> Result<Option<LegacyMigrationPhaseV1>, LegacyMigrationRecoveryError> {
        read_legacy_phase(&self.state_file_path).await
    }

    async fn remove_state_file(&self) -> Result<(), LegacyMigrationRecoveryError> {
        match fs::remove_file(&self.state_file_path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(LegacyMigrationRecoveryError::Internal(error.to_string())),
        }
    }

    async fn verify_backup(
        &self,
        run_id: &MigrationRunId,
    ) -> Result<(), LegacyMigrationRecoveryError> {
        for record in self
            .blob_migration_repo
            .list_records()
            .await
            .map_err(internal)?
        {
            let aad = Aad::from(uc_core::crypto::aad::for_inline(
                &record.event_id,
                &record.representation_id,
            ));
            self.key_migration
                .decrypt_with_migration_key(
                    run_id,
                    &Ciphertext::new(record.migration_ciphertext),
                    &aad,
                )
                .await
                .map_err(|_| LegacyMigrationRecoveryError::RecoveryRequired)?;
        }
        Ok(())
    }

    async fn verify_main_records(
        &self,
        expected_unreadable: u64,
    ) -> Result<(), LegacyMigrationRecoveryError> {
        let active = ActiveSpace::new(SpaceId::from_str("space"));
        let mut unreadable = 0_u64;
        for (event_id, representation_id) in self
            .blob_migration_repo
            .list_main_inline_representations()
            .await
            .map_err(internal)?
        {
            let Some(bytes) = self
                .blob_migration_repo
                .read_main_inline_data(&event_id, &representation_id)
                .await
                .map_err(internal)?
            else {
                continue;
            };
            let aad = Aad::from(uc_core::crypto::aad::for_inline(
                &event_id,
                &representation_id,
            ));
            match self
                .blob_cipher
                .decrypt(&active, &Ciphertext::new(bytes), &aad)
                .await
            {
                Ok(_) => {}
                Err(BlobCipherError::InvalidCiphertext) => unreadable += 1,
                Err(_) => return Err(LegacyMigrationRecoveryError::RecoveryRequired),
            }
        }
        if unreadable != expected_unreadable {
            return Err(LegacyMigrationRecoveryError::RecoveryRequired);
        }
        Ok(())
    }

    async fn rewrap_backup(
        &self,
        run_id: &MigrationRunId,
    ) -> Result<(), LegacyMigrationRecoveryError> {
        let active = ActiveSpace::new(SpaceId::from_str("space"));
        for record in self
            .blob_migration_repo
            .list_records()
            .await
            .map_err(internal)?
        {
            let aad = Aad::from(uc_core::crypto::aad::for_inline(
                &record.event_id,
                &record.representation_id,
            ));
            let plaintext = self
                .key_migration
                .decrypt_with_migration_key(
                    run_id,
                    &Ciphertext::new(record.migration_ciphertext),
                    &aad,
                )
                .await
                .map_err(|_| LegacyMigrationRecoveryError::RecoveryRequired)?;
            let ciphertext = self
                .blob_cipher
                .encrypt(&active, &plaintext, &aad)
                .await
                .map_err(|_| LegacyMigrationRecoveryError::RecoveryRequired)?;
            self.blob_migration_repo
                .update_main_inline_data(
                    &record.event_id,
                    &record.representation_id,
                    ciphertext.as_bytes(),
                )
                .await
                .map_err(internal)?;
        }
        Ok(())
    }

    async fn cleanup(&self, run_id: &MigrationRunId) -> Result<(), LegacyMigrationRecoveryError> {
        self.blob_migration_repo
            .discard_all_records()
            .await
            .map_err(internal)?;
        self.key_migration
            .discard_migration_key(run_id)
            .await
            .map_err(internal)?;
        self.remove_state_file().await
    }

    async fn finish_target(
        &self,
        run_id: &MigrationRunId,
        sponsor_space_person_id: Option<Uuid>,
        preserved_unreadable_records: u64,
        needs_rewrap: bool,
    ) -> Result<(), LegacyMigrationRecoveryError> {
        self.verify_backup(run_id).await?;
        if needs_rewrap {
            self.rewrap_backup(run_id).await?;
        }
        self.verify_main_records(preserved_unreadable_records)
            .await?;
        match sponsor_space_person_id {
            Some(person_id) => self.analytics.adopt_from_sponsor(person_id),
            None => self.analytics.release_to_solo(),
        }
        self.cleanup(run_id).await
    }
}

#[async_trait]
impl LegacyMigrationRecoveryPort for FileLegacyMigrationRecovery {
    async fn recover(&self) -> Result<(), LegacyMigrationRecoveryError> {
        let phase = self.read_phase().await?;
        let backup_count = self
            .blob_migration_repo
            .count_records()
            .await
            .map_err(internal)?;
        let Some(phase) = phase else {
            if backup_count != 0 {
                return Err(LegacyMigrationRecoveryError::RecoveryRequired);
            }
            return self.remove_state_file().await;
        };
        match phase {
            LegacyMigrationPhaseV1::Prepared {
                run_id,
                preserved_unreadable_records,
            } => {
                self.verify_main_records(preserved_unreadable_records)
                    .await?;
                self.verify_backup(&run_id).await?;
                self.cleanup(&run_id).await
            }
            LegacyMigrationPhaseV1::HandshakeDone {
                run_id,
                target_space_id: _,
                sponsor_space_person_id,
                preserved_unreadable_records,
            } => {
                self.finish_target(
                    &run_id,
                    sponsor_space_person_id,
                    preserved_unreadable_records,
                    true,
                )
                .await
            }
            LegacyMigrationPhaseV1::Swapped {
                run_id,
                target_space_id: _,
                sponsor_space_person_id,
                preserved_unreadable_records,
            } => {
                self.finish_target(
                    &run_id,
                    sponsor_space_person_id,
                    preserved_unreadable_records,
                    false,
                )
                .await
            }
        }
    }
}

fn internal(error: impl std::fmt::Display) -> LegacyMigrationRecoveryError {
    LegacyMigrationRecoveryError::Internal(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use uc_core::crypto::domain::Plaintext;
    use uc_core::ids::{EventId, RepresentationId};
    use uc_core::ports::clipboard::{BlobMigrationRepoError, MigrationRecord};
    use uc_core::ports::security::{BlobCipherError, KeyMigrationError, MigrationRunId};

    #[derive(Default)]
    struct RecoveryBlobRepo {
        main: Mutex<Vec<((EventId, RepresentationId), Vec<u8>)>>,
        backup: Mutex<Vec<MigrationRecord>>,
    }

    #[async_trait]
    impl BlobMigrationRepoPort for RecoveryBlobRepo {
        async fn list_main_inline_representations(
            &self,
        ) -> Result<Vec<(EventId, RepresentationId)>, BlobMigrationRepoError> {
            Ok(self
                .main
                .lock()
                .unwrap()
                .iter()
                .map(|(key, _)| key.clone())
                .collect())
        }

        async fn read_main_inline_data(
            &self,
            event_id: &EventId,
            representation_id: &RepresentationId,
        ) -> Result<Option<Vec<u8>>, BlobMigrationRepoError> {
            Ok(self
                .main
                .lock()
                .unwrap()
                .iter()
                .find(|((stored_event, stored_representation), _)| {
                    stored_event == event_id && stored_representation == representation_id
                })
                .map(|(_, bytes)| bytes.clone()))
        }

        async fn upsert_record(
            &self,
            record: &MigrationRecord,
        ) -> Result<(), BlobMigrationRepoError> {
            self.backup.lock().unwrap().push(record.clone());
            Ok(())
        }

        async fn count_records(&self) -> Result<u64, BlobMigrationRepoError> {
            Ok(self.backup.lock().unwrap().len() as u64)
        }

        async fn list_records(&self) -> Result<Vec<MigrationRecord>, BlobMigrationRepoError> {
            Ok(self.backup.lock().unwrap().clone())
        }

        async fn update_main_inline_data(
            &self,
            event_id: &EventId,
            representation_id: &RepresentationId,
            new_ciphertext: &[u8],
        ) -> Result<(), BlobMigrationRepoError> {
            let mut main = self.main.lock().unwrap();
            if let Some((_, bytes)) =
                main.iter_mut()
                    .find(|((stored_event, stored_representation), _)| {
                        stored_event == event_id && stored_representation == representation_id
                    })
            {
                *bytes = new_ciphertext.to_vec();
            }
            Ok(())
        }

        async fn mark_unreadable_inline_data(
            &self,
            _event_id: &EventId,
            _representation_id: &RepresentationId,
        ) -> Result<(), BlobMigrationRepoError> {
            Ok(())
        }

        async fn discard_all_records(&self) -> Result<(), BlobMigrationRepoError> {
            self.backup.lock().unwrap().clear();
            Ok(())
        }
    }

    #[derive(Default)]
    struct RecoveryKeyMigration {
        discarded: Mutex<Vec<MigrationRunId>>,
    }

    #[async_trait]
    impl KeyMigrationPort for RecoveryKeyMigration {
        async fn prepare_migration_key(&self) -> Result<MigrationRunId, KeyMigrationError> {
            Ok(MigrationRunId::new("unused"))
        }

        async fn encrypt_with_migration_key(
            &self,
            _run_id: &MigrationRunId,
            plaintext: &Plaintext,
            _aad: &Aad,
        ) -> Result<Ciphertext, KeyMigrationError> {
            Ok(Ciphertext::new(plaintext.as_bytes().to_vec()))
        }

        async fn decrypt_with_migration_key(
            &self,
            run_id: &MigrationRunId,
            ciphertext: &Ciphertext,
            _aad: &Aad,
        ) -> Result<Plaintext, KeyMigrationError> {
            if run_id.as_str() != "run-legacy" || ciphertext.as_bytes() == b"bad-backup" {
                return Err(KeyMigrationError::NotFound(run_id.clone()));
            }
            Ok(Plaintext::new(ciphertext.as_bytes().to_vec()))
        }

        async fn discard_migration_key(
            &self,
            run_id: &MigrationRunId,
        ) -> Result<(), KeyMigrationError> {
            self.discarded.lock().unwrap().push(run_id.clone());
            Ok(())
        }
    }

    struct RecoveryBlobCipher;

    #[async_trait]
    impl BlobCipherPort for RecoveryBlobCipher {
        async fn encrypt(
            &self,
            _space: &ActiveSpace,
            plaintext: &Plaintext,
            _aad: &Aad,
        ) -> Result<Ciphertext, BlobCipherError> {
            Ok(Ciphertext::new(plaintext.as_bytes().to_vec()))
        }

        async fn decrypt(
            &self,
            _space: &ActiveSpace,
            ciphertext: &Ciphertext,
            _aad: &Aad,
        ) -> Result<Plaintext, BlobCipherError> {
            if ciphertext.as_bytes() == b"unreadable" {
                return Err(BlobCipherError::InvalidCiphertext);
            }
            Ok(Plaintext::new(ciphertext.as_bytes().to_vec()))
        }
    }

    #[derive(Default)]
    struct RecoveryMarker;

    fn record(ciphertext: &[u8]) -> MigrationRecord {
        MigrationRecord {
            event_id: EventId::from_string("event-1".into()),
            representation_id: RepresentationId::from("representation-1"),
            migration_ciphertext: ciphertext.to_vec(),
        }
    }

    fn recovery_fixture(
        directory: &tempfile::TempDir,
        backup: Vec<MigrationRecord>,
        main_ciphertext: Option<&[u8]>,
    ) -> (
        FileLegacyMigrationRecovery,
        Arc<RecoveryBlobRepo>,
        Arc<RecoveryKeyMigration>,
        Arc<RecoveryMarker>,
    ) {
        let blobs = Arc::new(RecoveryBlobRepo::default());
        *blobs.backup.lock().unwrap() = backup;
        if let Some(ciphertext) = main_ciphertext {
            blobs.main.lock().unwrap().push((
                (
                    EventId::from_string("event-1".into()),
                    RepresentationId::from("representation-1"),
                ),
                ciphertext.to_vec(),
            ));
        }
        let keys = Arc::new(RecoveryKeyMigration::default());
        let marker = Arc::new(RecoveryMarker::default());
        let recovery = FileLegacyMigrationRecovery::with_defaults(
            directory.path().to_path_buf(),
            keys.clone(),
            blobs.clone(),
            Arc::new(RecoveryBlobCipher),
            Arc::new(uc_observability_contract::analytics::NoopAnalyticsFacade),
        );
        (recovery, blobs, keys, marker)
    }

    async fn write_legacy_state(directory: &tempfile::TempDir, json: &str) {
        fs::write(directory.path().join(DEFAULT_MIGRATION_STATE_FILE), json)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn orphan_backup_is_preserved_for_manual_recovery() {
        let directory = tempfile::tempdir().unwrap();
        let (recovery, blobs, keys, _) =
            recovery_fixture(&directory, vec![record(b"backup")], Some(b"source"));

        assert!(matches!(
            recovery.recover().await,
            Err(LegacyMigrationRecoveryError::RecoveryRequired)
        ));
        assert_eq!(blobs.backup.lock().unwrap().len(), 1);
        assert!(keys.discarded.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn prepared_state_cleans_only_after_source_and_backup_verify() {
        let directory = tempfile::tempdir().unwrap();
        write_legacy_state(
            &directory,
            r#"{"kind":"prepared","run_id":"run-legacy","preserved_unreadable_records":0}"#,
        )
        .await;
        let (recovery, blobs, keys, _) =
            recovery_fixture(&directory, vec![record(b"backup")], Some(b"source"));

        recovery.recover().await.unwrap();

        assert!(blobs.backup.lock().unwrap().is_empty());
        assert_eq!(keys.discarded.lock().unwrap().len(), 1);
        assert!(!directory.path().join(DEFAULT_MIGRATION_STATE_FILE).exists());
    }

    #[tokio::test]
    async fn handshake_done_rewraps_verifies_and_finishes_target() {
        let directory = tempfile::tempdir().unwrap();
        write_legacy_state(
            &directory,
            r#"{"kind":"handshake_done","run_id":"run-legacy","target_space_id":"target-space","preserved_unreadable_records":0}"#,
        )
        .await;
        let (recovery, blobs, _, marker) =
            recovery_fixture(&directory, vec![record(b"target")], Some(b"old"));

        recovery.recover().await.unwrap();

        let _ = marker;
        assert_eq!(
            blobs
                .main
                .lock()
                .unwrap()
                .first()
                .map(|(_, bytes)| bytes.clone()),
            Some(b"target".to_vec())
        );
        assert!(blobs.backup.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn swapped_state_verifies_main_before_cleanup() {
        let directory = tempfile::tempdir().unwrap();
        write_legacy_state(
            &directory,
            r#"{"kind":"swapped","run_id":"run-legacy","target_space_id":"target-space","preserved_unreadable_records":0}"#,
        )
        .await;
        let (recovery, blobs, _, marker) =
            recovery_fixture(&directory, vec![record(b"target")], Some(b"target"));

        recovery.recover().await.unwrap();

        let _ = marker;
        assert!(blobs.backup.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn corrupt_or_inconsistent_state_preserves_all_artifacts() {
        let directory = tempfile::tempdir().unwrap();
        write_legacy_state(&directory, "not-json").await;
        let (recovery, blobs, keys, marker) =
            recovery_fixture(&directory, vec![record(b"backup")], Some(b"source"));

        assert!(matches!(
            recovery.recover().await,
            Err(LegacyMigrationRecoveryError::RecoveryRequired)
        ));
        assert_eq!(blobs.backup.lock().unwrap().len(), 1);
        assert!(keys.discarded.lock().unwrap().is_empty());
        let _ = marker;
        assert!(directory.path().join(DEFAULT_MIGRATION_STATE_FILE).exists());

        write_legacy_state(
            &directory,
            r#"{"kind":"prepared","run_id":"run-legacy","preserved_unreadable_records":1}"#,
        )
        .await;
        assert!(matches!(
            recovery.recover().await,
            Err(LegacyMigrationRecoveryError::RecoveryRequired)
        ));
        assert_eq!(blobs.backup.lock().unwrap().len(), 1);
        assert!(keys.discarded.lock().unwrap().is_empty());
    }
}
