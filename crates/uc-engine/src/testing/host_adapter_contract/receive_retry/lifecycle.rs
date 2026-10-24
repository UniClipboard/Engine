//! 原生锁定与暂停必须等待同一接收任务的实际导出结算，重开不允许旧任务发布。
mod restart;
mod space;
use super::*;
use crate::{EngineState, UnlockSpaceInput};
use std::sync::atomic::Ordering;

async fn interrupted_retry(case: &ReceiveCase, attempt: &str) -> ReceiveProgressSummary {
    let current = timeout(Duration::from_secs(20), async {
        loop {
            if let OperationResult::EntryReceiveProgress(Some(current)) = case
                .receiver
                .execute(Operation::QueryEntryReceiveProgress(
                    EntryReceiveProgressInput {
                        entry_id: case.first.entry_id.clone(),
                    },
                ))
                .await
                .unwrap()
            {
                if matches!(current.state.as_str(), "failed" | "cancelled") {
                    break current;
                }
                assert!(
                    current.state != "completed",
                    "旧生命周期的未交付文件不得迟到成功"
                );
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("旧接收任务必须实际结算为中断");
    assert!(current.attempt_id == attempt, "重开不能替换或复活旧尝试");
    assert_eq!(current.items[0].state, "completed");
    assert_eq!(current.items[2].state, "completed");
    assert!(matches!(
        current.items[1].state.as_str(),
        "failed" | "cancelled"
    ));
    assert!(
        saved_files(
            &case.receiver,
            &case.first.entry_id,
            case.receiver_root.path()
        )
        .await
            == case.saved,
        "已经保存的成功文件与路径必须保留"
    );
    assert_eq!(
        case.exports.exports.lock().unwrap().len(),
        4,
        "中断恢复不得重新下载或导出旧项"
    );
    current
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_lock_waits_for_active_retry_export_and_preserves_saved_files() {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let case = receive_case(false).await;
    let _release = ReleaseOnDrop(case.exports.clone());
    let OperationResult::EntryReceiveRetry(EntryReceiveRetryOutcome::Accepted { attempt_id }) =
        case.receiver
            .execute(Operation::RetryEntryReceive(case.retry_input()))
            .await
            .unwrap()
    else {
        panic!("原部分失败必须允许完整选择重试")
    };
    timeout(Duration::from_secs(20), case.exports.paused.notified())
        .await
        .unwrap();
    let pending_output = case.exports.paths.lock().unwrap()[3].clone();
    assert!(pending_output.is_file());
    let lock_wait = timeout(
        Duration::from_millis(100),
        case.receiver.execute(Operation::LockEncryption),
    )
    .await;
    let kept_output_while_blocked = pending_output.is_file();
    assert!(
        lock_wait.is_err(),
        "未归还真实导出完成权之前不能报告已锁定或结束锁定动作"
    );
    // 等待既有业务门禁证明原锁定已封住接收；100ms调用超时本身不能证明该动作已经推进到封口。
    timeout(Duration::from_secs(20), async {
        loop {
            if matches!(case.receiver.execute(Operation::QueryReceiveReadiness).await.unwrap(),
                OperationResult::ReceiveReadiness(readiness) if !readiness.ready)
            {
                break;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("放行导出前原锁定必须实际封住接收准入");
    assert!(pending_output.is_file(), "导出尚未归还时不能提前清理输出");
    case.exports.release.add_permits(1);
    timeout(Duration::from_secs(20), async {
        loop {
            if matches!(case.receiver.execute(Operation::QueryEncryptionState).await.unwrap(),
                OperationResult::EncryptionState(state) if !state.session_ready)
            {
                break;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("调用者退出后原锁定动作仍须实际完成");
    assert!(matches!(
        timeout(
            Duration::from_secs(20),
            case.receiver
                .execute(Operation::UnlockSpace(UnlockSpaceInput {
                    passphrase: SecretString::new("real retry fixture"),
                }))
        )
        .await
        .expect("完整锁定完成后应允许解锁并恢复就绪")
        .unwrap(),
        OperationResult::SpaceUnlocked { .. }
    ));
    interrupted_retry(&case, &attempt_id).await;
    assert!(!pending_output.exists(), "锁定前未保存的本次输出必须清理");
    case.shutdown().await;
    assert!(lock_wait.is_err(), "未归还真实导出完成权之前不能报告已锁定");
    assert!(
        kept_output_while_blocked,
        "仍在实际导出中的输出不能提前被清理"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_suspend_deadline_keeps_export_supervised_until_resume() {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let case = receive_case(false).await;
    let _release = ReleaseOnDrop(case.exports.clone());
    let OperationResult::EntryReceiveRetry(EntryReceiveRetryOutcome::Accepted { attempt_id }) =
        case.receiver
            .execute(Operation::RetryEntryReceive(case.retry_input()))
            .await
            .unwrap()
    else {
        panic!("原部分失败必须允许完整选择重试")
    };
    timeout(Duration::from_secs(20), case.exports.paused.notified())
        .await
        .unwrap();
    let pending_output = case.exports.paths.lock().unwrap()[3].clone();
    let pause = case
        .receiver
        .suspend_with_deadline(Duration::from_millis(100))
        .await;
    let falsely_suspended = case.receiver.lifecycle_state().await == EngineState::Suspended;
    let kept_output_while_blocked = pending_output.is_file();
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
    interrupted_retry(&case, &attempt_id).await;
    assert!(!pending_output.exists(), "暂停前未保存的本次输出必须清理");
    case.shutdown().await;
    assert!(
        pause.is_err_and(|error| error.category() == EngineErrorCategory::DeadlineExceeded),
        "等待期限结束必须明确报告未完成"
    );
    assert!(!falsely_suspended, "导出仍未结算时不能发布Suspended");
    assert!(kept_output_while_blocked);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_revocation_during_retry_prevents_old_output_from_being_committed() {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let case = receive_case(false).await;
    let _release = ReleaseOnDrop(case.exports.clone());
    let OperationResult::EntryReceiveRetry(EntryReceiveRetryOutcome::Accepted { attempt_id }) =
        case.receiver
            .execute(Operation::RetryEntryReceive(case.retry_input()))
            .await
            .unwrap()
    else {
        panic!("原部分失败必须允许完整选择重试")
    };
    timeout(Duration::from_secs(20), case.exports.paused.notified())
        .await
        .unwrap();
    let pending_output = case.exports.paths.lock().unwrap()[3].clone();
    assert!(pending_output.is_file());
    // 只放弃等待，原成员移除仍由完整operation监督；不直接修改成员仓储。
    if let Ok(removed) = timeout(
        Duration::from_millis(100),
        case.receiver
            .execute(Operation::RemoveMember(RemoveMemberInput {
                device_id: case.source_id.clone(),
            })),
    )
    .await
    {
        removed.expect("真实成员移除必须接受");
    }
    timeout(Duration::from_secs(10), async {
        loop {
            if let OperationResult::DeviceGroupChoices(choices) = case
                .receiver
                .execute(Operation::QueryDeviceGroupChoices)
                .await
                .unwrap()
            {
                if choices.device_trust.local_membership == DeviceMembershipSummary::Active
                    && !choices.device_trust.devices.iter().any(|device| {
                        device.device_id == case.source_id
                            && device.membership == DeviceMembershipSummary::Active
                    })
                {
                    break;
                }
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("真实当前成员权威必须在恢复导出前确认撤销");
    assert!(
        pending_output.is_file(),
        "未归还的真实导出仍归原接收动作持有"
    );
    case.exports.release.add_permits(1);
    let current = interrupted_retry(&case, &attempt_id).await;
    assert!(!pending_output.exists(), "撤销前未保存的本次输出必须清理");
    assert_eq!(
        case.receiver
            .execute(Operation::RetryEntryReceive(RetryEntryReceiveInput {
                entry_id: current.entry_id,
                expected_attempt_id: current.attempt_id,
                item_ids: vec![current.items[1].item_id.clone()],
            }))
            .await
            .unwrap(),
        OperationResult::EntryReceiveRetry(EntryReceiveRetryOutcome::Unavailable {
            reason: ReceiveRetryUnavailableReason::AccessDenied,
        })
    );
    assert_eq!(case.exports.exports.lock().unwrap().len(), 4);
    case.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_offline_cancellation_ignores_permanently_blocked_progress() {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let case = receive_case(false).await;
    let _release = ReleaseOnDrop(case.exports.clone());
    case.source.shutdown_until_complete().await.unwrap();
    case.exports.block_reports.store(true, Ordering::SeqCst);
    let OperationResult::EntryReceiveRetry(EntryReceiveRetryOutcome::Accepted { attempt_id }) =
        case.receiver
            .execute(Operation::RetryEntryReceive(case.retry_input()))
            .await
            .unwrap()
    else {
        panic!("来源离线仍应允许使用原准确缓存重试")
    };
    timeout(Duration::from_secs(20), async {
        case.exports.paused.notified().await;
        case.exports.report_blocked.notified().await;
    })
    .await
    .expect("真实导出持完成权时，反向通知必须已经进入永久等待");
    let output = case.exports.paths.lock().unwrap()[3].clone();
    assert!(output.is_file());
    let cancellation =
        case.receiver
            .execute(Operation::CancelEntryReceive(CancelEntryReceiveInput {
                entry_id: case.first.entry_id.clone(),
                attempt_id: attempt_id.clone(),
            }));
    let settle_export = async {
        let cancelling = progress(&case.receiver, &case.first.entry_id, "cancelling").await;
        assert_eq!(cancelling.attempt_id, attempt_id);
        assert!(output.is_file(), "导出未归还完成权前仍归原流程持有");
        case.exports.release.add_permits(1);
    };
    let (cancelled, ()) =
        tokio::join!(timeout(Duration::from_secs(6), cancellation), settle_export);
    assert_eq!(
        cancelled.expect("本地取消不得等待反向通知").unwrap(),
        OperationResult::EntryReceiveCancellation(EntryReceiveCancellationOutcome::Cancelled)
    );
    let current = progress(&case.receiver, &case.first.entry_id, "cancelled").await;
    assert_eq!(current.attempt_id, attempt_id);
    assert_eq!(current.items[0].state, "completed");
    assert_eq!(current.items[1].state, "cancelled");
    assert_eq!(current.items[2].state, "completed");
    assert!(!output.exists(), "取消成功须包含本次真实输出清理");
    assert!(
        saved_files(
            &case.receiver,
            &case.first.entry_id,
            case.receiver_root.path()
        )
        .await
            == case.saved,
        "离线取消仍须保留原成功文件与路径"
    );
    assert_eq!(case.exports.exports.lock().unwrap().len(), 4);
    timeout(Duration::from_secs(20), case.shutdown())
        .await
        .expect("关闭不能等待解除永久通知屏障");
}
