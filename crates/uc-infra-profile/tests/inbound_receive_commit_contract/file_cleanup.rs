//! 已网络失败／取消的文件清理使用实际SQLite、AEAD与重开查询，不改写网络结果。
use super::*;
use diesel::result::Error as DieselError;
use std::error::Error as StdError;
use uc_core::ports::{
    GetEntryReceiveProgressPort, RequestReceiveCancellationPort, TrackedFileTransferStatus,
};

fn set_network(fixture: &Fixture, index: usize, status: TrackedFileTransferStatus) {
    let item = fixture.artifacts[index].item_id.clone();
    let status = status.as_str().to_owned();
    let code = if status == "cancelled" {
        "local_user"
    } else {
        "storage_unavailable"
    };
    fixture
        .executor
        .run(move |conn| {
            diesel::update(file_transfer::table.filter(file_transfer::receive_item_id.eq(item)))
                .set((
                    file_transfer::status.eq(status),
                    file_transfer::failure_code.eq(code),
                ))
                .execute(conn)?;
            Ok(())
        })
        .unwrap();
}

fn network_rows(fixture: &Fixture) -> Vec<(String, Option<String>, String, i64)> {
    fixture
        .executor
        .run(|conn| {
            Ok(file_transfer::table
                .order(file_transfer::transfer_id)
                .select((
                    file_transfer::status,
                    file_transfer::failure_code,
                    file_transfer::binding_state,
                    file_transfer::updated_at_ms,
                ))
                .load(conn)?)
        })
        .unwrap()
}

async fn clean(fixture: &Fixture, index: usize) -> Result<(), InboundReceiveCommitError> {
    fixture
        .commit
        .commit_inbound_receive(&InboundReceiveSettlement::FileCleaned {
            entry_id: EntryId::from("entry"),
            attempt_id: "a1".into(),
            artifact: fixture.artifacts[index].clone(),
            now_ms: 3,
        })
        .await
}

#[tokio::test]
async fn failed_and_cancelled_cleanup_keep_network_facts_and_saved_output_after_reopen() {
    for status in [
        TrackedFileTransferStatus::Failed,
        TrackedFileTransferStatus::Cancelled,
    ] {
        let fixture = Fixture::new().await;
        fixture
            .commit
            .commit_inbound_receive(&fixture.progress(0))
            .await
            .unwrap();
        set_network(&fixture, 1, status);
        let before = network_rows(&fixture);
        std::fs::remove_file(&fixture.artifacts[1].final_path).unwrap();
        clean(&fixture, 1).await.unwrap();
        assert_eq!(
            network_rows(&fixture),
            before,
            "清理不能改写网络状态、原因、绑定或网络时间"
        );
        assert!(fixture
            .journal
            .list_unsettled_receive_artifacts()
            .await
            .unwrap()[0]
            .artifacts
            .is_empty());
        assert!(!fixture
            .snapshot()
            .3
            .windows(b"private-output".len())
            .any(|part| part == b"private-output"));
        let terminal = if status == TrackedFileTransferStatus::Cancelled {
            fixture
                .attempts
                .request_receive_cancellation("entry", "a1", 4)
                .await
                .unwrap();
            PartialReceiveTerminal::Cancelled
        } else {
            fixture
                .attempts
                .begin_receive_failure("entry", "a1", 4)
                .await
                .unwrap();
            PartialReceiveTerminal::Failed
        };
        fixture
            .commit
            .commit_inbound_receive(&InboundReceiveSettlement::NoEntry {
                entry_id: EntryId::from("entry"),
                attempt_id: "a1".into(),
                terminal,
                artifacts: NoEntryReceiveArtifacts::RolledBack,
                now_ms: 5,
            })
            .await
            .unwrap();
        let reader = DieselFileTransferRepository::new(
            DieselSqliteExecutor::new(
                init_db_pool(
                    fixture
                        .directory
                        .path()
                        .join("progress.sqlite")
                        .to_str()
                        .unwrap(),
                )
                .unwrap(),
            ),
            Arc::new(FixedSubkey),
            Arc::new(FixedProfile),
        );
        let progress = reader
            .get_entry_receive_progress("entry")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(progress.items_completed, 1);
        assert_eq!(
            progress.items[0].status,
            TrackedFileTransferStatus::Completed
        );
        assert_eq!(progress.items[1].status, status);
        assert_eq!(
            std::fs::read(&fixture.artifacts[0].final_path).unwrap(),
            b"saved-content"
        );
        assert!(!fixture.artifacts[1].final_path.exists());
        assert_eq!(network_rows(&fixture), before);
    }
}

