//! 真实SQLite已提交但执行器回执丢失，只能确认原结果，不能重写终态或产物。
use super::*;
use std::error::Error;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};

struct LostAcknowledgement {
    inner: DieselSqliteExecutor,
    lose_once: AtomicBool,
}
impl DbExecutor for LostAcknowledgement {
    fn run<T>(
        &self,
        f: impl FnOnce(&mut SqliteConnection) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let result = self.inner.run(f)?;
        if self.lose_once.swap(false, Ordering::SeqCst) {
            return Err(io::Error::from(io::ErrorKind::BrokenPipe).into());
        }
        Ok(result)
    }
}

#[tokio::test]
async fn no_entry_lost_commit_acknowledgement_is_confirmed_without_rewriting_terminal_rows() {
    for terminal in [
        PartialReceiveTerminal::Failed,
        PartialReceiveTerminal::Cancelled,
    ] {
        for resolution in [
            NoEntryReceiveArtifacts::None,
            NoEntryReceiveArtifacts::RolledBack,
        ] {
            let directory = tempdir().unwrap();
            let database = directory.path().join("lost-ack.sqlite");
            let executor =
                DieselSqliteExecutor::new(init_db_pool(database.to_str().unwrap()).unwrap());
            let subkey: Arc<dyn DeriveSpaceSubkeyPort> = Arc::new(FixedSubkey);
            let profile: Arc<dyn CurrentProfilePort> = Arc::new(FixedProfile);
            let attempts = DieselEntryReceiveAttemptRepository::new(executor.clone());
            attempts
                .begin_first_receive("entry", "a1", 1)
                .await
                .unwrap();
            match terminal {
                PartialReceiveTerminal::Failed => {
                    attempts
                        .begin_receive_failure("entry", "a1", 2)
                        .await
                        .unwrap();
                }
                PartialReceiveTerminal::Cancelled => {
                    attempts
                        .request_receive_cancellation("entry", "a1", 2)
                        .await
                        .unwrap();
                }
            }
            if resolution == NoEntryReceiveArtifacts::RolledBack {
                let output = directory.path().join("owned.bin");
                std::fs::write(&output, b"owned output").unwrap();
                let artifacts = DieselReceiveArtifactLogRepository::new(
                    executor.clone(),
                    subkey.clone(),
                    profile.clone(),
                );
                artifacts
                    .record_receive_artifacts(&ReceiveArtifactRecord {
                        entry_id: "entry".into(),
                        attempt_id: "a1".into(),
                        phase: ReceiveArtifactPhase::RollingBack,
                        resolution: ReceiveArtifactResolution::Pending,
                        artifacts: vec![ReceiveArtifact {
                            item_id: "item".into(),
                            staged_path: output.clone(),
                            final_path: output.clone(),
                            ownership: ReceiveArtifactOwnership::ManagedStaging,
                        }],
                        updated_at_ms: 2,
                    })
                    .await
                    .unwrap();
                std::fs::remove_file(output).unwrap();
            }
            let commit = DieselInboundReceiveCommitRepository::new(
                LostAcknowledgement {
                    inner: executor.clone(),
                    lose_once: AtomicBool::new(true),
                },
                subkey,
                profile,
            );
            let settlement = |now_ms| InboundReceiveSettlement::NoEntry {
                entry_id: EntryId::from("entry"),
                attempt_id: "a1".into(),
                terminal,
                artifacts: resolution,
                now_ms,
            };
            let error = commit
                .commit_inbound_receive(&settlement(3))
                .await
                .unwrap_err();
            let InboundReceiveCommitError::Backend(source) = error else {
                panic!("实际回执错误必须保留来源");
            };
            let mut current = Some(source.as_ref() as &(dyn Error + 'static));
            let mut original_io = None;
            while let Some(cause) = current {
                if let Some(source) = cause.downcast_ref::<io::Error>() {
                    original_io = Some(source.kind());
                    break;
                }
                current = cause.source();
            }
            assert_eq!(original_io, Some(io::ErrorKind::BrokenPipe));

            let before = executor
                .run(|conn| {
                    let attempt = entry_receive_attempt::table
                        .select((
                            entry_receive_attempt::attempt_state,
                            entry_receive_attempt::updated_at_ms,
                        ))
                        .first::<(String, i64)>(conn)?;
                    let artifacts = receive_artifact_log::table
                        .select((
                            receive_artifact_log::phase,
                            receive_artifact_log::resolution,
                            receive_artifact_log::artifact_ciphertext,
                            receive_artifact_log::updated_at_ms,
                        ))
                        .load::<(String, String, Vec<u8>, i64)>(conn)?;
                    Ok((attempt, artifacts))
                })
                .unwrap();
            assert_eq!(
                before.0 .0,
                match terminal {
                    PartialReceiveTerminal::Failed => "failed",
                    PartialReceiveTerminal::Cancelled => "cancelled",
                }
            );
            // 成功确认不能执行任何新UPDATE；时间戳和AEAD密文也必须保持。
            executor.run(|conn| {
                diesel::sql_query("CREATE TRIGGER reject_terminal_rewrite BEFORE UPDATE ON entry_receive_attempt BEGIN SELECT RAISE(ABORT, 'terminal rewrite'); END").execute(conn)?;
                diesel::sql_query("CREATE TRIGGER reject_artifact_rewrite BEFORE UPDATE ON receive_artifact_log BEGIN SELECT RAISE(ABORT, 'artifact rewrite'); END").execute(conn)?;
                Ok(())
            }).unwrap();
            commit
                .commit_inbound_receive(&settlement(99))
                .await
                .expect("真实终态已提交，原NoEntry应只读确认");
            let reopened =
                DieselSqliteExecutor::new(init_db_pool(database.to_str().unwrap()).unwrap());
            let after = reopened
                .run(|conn| {
                    let attempt = entry_receive_attempt::table
                        .select((
                            entry_receive_attempt::attempt_state,
                            entry_receive_attempt::updated_at_ms,
                        ))
                        .first::<(String, i64)>(conn)?;
                    let artifacts = receive_artifact_log::table
                        .select((
                            receive_artifact_log::phase,
                            receive_artifact_log::resolution,
                            receive_artifact_log::artifact_ciphertext,
                            receive_artifact_log::updated_at_ms,
                        ))
                        .load::<(String, String, Vec<u8>, i64)>(conn)?;
                    Ok((attempt, artifacts))
                })
                .unwrap();
            assert_eq!(after, before);
        }
    }
}

#[tokio::test]
async fn terminal_rollback_confirmation_rejects_missing_pending_landed_and_unknown_artifacts() {
    for case in ["missing", "pending", "landed", "unknown"] {
        let directory = tempdir().unwrap();
        let database = directory.path().join("replay-guard.sqlite");
        let executor = DieselSqliteExecutor::new(init_db_pool(database.to_str().unwrap()).unwrap());
        let subkey: Arc<dyn DeriveSpaceSubkeyPort> = Arc::new(FixedSubkey);
        let profile: Arc<dyn CurrentProfilePort> = Arc::new(FixedProfile);
        let attempts = DieselEntryReceiveAttemptRepository::new(executor.clone());
        let commit = DieselInboundReceiveCommitRepository::new(
            executor.clone(),
            subkey.clone(),
            profile.clone(),
        );
        attempts
            .begin_first_receive("entry", "a1", 1)
            .await
            .unwrap();
        attempts
            .begin_receive_failure("entry", "a1", 2)
            .await
            .unwrap();
        commit
            .commit_inbound_receive(&InboundReceiveSettlement::NoEntry {
                entry_id: EntryId::from("entry"),
                attempt_id: "a1".into(),
                terminal: PartialReceiveTerminal::Failed,
                artifacts: NoEntryReceiveArtifacts::None,
                now_ms: 3,
            })
            .await
            .unwrap();
        let output = directory.path().join("retained.bin");
        std::fs::write(&output, b"must retain").unwrap();
        if case != "missing" {
            let (phase, resolution) = match case {
                "pending" => (
                    ReceiveArtifactPhase::Preparing,
                    ReceiveArtifactResolution::Pending,
                ),
                "landed" => (
                    ReceiveArtifactPhase::Settled,
                    ReceiveArtifactResolution::Landed,
                ),
                _ => (
                    ReceiveArtifactPhase::Settled,
                    ReceiveArtifactResolution::RolledBack,
                ),
            };
            DieselReceiveArtifactLogRepository::new(executor.clone(), subkey, profile)
                .record_receive_artifacts(&ReceiveArtifactRecord {
                    entry_id: "entry".into(),
                    attempt_id: "a1".into(),
                    phase,
                    resolution,
                    artifacts: vec![ReceiveArtifact {
                        item_id: "item".into(),
                        staged_path: output.clone(),
                        final_path: output.clone(),
                        ownership: ReceiveArtifactOwnership::UserDestination,
                    }],
                    updated_at_ms: 4,
                })
                .await
                .unwrap();
            if case == "unknown" {
                executor
                    .run(|conn| {
                        diesel::update(receive_artifact_log::table)
                            .set(receive_artifact_log::phase.eq("future_phase"))
                            .execute(conn)?;
                        Ok(())
                    })
                    .unwrap();
            }
        }
        let before = executor
            .run(|conn| {
                receive_artifact_log::table
                    .select((
                        receive_artifact_log::phase,
                        receive_artifact_log::resolution,
                        receive_artifact_log::artifact_ciphertext,
                        receive_artifact_log::updated_at_ms,
                    ))
                    .load::<(String, String, Vec<u8>, i64)>(conn)
                    .map_err(Into::into)
            })
            .unwrap();
        executor.run(|conn| {
            diesel::sql_query("CREATE TRIGGER reject_terminal_rewrite BEFORE UPDATE ON entry_receive_attempt BEGIN SELECT RAISE(ABORT, 'terminal rewrite'); END").execute(conn)?;
            diesel::sql_query("CREATE TRIGGER reject_artifact_rewrite BEFORE UPDATE ON receive_artifact_log BEGIN SELECT RAISE(ABORT, 'artifact rewrite'); END").execute(conn)?;
            Ok(())
        }).unwrap();
        let error = commit
            .commit_inbound_receive(&InboundReceiveSettlement::NoEntry {
                entry_id: EntryId::from("entry"),
                attempt_id: "a1".into(),
                terminal: PartialReceiveTerminal::Failed,
                artifacts: NoEntryReceiveArtifacts::RolledBack,
                now_ms: 99,
            })
            .await
            .unwrap_err();
        match case {
            "missing" => assert!(matches!(
                error,
                InboundReceiveCommitError::ArtifactRecordMissing
            )),
            "unknown" => assert!(matches!(error, InboundReceiveCommitError::Backend(_))),
            _ => assert!(matches!(
                error,
                InboundReceiveCommitError::ArtifactStateMismatch
            )),
        }
        let after = executor
            .run(|conn| {
                receive_artifact_log::table
                    .select((
                        receive_artifact_log::phase,
                        receive_artifact_log::resolution,
                        receive_artifact_log::artifact_ciphertext,
                        receive_artifact_log::updated_at_ms,
                    ))
                    .load::<(String, String, Vec<u8>, i64)>(conn)
                    .map_err(Into::into)
            })
            .unwrap();
        assert_eq!(after, before);
        assert_eq!(std::fs::read(&output).unwrap(), b"must retain");
        let current = attempts.get_entry_attempt("entry").await.unwrap().unwrap();
        assert_eq!(current.state, uc_core::ports::AttemptState::Failed);
        assert_eq!(current.updated_at_ms, 3);
    }
}
