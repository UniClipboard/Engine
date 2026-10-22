//! 逐项保存使用真实SQLite及产物AEAD；恢复只允许清理尚未交付的产物。
#[path = "file_failure.rs"]
mod file_failure;
#[path = "progress_v3.rs"]
mod v3;
use super::*;
use std::path::PathBuf;
use uc_core::ports::{
    CleanupReceiveArtifactsPort, ListUnsettledReceiveArtifactsPort, PendingInboundTransfer,
    ReceiveItemRole, RecordReceiverTransferPort,
};
use uc_infra_local::fs::receive_artifact_cleanup::FsReceiveArtifactCleaner;
use uc_infra_storage::db::repositories::DieselFileTransferRepository;
use uc_infra_storage::db::schema::{clipboard_event, file_transfer};

struct Fixture {
    directory: tempfile::TempDir,
    executor: DieselSqliteExecutor,
    attempts: DieselEntryReceiveAttemptRepository<DieselSqliteExecutor>,
    journal: DieselReceiveArtifactLogRepository<DieselSqliteExecutor>,
    commit: DieselInboundReceiveCommitRepository<DieselSqliteExecutor>,
    artifacts: Vec<ReceiveArtifact>,
}
impl Fixture {
    async fn new() -> Self {
        let directory = tempdir().unwrap();
        let database = directory.path().join("progress.sqlite");
        let executor = DieselSqliteExecutor::new(init_db_pool(database.to_str().unwrap()).unwrap());
        let keys = Arc::new(FixedSubkey);
        let profile = Arc::new(FixedProfile);
        let attempts = DieselEntryReceiveAttemptRepository::new(executor.clone());
        attempts
            .begin_first_receive("entry", "a1", 1)
            .await
            .unwrap();
        let transfers =
            DieselFileTransferRepository::new(executor.clone(), keys.clone(), profile.clone());
        let mut artifacts = Vec::new();
        for index in 0..2 {
            let path = directory.path().join(format!("private-output-{index}"));
            std::fs::write(&path, b"saved-content").unwrap();
            let id = format!("entry:attempt:a1:member:{index}");
            transfers
                .upsert_pending_transfer(&PendingInboundTransfer {
                    item_role: Some(ReceiveItemRole::File),
                    transfer_id: id.clone(),
                    entry_id: "entry".into(),
                    attempt_id: Some("a1".into()),
                    origin_device_id: "peer".into(),
                    filename: "PRIVATE_FILENAME".into(),
                    file_size: Some(13),
                    cached_path: path.to_str().unwrap().into(),
                    created_at_ms: 1,
                })
                .await
                .unwrap();
            artifacts.push(ReceiveArtifact {
                item_id: id,
                staged_path: path.clone(),
                final_path: path,
                ownership: ReceiveArtifactOwnership::ManagedStaging,
            });
        }
        executor
            .run(|conn| {
                diesel::update(file_transfer::table)
                    .set(file_transfer::status.eq("completed"))
                    .execute(conn)?;
                Ok(())
            })
            .unwrap();
        let journal = DieselReceiveArtifactLogRepository::new(
            executor.clone(),
            keys.clone(),
            profile.clone(),
        );
        journal
            .record_receive_artifacts(&ReceiveArtifactRecord {
                entry_id: "entry".into(),
                attempt_id: "a1".into(),
                phase: ReceiveArtifactPhase::Preparing,
                resolution: ReceiveArtifactResolution::Pending,
                artifacts: artifacts.clone(),
                updated_at_ms: 1,
            })
            .await
            .unwrap();
        let commit = DieselInboundReceiveCommitRepository::new(executor.clone(), keys, profile);
        Self {
            directory,
            executor,
            attempts,
            journal,
            commit,
            artifacts,
        }
    }
    fn progress(&self, index: usize) -> InboundReceiveSettlement {
        InboundReceiveSettlement::Progress {
            delivered_item_ids: vec![self.artifacts[index].item_id.clone()],
            record: record("entry"),
            attempt_id: "a1".into(),
            artifact: self.artifacts[index].clone(),
            now_ms: 2,
        }
    }
    fn snapshot(&self) -> (i64, i64, Vec<String>, Vec<u8>, String) {
        self.executor
            .run(|conn| {
                Ok((
                    clipboard_entry::table.count().get_result(conn)?,
                    clipboard_event::table.count().get_result(conn)?,
                    file_transfer::table
                        .order(file_transfer::transfer_id)
                        .select(file_transfer::binding_state)
                        .load(conn)?,
                    receive_artifact_log::table
                        .select(receive_artifact_log::artifact_ciphertext)
                        .first(conn)?,
                    entry_receive_attempt::table
                        .select(entry_receive_attempt::attempt_state)
                        .first(conn)?,
                ))
            })
            .unwrap()
    }
}