#[tokio::test]
async fn file_cleanup_refuses_nonfailed_network_results_without_mutating_ciphertext_or_rows() {
    for status in [
        TrackedFileTransferStatus::Pending,
        TrackedFileTransferStatus::Transferring,
        TrackedFileTransferStatus::Completed,
    ] {
        let fixture = Fixture::new().await;
        set_network(&fixture, 0, status);
        let before = (fixture.snapshot(), network_rows(&fixture));
        assert!(clean(&fixture, 0).await.is_err());
        assert_eq!((fixture.snapshot(), network_rows(&fixture)), before);
        assert!(fixture.artifacts[0].final_path.is_file());
    }
}

#[tokio::test]
async fn failed_file_cleanup_sql_failure_preserves_original_source_and_rolls_back_all_changes() {
    let fixture = Fixture::new().await;
    set_network(&fixture, 0, TrackedFileTransferStatus::Failed);
    std::fs::remove_file(&fixture.artifacts[0].final_path).unwrap();
    fixture.executor.run(|conn| {
        diesel::sql_query("CREATE TRIGGER reject_failed_file_cleanup BEFORE UPDATE OF artifact_ciphertext ON receive_artifact_log BEGIN SELECT RAISE(ABORT, 'injected cleanup ack failure'); END").execute(conn)?;
        Ok(())
    }).unwrap();
    let before = (fixture.snapshot(), network_rows(&fixture));
    let error = clean(&fixture, 0).await.unwrap_err();
    assert!(error.source().is_some());
    let mut current: Option<&(dyn StdError + 'static)> = Some(&error);
    let mut original_sql_error = false;
    while let Some(source) = current {
        if source.downcast_ref::<DieselError>().is_some() {
            original_sql_error = true;
            break;
        }
        current = source.source();
    }
    assert!(original_sql_error, "必须保留真实SQLite／Diesel来源");
    assert_eq!((fixture.snapshot(), network_rows(&fixture)), before);
    fixture
        .executor
        .run(|conn| {
            diesel::sql_query("DROP TRIGGER reject_failed_file_cleanup").execute(conn)?;
            Ok(())
        })
        .unwrap();
    clean(&fixture, 0).await.unwrap();
    assert_eq!(
        fixture
            .journal
            .list_unsettled_receive_artifacts()
            .await
            .unwrap()[0]
            .artifacts,
        vec![fixture.artifacts[1].clone()]
    );
    assert_eq!(network_rows(&fixture), before.1);
}

#[tokio::test]
async fn file_cleanup_refuses_saved_file_even_if_its_network_field_and_log_were_corrupted() {
    let fixture = Fixture::new().await;
    fixture
        .commit
        .commit_inbound_receive(&fixture.progress(0))
        .await
        .unwrap();
    // 模拟损坏的网络字段和过时的认证产物记录；交付绑定仍优先。
    set_network(&fixture, 0, TrackedFileTransferStatus::Failed);
    fixture
        .journal
        .record_receive_artifacts(&ReceiveArtifactRecord {
            entry_id: "entry".into(),
            attempt_id: "a1".into(),
            phase: ReceiveArtifactPhase::Preparing,
            resolution: ReceiveArtifactResolution::Pending,
            artifacts: fixture.artifacts.clone(),
            updated_at_ms: 3,
        })
        .await
        .unwrap();
    let before = (fixture.snapshot(), network_rows(&fixture));
    assert!(clean(&fixture, 0).await.is_err());
    assert_eq!((fixture.snapshot(), network_rows(&fixture)), before);
    assert_eq!(
        std::fs::read(&fixture.artifacts[0].final_path).unwrap(),
        b"saved-content"
    );
}

#[tokio::test]
async fn file_cleanup_refuses_superseded_attempt_and_unauthenticated_artifacts() {
    for corrupted in [false, true] {
        let fixture = Fixture::new().await;
        set_network(&fixture, 0, TrackedFileTransferStatus::Failed);
        fixture
            .executor
            .run(move |conn| {
                if corrupted {
                    diesel::update(receive_artifact_log::table)
                        .set(receive_artifact_log::artifact_ciphertext.eq(vec![9_u8; 32]))
                        .execute(conn)?;
                } else {
                    diesel::update(entry_receive_attempt::table)
                        .set(entry_receive_attempt::current_attempt_id.eq("a2"))
                        .execute(conn)?;
                }
                Ok(())
            })
            .unwrap();
        let before = (fixture.snapshot(), network_rows(&fixture));
        let error = clean(&fixture, 0).await.unwrap_err();
        if !corrupted {
            assert!(matches!(
                error,
                InboundReceiveCommitError::AttemptStateMismatch
            ));
        }
        assert_eq!((fixture.snapshot(), network_rows(&fixture)), before);
        assert!(fixture.artifacts[0].final_path.is_file());
    }
}

#[tokio::test]
async fn file_cleanup_refuses_unknown_or_duplicate_network_facts() {
    for duplicate in [false, true] {
        let fixture = Fixture::new().await;
        set_network(&fixture, 0, TrackedFileTransferStatus::Failed);
        let item = fixture.artifacts[0].item_id.clone();
        fixture
            .executor
            .run(move |conn| {
                if duplicate {
                    // 模拟损坏后的同一逻辑文件多条事实，不能只采用查询的第一条。
                    diesel::update(file_transfer::table)
                        .set((
                            file_transfer::receive_item_id.eq(item),
                            file_transfer::status.eq("failed"),
                        ))
                        .execute(conn)?;
                } else {
                    diesel::update(
                        file_transfer::table.filter(file_transfer::receive_item_id.eq(item)),
                    )
                    .set(file_transfer::status.eq("unknown_status"))
                    .execute(conn)?;
                }
                Ok(())
            })
            .unwrap();
        let before = (fixture.snapshot(), network_rows(&fixture));
        assert!(clean(&fixture, 0).await.is_err());
        assert_eq!((fixture.snapshot(), network_rows(&fixture)), before);
        assert!(fixture.artifacts[0].final_path.is_file());
    }
}

#[tokio::test]
async fn file_cleanup_can_settle_prepared_output_without_a_network_record() {
    let fixture = Fixture::new().await;
    let item = fixture.artifacts[0].item_id.clone();
    // 构造只完成目标和日志准备、网络登记尚未成功的状态。
    fixture
        .executor
        .run(move |conn| {
            diesel::delete(file_transfer::table.filter(file_transfer::receive_item_id.eq(item)))
                .execute(conn)?;
            Ok(())
        })
        .unwrap();
    std::fs::remove_file(&fixture.artifacts[0].final_path).unwrap();
    let before = network_rows(&fixture);
    clean(&fixture, 0).await.unwrap();
    assert_eq!(network_rows(&fixture), before, "不补造网络文件或终态");
    assert_eq!(
        fixture
            .journal
            .list_unsettled_receive_artifacts()
            .await
            .unwrap()[0]
            .artifacts,
        vec![fixture.artifacts[1].clone()]
    );
    assert!(!fixture.artifacts[0].final_path.exists());
}
