//! 稳定发送侧操作读取真实接收、SQLite结算和文件成果；核对不能启动新下载。
use super::*;
use crate::{
    OutboundProgressBatchState, OutboundProgressFreshness, OutboundProgressInput,
    OutboundProgressReconciliation, OutboundProgressState, ReconcileOutboundProgressInput,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_outbound_progress_reports_actual_partial_result_and_reconciles_without_download(
) {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let mut case = receive_case(false).await;
    let _release = ReleaseOnDrop(case.exports.clone());
    next_engine_event_matching(&mut case.source_events, |event| {
        matches!(event, EngineEvent::OutboundProgressChanged)
    })
    .await;
    let view = timeout(Duration::from_secs(20), async {
        loop {
            let OperationResult::OutboundProgressList(views) = case
                .source
                .execute(Operation::ListOutboundProgress)
                .await
                .unwrap()
            else {
                panic!("complete outbound list")
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
    .expect("actual receiver settlement reaches original sender");
    assert_ne!(
        view.entry_id, case.first.entry_id,
        "source and local received entries have independent identities"
    );
    assert_eq!(view.freshness, OutboundProgressFreshness::Settled);
    assert_eq!(
        (
            view.items_total,
            view.items_completed,
            view.items_failed,
            view.items_unconfirmed
        ),
        (3, 2, 1, 0)
    );
    assert_eq!(
        view.items.iter().map(|item| item.state).collect::<Vec<_>>(),
        [
            OutboundProgressState::Completed,
            OutboundProgressState::Failed,
            OutboundProgressState::Completed
        ]
    );
    assert_eq!(
        view.items[1].failure_reason,
        Some(ReceiveItemFailureReason::StorageUnavailable)
    );
    let OperationResult::OutboundProgress(Some(queried)) = case
        .source
        .execute(Operation::QueryOutboundProgress(OutboundProgressInput {
            entry_id: view.entry_id.clone(),
            target_device_id: view.target_device_id.clone(),
        }))
        .await
        .unwrap()
    else {
        panic!("same retained complete view")
    };
    assert_eq!(queried.items, view.items);
    let before = case.exports.exports.lock().unwrap().len();
    let result = case
        .source
        .execute(Operation::ReconcileOutboundProgress(
            ReconcileOutboundProgressInput {
                entry_id: view.entry_id.clone(),
                target_device_id: view.target_device_id.clone(),
                expected_attempt_id: view.attempt_id.clone(),
            },
        ))
        .await
        .unwrap();
    assert_eq!(
        result,
        OperationResult::OutboundProgressReconciliation(OutboundProgressReconciliation::Settled)
    );
    assert_eq!(
        case.exports.exports.lock().unwrap().len(),
        before,
        "query must not download again"
    );
    let result = case
        .source
        .execute(Operation::ReconcileOutboundProgress(
            ReconcileOutboundProgressInput {
                entry_id: view.entry_id,
                target_device_id: view.target_device_id,
                expected_attempt_id: Some("00000000-0000-0000-0000-000000000001".into()),
            },
        ))
        .await
        .unwrap();
    assert_eq!(
        result,
        OperationResult::OutboundProgressReconciliation(OutboundProgressReconciliation::Superseded)
    );
    assert_eq!(case.exports.exports.lock().unwrap().len(), before);
    assert_eq!(
        saved_files(
            &case.receiver,
            &case.first.entry_id,
            case.receiver_root.path()
        )
        .await,
        case.saved
    );
    case.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_deleting_source_retires_outbound_view_without_touching_received_files() {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    for paused in [false, true] {
        let case = receive_case(paused).await;
        let _release = ReleaseOnDrop(case.exports.clone());
        let view = timeout(Duration::from_secs(20), async {
            loop {
                let OperationResult::OutboundProgressList(views) = case
                    .source
                    .execute(Operation::ListOutboundProgress)
                    .await
                    .unwrap()
                else {
                    panic!("complete outbound list")
                };
                if let Some(view) = views.into_iter().find(|view| {
                    view.attempt_id.as_deref() == Some(case.first.attempt_id.as_str())
                        && (paused || view.state == OutboundProgressBatchState::PartiallyFailed)
                }) {
                    return view;
                }
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("actual receiver result");
        case.source
            .execute(Operation::DeleteHistoryEntry(HistoryEntryInput {
                entry_id: view.entry_id.clone(),
            }))
            .await
            .unwrap();
        if paused {
            // 完整本地输出随后归还，真实迟到终态仍会到达原发送端。
            case.exports.release.add_permits(1);
            progress(&case.receiver, &case.first.entry_id, "failed").await;
        }
        let queried = case
            .source
            .execute(Operation::QueryOutboundProgress(OutboundProgressInput {
                entry_id: view.entry_id.clone(),
                target_device_id: view.target_device_id,
            }))
            .await
            .unwrap();
        let listed = case
            .source
            .execute(Operation::ListOutboundProgress)
            .await
            .unwrap();
        let saved = saved_files(
            &case.receiver,
            &case.first.entry_id,
            case.receiver_root.path(),
        )
        .await;
        let original = case.saved.clone();
        case.shutdown().await;
        assert_eq!(
            queried,
            OperationResult::OutboundProgress(None),
            "deleted source cannot be recreated by late results"
        );
        let OperationResult::OutboundProgressList(listed) = listed else {
            panic!("complete list")
        };
        assert!(!listed
            .iter()
            .any(|current| current.entry_id == view.entry_id));
        assert_eq!(saved.len(), 2);
        assert!(saved.contains_key(CONTENTS[0]) && saved.contains_key(CONTENTS[2]));
        if !paused {
            assert_eq!(
                saved, original,
                "source deletion cannot remove receiver-owned files"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_membership_removal_and_space_reset_retire_outbound_progress_and_keep_saved_files(
) {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    for reset_space in [false, true] {
        let case = receive_case(false).await;
        let _release = ReleaseOnDrop(case.exports.clone());
        let view = timeout(Duration::from_secs(20), async {
            loop {
                let OperationResult::OutboundProgressList(views) = case
                    .source
                    .execute(Operation::ListOutboundProgress)
                    .await
                    .unwrap()
                else {
                    panic!("complete outbound list")
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
        .expect("actual receiver result before membership action");
        if reset_space {
            case.source.execute(Operation::ResetSpace).await.unwrap();
        } else {
            case.source
                .execute(Operation::RemoveMember(RemoveMemberInput {
                    device_id: member_id(&case.receiver).await,
                }))
                .await
                .unwrap();
        }
        let listed = case
            .source
            .execute(Operation::ListOutboundProgress)
            .await
            .unwrap();
        let queried = case
            .source
            .execute(Operation::QueryOutboundProgress(OutboundProgressInput {
                entry_id: view.entry_id,
                target_device_id: view.target_device_id,
            }))
            .await
            .unwrap_err();
        let files = saved_files(
            &case.receiver,
            &case.first.entry_id,
            case.receiver_root.path(),
        )
        .await;
        let original = case.saved.clone();
        case.shutdown().await;
        assert_eq!(listed, OperationResult::OutboundProgressList(vec![]));
        assert_eq!(queried.category(), EngineErrorCategory::Unauthorized);
        assert_eq!(
            files, original,
            "retiring sender association does not revoke receiver-owned output"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_resend_reconciles_original_task_and_only_explicit_transfer_retries_failed_item(
) {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    for paused in [true, false] {
        let case = receive_case(paused).await;
        let _release = ReleaseOnDrop(case.exports.clone());
        let view = timeout(Duration::from_secs(20), async {
            loop {
                let OperationResult::OutboundProgressList(views) = case
                    .source
                    .execute(Operation::ListOutboundProgress)
                    .await
                    .unwrap()
                else {
                    panic!("outbound views")
                };
                if let Some(view) = views.into_iter().find(|view| {
                    view.attempt_id.as_deref() == Some(case.first.attempt_id.as_str())
                        && (paused || view.state == OutboundProgressBatchState::PartiallyFailed)
                }) {
                    return view;
                }
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        let before = case.exports.exports.lock().unwrap().len();
        let resend = |confirm| {
            case.source
                .execute(Operation::ResendEntry(crate::ResendEntryInput {
                    entry_id: view.entry_id.clone(),
                    target_devices: vec![view.target_device_id.clone()],
                    confirm_new_transfer: confirm,
                }))
        };
        if paused {
            for confirm in [false, true] {
                let OperationResult::EntryResent(crate::ResendEntryOutcome::Completed(report)) =
                    resend(confirm).await.unwrap()
                else {
                    panic!("complete original action")
                };
                assert_eq!(report.accepted + report.duplicate + report.pending, 0);
                assert_eq!(
                    report.reconciled[0].state,
                    OutboundProgressReconciliation::Running
                );
                assert!(!report.reconciled[0].new_transfer_requested);
                assert_eq!(case.exports.exports.lock().unwrap().len(), before);
            }
            case.exports.release.add_permits(1);
            progress(&case.receiver, &case.first.entry_id, "failed").await;
        }
        let settled_exports = case.exports.exports.lock().unwrap().len();
        let OperationResult::EntryResent(crate::ResendEntryOutcome::Completed(report)) =
            resend(false).await.unwrap()
        else {
            panic!("settled original action")
        };
        assert_eq!(report.accepted + report.duplicate + report.pending, 0);
        assert_eq!(report.reconciled.len(), 1);
        assert_eq!(
            report.reconciled[0].state,
            OutboundProgressReconciliation::Settled
        );
        assert_eq!(
            report.reconciled[0]
                .current
                .as_ref()
                .unwrap()
                .items_completed,
            2
        );
        assert_eq!(
            report.reconciled[0].current.as_ref().unwrap().items_failed,
            1
        );
        assert!(!report.reconciled[0].new_transfer_requested);
        assert_eq!(case.exports.exports.lock().unwrap().len(), settled_exports);
        if !paused {
            let releasing = async {
                timeout(Duration::from_secs(20), case.exports.paused.notified())
                    .await
                    .unwrap();
                assert_eq!(
                    case.exports.exports.lock().unwrap().len(),
                    settled_exports + 1
                );
                assert_eq!(
                    saved_files(
                        &case.receiver,
                        &case.first.entry_id,
                        case.receiver_root.path()
                    )
                    .await,
                    case.saved
                );
                case.exports.release.add_permits(1);
            };
            let (result, ()) = tokio::join!(resend(true), releasing);
            let OperationResult::EntryResent(crate::ResendEntryOutcome::Completed(report)) =
                result.unwrap()
            else {
                panic!("explicit original redelivery")
            };
            assert!(report.reconciled[0].new_transfer_requested);
            assert_eq!(report.accepted + report.pending, 1);
            let complete = progress(&case.receiver, &case.first.entry_id, "completed").await;
            assert_ne!(complete.attempt_id, case.first.attempt_id);
            let files = saved_files(
                &case.receiver,
                &case.first.entry_id,
                case.receiver_root.path(),
            )
            .await;
            assert_eq!(files.len(), 3);
            for (bytes, path) in &case.saved {
                assert_eq!(files.get(bytes), Some(path));
            }
            assert_eq!(
                case.exports.exports.lock().unwrap().len(),
                settled_exports + 1
            );
        }
        case.shutdown().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_resend_does_not_redownload_when_receiver_is_offline_even_with_confirmation() {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let case = receive_case(false).await;
    let _release = ReleaseOnDrop(case.exports.clone());
    let view = timeout(Duration::from_secs(20), async {
        loop {
            let OperationResult::OutboundProgressList(views) = case
                .source
                .execute(Operation::ListOutboundProgress)
                .await
                .unwrap()
            else {
                panic!("outbound list")
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
    let original = case.saved.clone();
    let exports = case.exports.exports.lock().unwrap().len();
    case.receiver.shutdown_until_complete().await.unwrap();
    let result = case
        .source
        .execute(Operation::ResendEntry(crate::ResendEntryInput {
            entry_id: view.entry_id,
            target_devices: vec![view.target_device_id],
            confirm_new_transfer: true,
        }))
        .await;
    match result {
        Ok(OperationResult::EntryResent(crate::ResendEntryOutcome::Completed(report))) => {
            assert_eq!(report.accepted + report.duplicate + report.pending, 0);
            assert_eq!(
                report.reconciled[0].state,
                OutboundProgressReconciliation::Unreachable
            );
            assert!(!report.reconciled[0].new_transfer_requested);
        }
        Err(error) => assert_eq!(
            error.code(),
            crate::error_codes::OUTBOUND_PROGRESS_UNAVAILABLE_CODE
        ),
        other => panic!("offline must remain unverified: {other:?}"),
    }
    assert_eq!(case.exports.exports.lock().unwrap().len(), exports);
    for (bytes, path) in original {
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
    case.shutdown().await;
}
