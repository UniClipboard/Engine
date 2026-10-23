//! 使用真实Space重置与本机移除确认，验证旧接收不会迟到交付。
use super::*;
use crate::{ChooseDeviceGroupInput, DeviceGroupChoiceOutcomeSummary};

async fn paused_retry(case: &ReceiveCase) -> (String, PathBuf) {
    let OperationResult::EntryReceiveRetry(EntryReceiveRetryOutcome::Accepted { attempt_id }) =
        case.receiver
            .execute(Operation::RetryEntryReceive(case.retry_input()))
            .await
            .unwrap()
    else {
        panic!("原部分失败必须接受单项重试")
    };
    timeout(Duration::from_secs(20), case.exports.paused.notified())
        .await
        .unwrap();
    let output = case.exports.paths.lock().unwrap()[3].clone();
    assert!(output.is_file());
    (attempt_id, output)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_space_reset_seals_active_retry_before_switching_identity() {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let case = receive_case(false).await;
    let _release = ReleaseOnDrop(case.exports.clone());
    let OperationResult::SetupState(before) = case
        .receiver
        .execute(Operation::QuerySetupState)
        .await
        .unwrap()
    else {
        panic!("原Space必须存在")
    };
    let (attempt, output) = paused_retry(&case).await;
    {
        let resetting = case.receiver.execute(Operation::ResetSpace);
        tokio::pin!(resetting);
        assert!(
            timeout(Duration::from_millis(100), &mut resetting)
                .await
                .is_err(),
            "实际导出未结算前不能报告切Space完成"
        );
        timeout(Duration::from_secs(10), async {
            loop {
                match case
                    .receiver
                    .execute(Operation::QueryReceiveReadiness)
                    .await
                {
                    Ok(OperationResult::ReceiveReadiness(readiness)) if !readiness.ready => break,
                    // 整体会话先封住操作准入；交接期间没有可读facade，稳定错误同样证明旧会话已封口。
                    Err(error)
                        if error.code() == 1103
                            && error.category() == EngineErrorCategory::Unavailable =>
                    {
                        break
                    }
                    Ok(OperationResult::ReceiveReadiness(_)) => {}
                    _ => panic!("切Space必须提供既有readiness或会话封口合同"),
                }
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("切Space必须先封住旧接收准入");
        assert!(output.is_file(), "未归还的输出仍由原接收监督持有");
        case.exports.release.add_permits(1);
        assert_eq!(
            timeout(Duration::from_secs(20), &mut resetting)
                .await
                .unwrap()
                .unwrap(),
            OperationResult::SpaceReset
        );
    }
    let OperationResult::SetupState(after) = case
        .receiver
        .execute(Operation::QuerySetupState)
        .await
        .unwrap()
    else {
        panic!("新Space必须存在")
    };
    assert!(
        after.has_completed && after.space_id.is_some() && after.space_id != before.space_id,
        "必须实际切换到新Space"
    );
    interrupted_retry(&case, &attempt).await;
    assert!(!output.exists(), "旧Space未保存输出必须实际清理");
    let OperationResult::Devices(devices) =
        case.receiver.execute(Operation::ListDevices).await.unwrap()
    else {
        panic!("新Space必须能读取既有设备列表")
    };
    assert_eq!(devices.len(), 1);
    assert!(devices[0].is_local);
    case.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_confirmed_local_removal_prevents_active_retry_from_publishing() {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let case = receive_case(false).await;
    let _release = ReleaseOnDrop(case.exports.clone());
    let receiver_id = member_id(&case.receiver).await;
    let (attempt, output) = paused_retry(&case).await;
    case.source
        .execute(Operation::RemoveMember(RemoveMemberInput {
            device_id: receiver_id,
        }))
        .await
        .unwrap();
    timeout(Duration::from_secs(20), async {
        loop {
            let OperationResult::DeviceGroupChoices(choices) = case.receiver.execute(Operation::QueryDeviceGroupChoices).await.unwrap()
            else { panic!("本机移除必须提供既有成员视图") };
            if choices.device_trust.local_membership == DeviceMembershipSummary::Removed { break; }
            if let Some(issue) = choices.issues.iter().find(|issue| issue.issue_id.starts_with("p:")) {
                let choice = case.receiver.execute(Operation::ChooseDeviceGroup(ChooseDeviceGroupInput {
                    issue_id: issue.issue_id.clone(), choice_id: "apply".into(),
                    expected_revision: choices.revision, confirm_local_removal: true,
                }));
                match timeout(Duration::from_millis(100), choice).await {
                    Ok(Ok(OperationResult::DeviceGroupChosen(result))) => {
                        assert!(matches!(result.outcome, DeviceGroupChoiceOutcomeSummary::Completed
                            | DeviceGroupChoiceOutcomeSummary::AlreadyCompleted
                            | DeviceGroupChoiceOutcomeSummary::StateChanged));
                    }
                    Ok(_) => panic!("本机移除确认必须沿完整成员操作结算"),
                    // 等待方离开不取消原操作；之后只查询已存在的稳定准入／成员事实。
                    Err(_) => {
                        loop {
                            if matches!(case.receiver.execute(Operation::QueryReceiveReadiness).await.unwrap(),
                                OperationResult::ReceiveReadiness(readiness) if !readiness.ready) { break; }
                            if matches!(case.receiver.execute(Operation::QueryDeviceGroupChoices).await.unwrap(),
                                OperationResult::DeviceGroupChoices(current) if current.device_trust.local_membership == DeviceMembershipSummary::Removed) { break; }
                            sleep(Duration::from_millis(20)).await;
                        }
                        break;
                    }
                }
            }
            sleep(Duration::from_millis(20)).await;
        }
    }).await.expect("本机离组必须实际撤销当前授权或封住接收准入");
    assert!(output.is_file(), "导出完成权尚未归还，不能丢弃实际输出责任");
    case.exports.release.add_permits(1);
    interrupted_retry(&case, &attempt).await;
    assert!(!output.exists(), "本机离组前未保存的输出必须清理");
    timeout(Duration::from_secs(20), async {
        loop {
            if matches!(case.receiver.execute(Operation::QueryDeviceGroupChoices).await.unwrap(),
                OperationResult::DeviceGroupChoices(current) if current.device_trust.local_membership == DeviceMembershipSummary::Removed) { break; }
            sleep(Duration::from_millis(20)).await;
        }
    }).await.expect("本机离组确认必须最终达到Removed");
    case.shutdown().await;
}
