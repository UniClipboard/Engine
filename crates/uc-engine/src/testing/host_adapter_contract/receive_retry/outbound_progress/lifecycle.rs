//! 暂停和锁定只退休没有持久依据的进度；真实保存成果不随会话消失。
use super::super::lifecycle::interrupted_retry;
use super::*;
use crate::{EngineState, OutboundProgressSummary, RecoverSessionInput, UnlockSpaceInput};

async fn attempt_view(case: &ReceiveCase, attempt: &str) -> OutboundProgressSummary {
    timeout(Duration::from_secs(20), async {
        loop {
            let OperationResult::OutboundProgressList(views) = case
                .source
                .execute(Operation::ListOutboundProgress)
                .await
                .unwrap()
            else {
                panic!("outbound list")
            };
            if let Some(view) = views
                .into_iter()
                .find(|view| view.attempt_id.as_deref() == Some(attempt))
            {
                return view;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_sender_resume_and_profile_reopen_retire_unpersisted_progress_without_redownload(
) {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    for reopen in [false, true] {
        let mut case = receive_case(false).await;
        let _release = ReleaseOnDrop(case.exports.clone());
        let view = timeout(Duration::from_secs(20), async {
            loop {
                let OperationResult::OutboundProgressList(views) = case
                    .source
                    .execute(Operation::ListOutboundProgress)
                    .await
                    .unwrap()
                else {
                    panic!("complete outbound views")
                };
                if let Some(view) = views.into_iter().find(|view| {
                    view.attempt_id.as_deref() == Some(case.first.attempt_id.as_str())
                        && view.state == OutboundProgressBatchState::PartiallyFailed
                }) {
                    return view;
                }
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        let exported = case.exports.exports.lock().unwrap().len();
        if reopen {
            case.source.shutdown_until_complete().await.unwrap();
            let config = EngineConfig::new("1.2.3")
                .with_rendezvous_base_url(case._server.uri())
                .with_test_relay_fallback(false);
            let (source, events) = Engine::start(
                config,
                persistent_engine_host(case._source_root.path(), case.source_storage.clone()),
            )
            .await
            .unwrap();
            case.source = source;
            case.source_events = events;
            case.source
                .execute(Operation::RecoverSession(RecoverSessionInput {
                    allow_secure_storage_unlock: true,
                }))
                .await
                .unwrap();
        } else {
            case.source.suspend().await.unwrap();
            case.source.resume().await.unwrap();
        }
        let OperationResult::EncryptionState(encryption) = case
            .source
            .execute(Operation::QueryEncryptionState)
            .await
            .unwrap()
        else {
            panic!("resumed encryption state")
        };
        if !encryption.session_ready {
            case.source
                .execute(Operation::UnlockSpace(crate::UnlockSpaceInput {
                    passphrase: SecretString::new("real retry fixture"),
                }))
                .await
                .unwrap();
        }
        assert_eq!(member_id(&case.source).await, case.source_id);
        let listed = case
            .source
            .execute(Operation::ListOutboundProgress)
            .await
            .unwrap();
        let queried = case
            .source
            .execute(Operation::QueryOutboundProgress(OutboundProgressInput {
                entry_id: view.entry_id.clone(),
                target_device_id: view.target_device_id.clone(),
            }))
            .await
            .unwrap();
        let resent = case
            .source
            .execute(Operation::ResendEntry(crate::ResendEntryInput {
                entry_id: view.entry_id,
                target_devices: vec![view.target_device_id],
                confirm_new_transfer: false,
            }))
            .await
            .unwrap();
        let files = saved_files(
            &case.receiver,
            &case.first.entry_id,
            case.receiver_root.path(),
        )
        .await;
        let expected = case.saved.clone();
        let after = case.exports.exports.lock().unwrap().len();
        case.shutdown().await;
        let OperationResult::OutboundProgress(Some(unknown)) = queried else {
            panic!("existing ownership stays unconfirmed")
        };
        assert_eq!(unknown.attempt_id, None);
        assert_eq!(unknown.freshness, OutboundProgressFreshness::Unconfirmed);
        assert_eq!((unknown.items_total, unknown.items_unconfirmed), (3, 3));
        assert_eq!(
            listed,
            OperationResult::OutboundProgressList(vec![unknown.clone()])
        );
        let OperationResult::EntryResent(crate::ResendEntryOutcome::Completed(report)) = resent
        else {
            panic!("complete resumed resend")
        };
        assert_eq!(report.accepted + report.duplicate + report.pending, 0);
        assert_eq!(
            report.reconciled[0].state,
            OutboundProgressReconciliation::NotRetained
        );
        assert_eq!(report.reconciled[0].current.as_ref(), Some(&unknown));
        assert!(report.reconciled[0].duplicate_files_possible);
        assert!(!report.reconciled[0].new_transfer_requested);
        assert_eq!(after, exported);
        assert_eq!(files, expected);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_receiver_session_retirement_does_not_leave_unretained_retry_running_on_sender()
{
    let _lock = ENGINE_TEST_LOCK.lock().await;
    for suspend in [false, true] {
        let case = receive_case(false).await;
        let _release = ReleaseOnDrop(case.exports.clone());
        let original = attempt_view(&case, &case.first.attempt_id).await;
        let OperationResult::EntryReceiveRetry(EntryReceiveRetryOutcome::Accepted { attempt_id }) =
            case.receiver
                .execute(Operation::RetryEntryReceive(case.retry_input()))
                .await
                .unwrap()
        else {
            panic!("real retry accepted")
        };
        timeout(Duration::from_secs(20), case.exports.paused.notified())
            .await
            .unwrap();
        let _ = case
            .source
            .execute(Operation::ReconcileOutboundProgress(
                ReconcileOutboundProgressInput {
                    entry_id: original.entry_id.clone(),
                    target_device_id: original.target_device_id.clone(),
                    expected_attempt_id: None,
                },
            ))
            .await
            .unwrap();
        // 新尝试登记先于实际活动通知；等待完整视图，不能把登记先到误当成状态丢失。
        let active = timeout(Duration::from_secs(20), async {
            loop {
                let view = attempt_view(&case, &attempt_id).await;
                if view.items_completed == 2 && view.items_running == 1 {
                    break view;
                }
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("actual active retry reaches original sender");
        let pending_output = case.exports.paths.lock().unwrap()[3].clone();
        assert!(pending_output.is_file());
        if suspend {
            let paused = case
                .receiver
                .suspend_with_deadline(Duration::from_millis(100))
                .await;
            assert!(paused
                .is_err_and(|error| error.category() == EngineErrorCategory::DeadlineExceeded));
            assert_ne!(
                case.receiver.lifecycle_state().await,
                EngineState::Suspended
            );
            assert!(pending_output.is_file());
            case.exports.release.add_permits(1);
            timeout(Duration::from_secs(20), case.receiver.suspend())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                case.receiver.lifecycle_state().await,
                EngineState::Suspended
            );
            case.receiver.resume().await.unwrap();
        } else {
            assert!(timeout(
                Duration::from_millis(100),
                case.receiver.execute(Operation::LockEncryption)
            )
            .await
            .is_err());
            timeout(Duration::from_secs(20), async {
                loop {
                    let readiness = case.receiver.execute(Operation::QueryReceiveReadiness).await.unwrap();
                    if matches!(readiness, OperationResult::ReceiveReadiness(readiness) if !readiness.ready) {
                        break;
                    }
                    sleep(Duration::from_millis(20)).await;
                }
            }).await.unwrap();
            assert!(pending_output.is_file());
            case.exports.release.add_permits(1);
            timeout(Duration::from_secs(20), async {
                loop {
                    let state = case.receiver.execute(Operation::QueryEncryptionState).await.unwrap();
                    if matches!(state, OperationResult::EncryptionState(state) if !state.session_ready) {
                        break;
                    }
                    sleep(Duration::from_millis(20)).await;
                }
            }).await.unwrap();
        }
        let OperationResult::EncryptionState(encryption) = case
            .receiver
            .execute(Operation::QueryEncryptionState)
            .await
            .unwrap()
        else {
            panic!("resumed encryption state")
        };
        if !encryption.session_ready {
            case.receiver
                .execute(Operation::UnlockSpace(UnlockSpaceInput {
                    passphrase: SecretString::new("real retry fixture"),
                }))
                .await
                .unwrap();
        }
        let current = interrupted_retry(&case, &attempt_id).await;
        assert_eq!(current.attempt_id, attempt_id);
        assert!(!pending_output.exists());
        let before_query = attempt_view(&case, &attempt_id).await;
        assert_eq!(before_query.items_completed, 2);
        let OperationResult::EntryResent(crate::ResendEntryOutcome::Completed(report)) = case
            .source
            .execute(Operation::ResendEntry(crate::ResendEntryInput {
                entry_id: active.entry_id,
                target_devices: vec![active.target_device_id],
                confirm_new_transfer: false,
            }))
            .await
            .unwrap()
        else {
            panic!("complete original resend")
        };
        let files = saved_files(
            &case.receiver,
            &case.first.entry_id,
            case.receiver_root.path(),
        )
        .await;
        let expected = case.saved.clone();
        let exports = case.exports.exports.lock().unwrap().len();
        case.shutdown().await;
        assert_eq!(report.accepted + report.duplicate + report.pending, 0);
        assert_eq!(
            report.reconciled[0].state,
            if suspend {
                OutboundProgressReconciliation::Settled
            } else {
                OutboundProgressReconciliation::NotRetained
            }
        );
        let view = report.reconciled[0]
            .current
            .as_ref()
            .expect("original source still retains its view");
        assert_eq!(view.items_completed, 2);
        if suspend {
            assert_eq!(view.freshness, OutboundProgressFreshness::Settled);
            assert_eq!(view.attempt_id.as_deref(), Some(attempt_id.as_str()));
            assert_eq!(view.items_running + view.items_unconfirmed, 0);
            assert_eq!(
                view.items[1].state,
                match current.items[1].state.as_str() {
                    "cancelled" => OutboundProgressState::Cancelled,
                    "failed" => OutboundProgressState::Failed,
                    _ => panic!("actual interrupted receiver result"),
                }
            );
        } else {
            match before_query.items[1].state {
                OutboundProgressState::Cancelled => {
                    assert_eq!(current.items[1].state, "cancelled");
                    assert_eq!(view.items[1], before_query.items[1]);
                }
                OutboundProgressState::Failed => {
                    assert_eq!(current.items[1].state, "failed");
                    assert_eq!(view.items[1], before_query.items[1]);
                }
                _ => {
                    assert_eq!(view.items_cancelled, 0);
                    assert_eq!(
                        view.items_running, 0,
                        "explicitly unretained task must not continue a live download animation"
                    );
                    assert_eq!(view.items_unconfirmed, 1);
                }
            }
        }
        assert_eq!(files, expected);
        assert_eq!(exports, 4);
    }
}
