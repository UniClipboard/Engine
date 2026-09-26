//! 成员历史交换的可重试失败与稳定拒绝。

use super::*;

const SPACE_DEVICE_UPDATE_WAIT_TIMEOUT: Duration = Duration::from_secs(30);

async fn arm_membership_history_failures(
    engine: &Engine,
    failure: uc_engine::DevMembershipHistoryFailure,
    count: usize,
) -> u64 {
    let result = engine
        .execute_dev(uc_engine::DevOperation::ArmMembershipHistoryFailures { failure, count })
        .await
        .expect("arm membership history failures");
    let uc_engine::DevOperationResult::MembershipHistoryFailuresArmed { after_sequence } = result
    else {
        panic!("unexpected membership history failure arm result");
    };
    after_sequence
}

async fn clear_membership_history_failures(engine: &Engine) -> usize {
    let result = engine
        .execute_dev(uc_engine::DevOperation::ClearMembershipHistoryFailures)
        .await
        .expect("clear membership history failures");
    let uc_engine::DevOperationResult::MembershipHistoryFailuresCleared { remaining } = result
    else {
        panic!("unexpected membership history failure clear result");
    };
    remaining
}

async fn wait_for_maintenance_health(
    engine: &Engine,
    phase: uc_engine::MembershipMaintenanceHealthPhaseSummary,
) -> uc_engine::MembershipMaintenanceHealthSummary {
    let deadline = tokio::time::Instant::now() + ADMISSION_WAIT_TIMEOUT;
    loop {
        if let Ok(OperationResult::DeviceGroupChoices(summary)) =
            engine.execute(Operation::QueryDeviceGroupChoices).await
        {
            if summary.device_trust.maintenance_health.phase == phase {
                return summary.device_trust.maintenance_health;
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "membership maintenance health did not reach {phase:?}"
        );
        tokio::task::yield_now().await;
    }
}

/// 等待维护健康状态进入带持久重试时间的 Retrying。
async fn wait_for_maintenance_retry_deadline(
    engine: &Engine,
) -> uc_engine::MembershipMaintenanceHealthSummary {
    let deadline = tokio::time::Instant::now() + ADMISSION_WAIT_TIMEOUT;
    loop {
        if let Ok(OperationResult::DeviceGroupChoices(summary)) =
            engine.execute(Operation::QueryDeviceGroupChoices).await
        {
            let health = summary.device_trust.maintenance_health;
            if health.phase == uc_engine::MembershipMaintenanceHealthPhaseSummary::Retrying
                && health.next_retry_at_ms.is_some()
            {
                return health;
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "membership maintenance health did not expose a retry deadline"
        );
        tokio::task::yield_now().await;
    }
}

async fn wait_for_space_device_update(
    engine: &Engine,
    phase: uc_engine::SpaceDeviceUpdatePhaseSummary,
) -> uc_engine::SpaceDeviceUpdateStatusSummary {
    let deadline = tokio::time::Instant::now() + SPACE_DEVICE_UPDATE_WAIT_TIMEOUT;
    loop {
        let status = query_space_device_update(engine).await;
        if status.phase == phase {
            return status;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "space device update did not reach {phase:?}: {status:?}"
        );
        tokio::task::yield_now().await;
    }
}

// 普通成员历史交换暂时失败时，公开状态必须带有持久重试时间，并在到期后自动恢复。
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn membership_history_retryable_failure_exposes_deadline_and_recovers() {
    let _scenario = TestScenario::start();
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let sponsor_harness = DeviceHarness::new(rendezvous.uri());
    let joiner_harness = DeviceHarness::new(rendezvous.uri());
    let sponsor = sponsor_harness.start().await;
    let joiner = joiner_harness.start().await;
    let (space_id, _) = create_space(&sponsor, "Sponsor").await;
    let invitation = issue_invitation(&sponsor).await;
    let baseline = arm_membership_history_failures(
        &sponsor,
        uc_engine::DevMembershipHistoryFailure::Retryable,
        1,
    )
    .await;

    join_with_invitation(&joiner, "Joiner", &space_id, invitation).await;
    let failed = wait_for_space_work_event(
        &sponsor,
        baseline,
        uc_engine::DevSpaceWorkEventKind::MembershipHistorySyncRetryableFailure,
    )
    .await;
    // 旧式健康投影把“更新中”也显示为 Retrying；失败事件早于退避结果提交，须等到带重试时间的状态。
    let retrying = wait_for_maintenance_retry_deadline(&sponsor).await;
    assert!(retrying.next_retry_at_ms.is_some());
    assert_eq!(retrying.reason, None);
    assert_eq!(retrying.recovery, None);

    let recovered = wait_for_space_work_event(
        &sponsor,
        failed.sequence,
        uc_engine::DevSpaceWorkEventKind::MembershipHistorySyncReplyReceived,
    )
    .await;
    let events = query_space_work_events(&sponsor).await;
    let observed = events
        .iter()
        .filter(|event| event.sequence > baseline)
        .filter_map(|event| match event.kind {
            uc_engine::DevSpaceWorkEventKind::MembershipHistorySyncStarted
            | uc_engine::DevSpaceWorkEventKind::MembershipHistorySyncRetryableFailure
            | uc_engine::DevSpaceWorkEventKind::MembershipHistorySyncReplyReceived => {
                Some(event.kind)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        observed,
        vec![
            uc_engine::DevSpaceWorkEventKind::MembershipHistorySyncStarted,
            uc_engine::DevSpaceWorkEventKind::MembershipHistorySyncRetryableFailure,
            uc_engine::DevSpaceWorkEventKind::MembershipHistorySyncStarted,
            uc_engine::DevSpaceWorkEventKind::MembershipHistorySyncReplyReceived,
        ]
    );
    assert!(failed.sequence < recovered.sequence);
    let completed = wait_for_space_device_update(
        &sponsor,
        uc_engine::SpaceDeviceUpdatePhaseSummary::Completed,
    )
    .await;
    assert_eq!(completed.reason, None);
    assert_eq!(completed.recovery, None);
    assert_eq!(completed.next_retry_at_ms, None);

    sponsor.shutdown(SHUTDOWN_TIMEOUT).await.unwrap();
    joiner.shutdown(SHUTDOWN_TIMEOUT).await.unwrap();
}

// 稳定拒绝必须明确进入需处理；测试触发新的连接机会后，真实交换可以恢复健康。
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn membership_history_rejection_exposes_recovery_and_can_recover() {
    let _scenario = TestScenario::start();
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let sponsor_harness = DeviceHarness::new(rendezvous.uri());
    let joiner_harness = DeviceHarness::new(rendezvous.uri());
    let sponsor = sponsor_harness.start().await;
    let joiner = joiner_harness.start().await;
    let (space_id, _) = create_space(&sponsor, "Sponsor").await;
    let invitation = issue_invitation(&sponsor).await;
    let failure_count = 1_024;
    let baseline = arm_membership_history_failures(
        &sponsor,
        uc_engine::DevMembershipHistoryFailure::NeedsAttention,
        failure_count,
    )
    .await;

    join_with_invitation(&joiner, "Joiner", &space_id, invitation).await;
    let rejected = wait_for_space_work_event(
        &sponsor,
        baseline,
        uc_engine::DevSpaceWorkEventKind::MembershipHistorySyncNeedsAttention,
    )
    .await;
    let attention = wait_for_maintenance_health(
        &sponsor,
        uc_engine::MembershipMaintenanceHealthPhaseSummary::NeedsAttention,
    )
    .await;
    assert_eq!(
        attention.reason,
        Some(uc_engine::MembershipMaintenanceProblemSummary::MembershipHistoryRejected)
    );
    assert_eq!(
        attention.recovery,
        Some(uc_engine::MembershipMaintenanceRecoverySummary::ResolveDeviceTrust)
    );
    assert_eq!(attention.next_retry_at_ms, None);

    let remaining_failures = clear_membership_history_failures(&sponsor).await;
    assert!(remaining_failures < failure_count);
    assert!(remaining_failures > 0);
    sponsor
        .execute(Operation::NotifyConnectivityOpportunity {
            reason: uc_engine::ConnectivityOpportunity::NetworkChanged,
        })
        .await
        .expect("notify a new connectivity opportunity");
    let recovered = wait_for_space_work_event(
        &sponsor,
        rejected.sequence,
        uc_engine::DevSpaceWorkEventKind::MembershipHistorySyncReplyReceived,
    )
    .await;
    let after_recovery = sponsor
        .execute(Operation::QueryDeviceGroupChoices)
        .await
        .expect("query overall device update after history recovery");
    let OperationResult::DeviceGroupChoices(after_recovery) = after_recovery else {
        panic!("unexpected device group choices result");
    };
    assert_ne!(
        after_recovery.device_trust.maintenance_health.reason,
        Some(uc_engine::MembershipMaintenanceProblemSummary::MembershipHistoryRejected)
    );
    let completed = wait_for_space_device_update(
        &sponsor,
        uc_engine::SpaceDeviceUpdatePhaseSummary::Completed,
    )
    .await;
    assert_eq!(completed.reason, None);
    assert_eq!(completed.recovery, None);
    assert_eq!(completed.next_retry_at_ms, None);
    let events = query_space_work_events(&sponsor).await;
    assert_eq!(
        events
            .iter()
            .filter(|event| {
                event.sequence > baseline
                    && event.kind
                        == uc_engine::DevSpaceWorkEventKind::MembershipHistorySyncNeedsAttention
            })
            .count(),
        failure_count - remaining_failures
    );
    assert!(rejected.sequence < recovered.sequence);

    sponsor.shutdown(SHUTDOWN_TIMEOUT).await.unwrap();
    joiner.shutdown(SHUTDOWN_TIMEOUT).await.unwrap();
}