#[tokio::test]
async fn progress_commit_keeps_receiving_and_recovery_preserves_delivered_file_after_reopen() {
    let fixture = Fixture::new().await;
    fixture
        .commit
        .commit_inbound_receive(&fixture.progress(0))
        .await
        .unwrap();
    let (entries, _, bindings, ciphertext, state) = fixture.snapshot();
    assert_eq!(entries, 1);
    assert_eq!(bindings, ["entry_bound", "attempt_bound"]);
    assert_eq!(state, "receiving");
    assert!(!ciphertext
        .windows(b"private-output".len())
        .any(|part| part == b"private-output"));
    let path = fixture.directory.path().join("progress.sqlite");
    let reopened = DieselReceiveArtifactLogRepository::new(
        DieselSqliteExecutor::new(init_db_pool(path.to_str().unwrap()).unwrap()),
        Arc::new(FixedSubkey),
        Arc::new(FixedProfile),
    );
    let pending = reopened.list_unsettled_receive_artifacts().await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].phase, ReceiveArtifactPhase::Preparing);
    assert_eq!(pending[0].artifacts, vec![fixture.artifacts[1].clone()]);
    FsReceiveArtifactCleaner
        .cleanup_receive_artifacts(&pending[0].artifacts)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(&fixture.artifacts[0].final_path).unwrap(),
        b"saved-content"
    );
    assert!(!fixture.artifacts[1].final_path.exists());
}

#[tokio::test]
async fn cancellation_wins_before_progress_commit_without_changing_saved_or_cleanup_rows() {
    let fixture = Fixture::new().await;
    fixture
        .attempts
        .request_receive_cancellation("entry", "a1", 2)
        .await
        .unwrap();
    let before = fixture.snapshot();
    assert!(matches!(
        fixture
            .commit
            .commit_inbound_receive(&fixture.progress(0))
            .await,
        Err(InboundReceiveCommitError::AttemptStateMismatch)
    ));
    assert_eq!(fixture.snapshot(), before);
}

#[tokio::test]
async fn progress_commit_rejects_changed_output_ownership_and_corrupt_journal() {
    let fixture = Fixture::new().await;
    let before = fixture.snapshot();
    let mut artifact = fixture.artifacts[0].clone();
    artifact.final_path = PathBuf::from("PRIVATE_FOREIGN_PATH");
    let progress = InboundReceiveSettlement::Progress {
        delivered_item_ids: vec![artifact.item_id.clone()],
        record: record("entry"),
        attempt_id: "a1".into(),
        artifact,
        now_ms: 2,
    };
    assert!(matches!(
        fixture.commit.commit_inbound_receive(&progress).await,
        Err(InboundReceiveCommitError::ArtifactStateMismatch)
    ));
    assert_eq!(fixture.snapshot(), before);
    fixture
        .executor
        .run(|conn| {
            diesel::update(receive_artifact_log::table)
                .set(receive_artifact_log::artifact_ciphertext.eq(vec![0u8; 40]))
                .execute(conn)?;
            Ok(())
        })
        .unwrap();
    let corrupt = fixture.snapshot();
    let error = fixture
        .commit
        .commit_inbound_receive(&fixture.progress(0))
        .await
        .unwrap_err();
    assert!(std::error::Error::source(&error).is_some());
    assert_eq!(fixture.snapshot(), corrupt);
}

#[tokio::test]
async fn late_delivery_failure_rolls_back_record_and_artifact_handoff_together() {
    let fixture = Fixture::new().await;
    let before = fixture.snapshot();
    fixture.executor.run(|conn| {
        diesel::sql_query("CREATE TRIGGER reject_progress_binding BEFORE UPDATE OF binding_state ON file_transfer BEGIN SELECT RAISE(ABORT, 'injected delivery failure'); END").execute(conn)?;
        Ok(())
    }).unwrap();
    let error = fixture
        .commit
        .commit_inbound_receive(&fixture.progress(0))
        .await
        .unwrap_err();
    assert!(std::error::Error::source(&error).is_some());
    assert_eq!(fixture.snapshot(), before);
    assert_eq!(
        fixture
            .journal
            .list_unsettled_receive_artifacts()
            .await
            .unwrap()[0]
            .artifacts,
        fixture.artifacts
    );
}

struct PausedSubkey {
    calls: std::sync::atomic::AtomicUsize,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
#[async_trait]
impl DeriveSpaceSubkeyPort for PausedSubkey {
    async fn derive_subkey(&self, salt: &[u8], info: &[u8]) -> Result<[u8; 32], SpaceAccessError> {
        if self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 1 {
            self.entered.notify_one();
            self.release.notified().await;
        }
        FixedSubkey.derive_subkey(salt, info).await
    }
}

#[tokio::test]
async fn concurrent_artifact_change_rejects_stale_progress_and_preserves_new_journal() {
    let fixture = Fixture::new().await;
    let keys = Arc::new(PausedSubkey {
        calls: std::sync::atomic::AtomicUsize::new(0),
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    let commit = DieselInboundReceiveCommitRepository::new(
        fixture.executor.clone(),
        keys.clone(),
        Arc::new(FixedProfile),
    );
    let progress = fixture.progress(0);
    let task = tokio::spawn(async move { commit.commit_inbound_receive(&progress).await });
    tokio::time::timeout(std::time::Duration::from_secs(5), keys.entered.notified())
        .await
        .unwrap();
    let mut changed = fixture.artifacts.clone();
    changed.reverse();
    fixture
        .journal
        .record_receive_artifacts(&ReceiveArtifactRecord {
            entry_id: "entry".into(),
            attempt_id: "a1".into(),
            phase: ReceiveArtifactPhase::Preparing,
            resolution: ReceiveArtifactResolution::Pending,
            artifacts: changed.clone(),
            updated_at_ms: 3,
        })
        .await
        .unwrap();
    let before = fixture.snapshot();
    keys.release.notify_one();
    assert!(matches!(
        task.await.unwrap(),
        Err(InboundReceiveCommitError::ArtifactStateMismatch)
    ));
    assert_eq!(fixture.snapshot(), before);
    assert_eq!(
        fixture
            .journal
            .list_unsettled_receive_artifacts()
            .await
            .unwrap()[0]
            .artifacts,
        changed
    );
}

#[tokio::test]
async fn sequential_progress_replaces_saved_snapshot_and_keeps_all_completed_items() {
    let fixture = Fixture::new().await;
    fixture
        .commit
        .commit_inbound_receive(&fixture.progress(0))
        .await
        .unwrap();
    let InboundReceiveRecord::Create {
        entry,
        mut event,
        representations,
        selection,
    } = record("entry")
    else {
        unreachable!()
    };
    event.event_id = EventId::from("event-entry-second");
    let replacement = InboundReceiveRecord::Replace {
        entry_id: entry.entry_id,
        new_event: event,
        new_representations: representations,
        new_selection: selection,
        new_total_size: 10,
        new_content_category: entry.content_category,
    };
    fixture
        .commit
        .commit_inbound_receive(&InboundReceiveSettlement::Progress {
            delivered_item_ids: vec![fixture.artifacts[1].item_id.clone()],
            record: replacement,
            attempt_id: "a1".into(),
            artifact: fixture.artifacts[1].clone(),
            now_ms: 3,
        })
        .await
        .unwrap();
    let (entries, events, bindings, _, state) = fixture.snapshot();
    assert_eq!((entries, events), (1, 1));
    assert_eq!(bindings, ["entry_bound", "entry_bound"]);
    assert_eq!(state, "receiving");
    assert!(fixture
        .journal
        .list_unsettled_receive_artifacts()
        .await
        .unwrap()[0]
        .artifacts
        .is_empty());
    fixture
        .attempts
        .request_receive_cancellation("entry", "a1", 4)
        .await
        .unwrap();
    fixture
        .commit
        .commit_inbound_receive(&InboundReceiveSettlement::NoEntry {
            entry_id: EntryId::from("entry"),
            attempt_id: "a1".into(),
            terminal: PartialReceiveTerminal::Cancelled,
            artifacts: NoEntryReceiveArtifacts::None,
            now_ms: 5,
        })
        .await
        .unwrap();
    let (entries, _, bindings, _, state) = fixture.snapshot();
    assert_eq!(entries, 1);
    assert_eq!(bindings, ["entry_bound", "entry_bound"]);
    assert_eq!(state, "cancelled");
    for artifact in &fixture.artifacts {
        assert_eq!(
            std::fs::read(&artifact.final_path).unwrap(),
            b"saved-content"
        );
    }
}

#[tokio::test]
async fn progress_commit_uses_real_inline_aead_before_sqlite_transaction() {
    use uc_core::crypto::{
        aad,
        domain::{Aad, Ciphertext},
    };
    use uc_core::ids::SpaceId;
    use uc_core::ports::security::BlobCipherPort;
    use uc_infra_profile::security::{BlobCipherAdapter, EncryptingInboundReceiveCommit};
    use uc_infra_security::{InMemorySession, MasterKey};
    use uc_infra_storage::db::schema::clipboard_snapshot_representation;
    let fixture = Fixture::new().await;
    let session = Arc::new(InMemorySession::new());
    let space = SpaceId::from_str("test-space");
    session.set_master_key_for_space(space.clone(), MasterKey::from_bytes(&[9; 32]).unwrap());
    let material = session.create_migrated_space_material(&space, 1).unwrap();
    session.install_space_material(&material).unwrap();
    let cipher = Arc::new(BlobCipherAdapter::new(session));
    let commit = EncryptingInboundReceiveCommit::new(
        Arc::new(DieselInboundReceiveCommitRepository::new(
            fixture.executor.clone(),
            Arc::new(FixedSubkey),
            Arc::new(FixedProfile),
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
    assert_ne!(ciphertext, b"hello");
    let aad = Aad::new(aad::for_inline(
        &EventId::from("event-entry"),
        &RepresentationId::from("rep-entry"),
    ));
    assert_eq!(
        cipher
            .decrypt(&Ciphertext::new(ciphertext.clone()), &aad)
            .await
            .unwrap()
            .as_bytes(),
        b"hello"
    );
    assert!(cipher
        .decrypt(&Ciphertext::new(ciphertext), &Aad::new(b"wrong".to_vec()))
        .await
        .is_err());
}

#[tokio::test]
async fn progress_refuses_unhanded_files_and_rolls_back_foreign_delivery_claims() {
    let fixture = Fixture::new().await;
    let before = fixture.snapshot();
    for ids in [
        Vec::new(),
        fixture
            .artifacts
            .iter()
            .map(|item| item.item_id.clone())
            .collect(),
    ] {
        let progress = InboundReceiveSettlement::Progress {
            record: record("entry"),
            attempt_id: "a1".into(),
            artifact: fixture.artifacts[0].clone(),
            delivered_item_ids: ids,
            now_ms: 2,
        };
        assert!(matches!(
            fixture.commit.commit_inbound_receive(&progress).await,
            Err(InboundReceiveCommitError::ArtifactStateMismatch)
        ));
        assert_eq!(fixture.snapshot(), before);
    }
    let progress = InboundReceiveSettlement::Progress {
        record: record("entry"),
        attempt_id: "a1".into(),
        artifact: fixture.artifacts[0].clone(),
        delivered_item_ids: vec![fixture.artifacts[0].item_id.clone(), "foreign".into()],
        now_ms: 2,
    };
    let error = fixture
        .commit
        .commit_inbound_receive(&progress)
        .await
        .unwrap_err();
    assert!(std::error::Error::source(&error).is_some());
    assert_eq!(fixture.snapshot(), before);
}
