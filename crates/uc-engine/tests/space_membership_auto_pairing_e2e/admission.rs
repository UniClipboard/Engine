//! 加入、最终确认、重启恢复、旧重复成员与同设备重新加入。

use super::*;

// 新设备只经过稳定 JoinSpace 入口，并最终形成可查询的活动 Space。
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn fresh_device_join_completes_through_stable_operations() {
    let _scenario = TestScenario::start();
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let first_harness = DeviceHarness::new(rendezvous.uri());
    let joiner_harness = DeviceHarness::new(rendezvous.uri());
    let first = first_harness.start().await;
    let joiner = joiner_harness.start().await;
    let first_space_id = create_space(&first, "First Sponsor").await.0;

    join_through(&first, &joiner, "Joining Device", &first_space_id).await;

    for engine in [&first, &joiner] {
        engine
            .shutdown(SHUTDOWN_TIMEOUT)
            .await
            .expect("shut down join routing engine");
    }
}

// 新成员加入后，已经在 Space 中的成员也必须收到同一更新并能联系新成员。
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn existing_member_receives_the_new_member_update() {
    let _scenario = TestScenario::start();
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let sponsor_harness = DeviceHarness::new(rendezvous.uri());
    let existing_harness = DeviceHarness::new(rendezvous.uri());
    let newcomer_harness = DeviceHarness::new(rendezvous.uri());
    let sponsor = sponsor_harness.start().await;
    let existing = existing_harness.start().await;
    let newcomer = newcomer_harness.start().await;
    let space_id = create_space(&sponsor, "Sponsor").await.0;

    let existing_id = join_through(&sponsor, &existing, "Existing Member", &space_id)
        .await
        .self_device_id;
    wait_for_active_member_count(&sponsor, 2).await;
    wait_for_active_member_count(&existing, 2).await;
    let newcomer_id = join_through(&sponsor, &newcomer, "New Member", &space_id)
        .await
        .self_device_id;

    for engine in [&sponsor, &existing, &newcomer] {
        wait_for_active_member_count(engine, 3).await;
    }
    tokio::join!(
        automatic_connections::wait_eligible(&existing, &newcomer_id),
        automatic_connections::wait_eligible(&newcomer, &existing_id),
    );
    automatic_connections::wait_online(&existing, &newcomer_id).await;
    automatic_connections::wait_online(&newcomer, &existing_id).await;
    let text = "existing member sees newcomer";
    let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
    loop {
        let sent = existing
            .execute(Operation::SendText(SendTextInput {
                text: text.to_owned(),
                target_devices: vec![newcomer_id.clone()],
            }))
            .await
            .expect("existing member sends to newcomer");
        let OperationResult::EntrySent(report) = sent else {
            panic!("unexpected send result");
        };
        if report.total_accepted == 1 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "existing member did not establish delivery to the newcomer"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    wait_for_received_text(&newcomer, text).await;

    for engine in [&sponsor, &existing, &newcomer] {
        engine
            .shutdown(SHUTDOWN_TIMEOUT)
            .await
            .expect("shut down three-member engine");
    }
    uc_engine::flush_test_tracing();
}

// 加入完成后重启 Joiner，持久化准入状态必须足以恢复成员权限并接收正文。
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn completed_admission_survives_restart_and_allows_transfer() {
    let _scenario = TestScenario::start();
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let sponsor_harness = DeviceHarness::new(rendezvous.uri());
    let joiner_harness = DeviceHarness::new(rendezvous.uri());
    let sponsor = sponsor_harness.start().await;
    let joiner = joiner_harness.start().await;
    let space_id = create_space(&sponsor, "Sponsor").await.0;
    let joiner_id = join_through(&sponsor, &joiner, "Joiner", &space_id)
        .await
        .self_device_id;
    joiner
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shut down admitted joiner");

    let restarted_joiner = joiner_harness.start().await;
    wait_for_peer_refresh(&sponsor, "sponsor").await;
    wait_for_peer_refresh(&restarted_joiner, "joiner").await;
    let text = "admission survives restart";
    sponsor
        .execute(Operation::SendText(SendTextInput {
            text: text.to_owned(),
            target_devices: vec![joiner_id],
        }))
        .await
        .expect("send text to restarted joiner");
    wait_for_received_text(&restarted_joiner, text).await;

    sponsor
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shut down sponsor");
    restarted_joiner
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shut down restarted joiner");
}

// 公开查询必须把已确认状态与准确成员一起持久保留；移除后同一设备可作为新实例重新配对。
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn confirmed_pairing_survives_restart_removal_and_same_device_rejoin() {
    let _scenario = TestScenario::start();
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let sponsor_harness = DeviceHarness::new(rendezvous.uri());
    let joiner_harness = DeviceHarness::new(rendezvous.uri());
    let sponsor = sponsor_harness.start().await;
    let joiner = joiner_harness.start().await;
    let space_id = create_space(&sponsor, "Sponsor").await.0;
    let joiner_id = join_through(&sponsor, &joiner, "Joiner", &space_id)
        .await
        .self_device_id;

    wait_for_pairing_confirmation(
        &sponsor,
        &joiner_id,
        PairingConfirmationSummary::Confirmed,
        "initial admission",
    )
    .await;
    sponsor
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shut down sponsor before confirmation reload");
    let restarted_sponsor = sponsor_harness.start().await;
    wait_for_pairing_confirmation(
        &restarted_sponsor,
        &joiner_id,
        PairingConfirmationSummary::Confirmed,
        "sponsor restart",
    )
    .await;

    remove_member(&restarted_sponsor, &joiner_id).await;
    wait_for_active_member_count(&restarted_sponsor, 1).await;
    let rejoined_id = join_through(&restarted_sponsor, &joiner, "Joiner", &space_id)
        .await
        .self_device_id;
    assert_eq!(rejoined_id, joiner_id);
    wait_for_pairing_confirmation(
        &restarted_sponsor,
        &joiner_id,
        PairingConfirmationSummary::Confirmed,
        "same-device rejoin",
    )
    .await;

    restarted_sponsor
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shut down rejoined sponsor");
    joiner
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shut down rejoined device");
}

async fn wait_for_pairing_confirmation(
    engine: &Engine,
    device_id: &str,
    expected: PairingConfirmationSummary,
    stage: &str,
) {
    let deadline = tokio::time::Instant::now() + ADMISSION_WAIT_TIMEOUT;
    let mut observed = None;
    loop {
        if let Ok(OperationResult::DeviceGroupChoices(summary)) =
            engine.execute(Operation::QueryDeviceGroupChoices).await
        {
            if let Some(device) = summary
                .device_trust
                .devices
                .iter()
                .find(|device| device.device_id == device_id)
            {
                observed = device.pairing_confirmation;
                if observed == Some(expected) {
                    return;
                }
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "pairing confirmation did not reach {expected:?} during {stage}; last observed: {observed:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn remove_member(engine: &Engine, device_id: &str) {
    let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
    loop {
        match engine
            .execute(Operation::RemoveMember(RemoveMemberInput {
                device_id: device_id.to_owned(),
            }))
            .await
        {
            Ok(OperationResult::DeviceTrust(_)) => return,
            Err(error) if error.is_retryable() && tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Ok(_) => panic!("unexpected remove member result"),
            Err(error) => panic!("remove member failed: {error}"),
        }
    }
}

async fn seed_legacy_duplicate_group_members(
    engine: &Engine,
    device_id: &str,
    additional_members: usize,
) {
    let result = engine
        .execute_dev(uc_engine::DevOperation::SeedLegacyDuplicateGroupMembers {
            device_id: device_id.to_owned(),
            additional_members,
        })
        .await
        .expect("seed legacy duplicate group members");
    assert_eq!(
        result,
        uc_engine::DevOperationResult::LegacyDuplicateGroupMembersSeeded
    );
}

async fn query_group_member_count(engine: &Engine, device_id: &str) -> usize {
    let result = engine
        .execute_dev(uc_engine::DevOperation::QueryGroupMemberCount {
            device_id: device_id.to_owned(),
        })
        .await
        .expect("query group member count");
    let uc_engine::DevOperationResult::GroupMemberCount { count } = result else {
        panic!("unexpected group member count result");
    };
    count
}

async fn arm_joiner_final_confirmation_pause(engine: &Engine) {
    assert_eq!(
        engine
            .execute_dev(uc_engine::DevOperation::ArmJoinerFinalConfirmationPause)
            .await
            .expect("arm Joiner final confirmation pause"),
        uc_engine::DevOperationResult::JoinerFinalConfirmationPauseArmed
    );
}

async fn wait_for_joiner_final_confirmation_pause(engine: &Engine) {
    assert_eq!(
        tokio::time::timeout(
            ADMISSION_WAIT_TIMEOUT,
            engine.execute_dev(uc_engine::DevOperation::WaitForJoinerFinalConfirmationPause),
        )
        .await
        .expect("Joiner did not reach the final confirmation pause")
        .expect("wait for Joiner final confirmation pause"),
        uc_engine::DevOperationResult::JoinerFinalConfirmationPauseEntered
    );
}

async fn release_joiner_final_confirmation_pause(engine: &Engine) {
    assert_eq!(
        engine
            .execute_dev(uc_engine::DevOperation::ReleaseJoinerFinalConfirmationPause)
            .await
            .expect("release Joiner final confirmation pause"),
        uc_engine::DevOperationResult::JoinerFinalConfirmationPauseReleased
    );
}

async fn arm_final_confirmation_connection_failure(engine: &Engine) -> u64 {
    let result = engine
        .execute_dev(uc_engine::DevOperation::ArmFinalConfirmationConnectionFailure)
        .await
        .expect("arm one final confirmation connection failure");
    let uc_engine::DevOperationResult::FinalConfirmationConnectionFailureArmed { after_sequence } =
        result
    else {
        panic!("unexpected final confirmation failure arm result");
    };
    after_sequence
}

async fn arm_final_confirmation_success_reply_drop(engine: &Engine) -> u64 {
    let result = engine
        .execute_dev(uc_engine::DevOperation::ArmFinalConfirmationSuccessReplyDrop)
        .await
        .expect("arm one final confirmation success reply drop");
    let uc_engine::DevOperationResult::FinalConfirmationSuccessReplyDropArmed { after_sequence } =
        result
    else {
        panic!("unexpected final confirmation success reply drop arm result");
    };
    after_sequence
}

async fn query_membership_diagnostics(engine: &Engine) -> uc_engine::MembershipDiagnosticsSummary {
    let OperationResult::MembershipDiagnostics(summary) = engine
        .execute(Operation::QueryMembershipDiagnostics)
        .await
        .expect("query membership diagnostics")
    else {
        panic!("unexpected membership diagnostics result");
    };
    summary
}

/// `engine` 显示设备更新已完成时，本机不能再有待确认、未完成效果或冲突，且与 `peer` 处于同一组密钥代数。
async fn assert_completed_only_when_settled(engine: &Engine, peer: &Engine) {
    if query_space_device_update(engine).await.phase
        != uc_engine::SpaceDeviceUpdatePhaseSummary::Completed
    {
        return;
    }
    let local = query_membership_diagnostics(engine).await;
    let remote = query_membership_diagnostics(peer).await;
    assert_eq!(
        (
            local.pending_confirmation_count,
            local.pending_effect_count,
            local.pending_conflict_count
        ),
        (0, 0, 0),
        "device update reported completion with outstanding membership work"
    );
    assert_eq!(
        local.group_epoch, remote.group_epoch,
        "device update reported completion before security material converged"
    );
}

// 旧版异常资料已经同步到本机后，正常移除必须清完同设备的全部旧身份，重启后邀请恢复。
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn legacy_duplicate_members_finish_normal_removal_before_new_invitation() {
    let _scenario = TestScenario::start();
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let sponsor_harness = DeviceHarness::new(rendezvous.uri());
    let removed_harness = DeviceHarness::new(rendezvous.uri());
    let next_harness = DeviceHarness::new(rendezvous.uri());
    let sponsor = sponsor_harness.start().await;
    let removed = removed_harness.start().await;
    let next = next_harness.start().await;
    let space_id = create_space(&sponsor, "Sponsor").await.0;
    let removed_device_id = join_through(&sponsor, &removed, "Old Device", &space_id)
        .await
        .self_device_id;

    seed_legacy_duplicate_group_members(&sponsor, &removed_device_id, 2).await;
    assert_eq!(
        query_group_member_count(&sponsor, &removed_device_id).await,
        3
    );
    removed
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shutdown old device before removal");

    remove_member(&sponsor, &removed_device_id).await;
    wait_for_active_member_count(&sponsor, 1).await;
    let OperationResult::DeviceGroupChoices(choices) = sponsor
        .execute(Operation::QueryDeviceGroupChoices)
        .await
        .expect("query device list after removal")
    else {
        panic!("unexpected device list result");
    };
    assert_eq!(
        choices
            .device_trust
            .devices
            .iter()
            .find(|device| device.device_id == removed_device_id)
            .map(|device| device.membership),
        Some(uc_engine::DeviceMembershipSummary::Removed)
    );
    assert_eq!(
        query_group_member_count(&sponsor, &removed_device_id).await,
        0,
        "normal removal must clear every legacy identity for the device"
    );

    sponsor
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shutdown sponsor before recovery check");
    let restarted = sponsor_harness.start().await;
    wait_for_active_member_count(&restarted, 1).await;
    assert_eq!(
        query_group_member_count(&restarted, &removed_device_id).await,
        0
    );
    let invitation = issue_invitation(&restarted).await;
    join_with_invitation(&next, "Next Device", &space_id, invitation).await;
    wait_for_active_member_count(&restarted, 2).await;

    restarted
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shutdown restarted sponsor");
    next.shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shutdown next device");
}

// 第三台在线设备只能在 Joiner 发出最终确认后看到新正式成员。
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn pending_join_is_not_published_before_final_confirmation() {
    let _scenario = TestScenario::start();
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let sponsor_harness = DeviceHarness::new(rendezvous.uri());
    let existing_harness = DeviceHarness::new(rendezvous.uri());
    let pending_harness = DeviceHarness::new(rendezvous.uri());
    let sponsor = Arc::new(sponsor_harness.start().await);
    let existing = Arc::new(existing_harness.start().await);
    let pending = Arc::new(pending_harness.start().await);
    let (space_id, sponsor_device_id) = create_space(&sponsor, "Sponsor").await;

    join_through(&sponsor, &existing, "Existing Device", &space_id).await;
    wait_for_active_member_count(&sponsor, 2).await;
    wait_for_active_member_count(&existing, 2).await;

    let invitation = issue_invitation(&sponsor).await;
    arm_joiner_final_confirmation_pause(&pending).await;
    let pending_joiner = Arc::clone(&pending);
    let pending_space_id = space_id.clone();
    let join = tokio::spawn(async move {
        join_with_invitation(
            &pending_joiner,
            "Pending Device",
            &pending_space_id,
            invitation,
        )
        .await
    });
    wait_for_joiner_final_confirmation_pause(&pending).await;

    sponsor
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shutdown Sponsor before final confirmation");
    let restarted_sponsor = Arc::new(sponsor_harness.start().await);
    restarted_sponsor
        .execute(Operation::NotifyConnectivityOpportunity {
            reason: uc_engine::ConnectivityOpportunity::Foreground,
        })
        .await
        .expect("notify Sponsor connectivity opportunity while confirmation is pending");
    restarted_sponsor
        .execute(Operation::RefreshPeerConnections)
        .await
        .expect("refresh Sponsor peers while confirmation is pending");
    existing
        .execute(Operation::SendText(SendTextInput {
            text: "trigger membership synchronization before final confirmation".to_owned(),
            target_devices: vec![sponsor_device_id.clone()],
        }))
        .await
        .expect("existing member contacts Sponsor while confirmation is pending");

    let OperationResult::DeviceGroupChoices(pending_snapshot) = restarted_sponsor
        .execute(Operation::QueryDeviceGroupChoices)
        .await
        .expect("query Sponsor while final confirmation is pending")
    else {
        panic!("device group query must return a device snapshot");
    };
    let pending_member = pending_snapshot
        .device_trust
        .pending_inbound_member
        .as_ref()
        .expect("Sponsor must expose the Joiner as pending confirmation");
    assert_eq!(pending_member.display_name, "Pending Device");
    assert!(pending_snapshot
        .device_trust
        .devices
        .iter()
        .all(|device| device.device_id != pending_member.device_id));

    assert!(
        tokio::time::timeout(
            Duration::from_secs(15),
            wait_for_active_member_count(&existing, 3),
        )
        .await
        .is_err(),
        "an online third device observed the pending Joiner before final confirmation"
    );
    assert!(
        tokio::time::timeout(
            Duration::from_secs(1),
            wait_for_active_member_count(&restarted_sponsor, 3),
        )
        .await
        .is_err(),
        "the Sponsor published the pending Joiner before final confirmation"
    );

    release_joiner_final_confirmation_pause(&pending).await;
    let joined = join.await.expect("pending Joiner task must not panic");
    wait_for_active_member_count(&restarted_sponsor, 3).await;
    wait_for_active_member_count(&existing, 3).await;
    wait_for_active_member_count(&pending, 3).await;
    let OperationResult::DeviceGroupChoices(confirmed_snapshot) = restarted_sponsor
        .execute(Operation::QueryDeviceGroupChoices)
        .await
        .expect("query Sponsor after final confirmation")
    else {
        panic!("device group query must return a device snapshot");
    };
    assert!(confirmed_snapshot
        .device_trust
        .pending_inbound_member
        .is_none());
    let sponsor_to_joiner = "confirmed member receives content";
    restarted_sponsor
        .execute(Operation::SendText(SendTextInput {
            text: sponsor_to_joiner.to_owned(),
            target_devices: vec![joined.self_device_id.clone()],
        }))
        .await
        .expect("send content to confirmed Joiner");
    wait_for_received_text(&pending, sponsor_to_joiner).await;
    let joiner_to_sponsor = "confirmed member sends content";
    pending
        .execute(Operation::SendText(SendTextInput {
            text: joiner_to_sponsor.to_owned(),
            target_devices: vec![sponsor_device_id],
        }))
        .await
        .expect("send content from confirmed Joiner");
    wait_for_received_text(&restarted_sponsor, joiner_to_sponsor).await;

    restarted_sponsor
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shutdown restarted sponsor");
    existing
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shutdown existing device");
    pending
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shutdown joined device");
}

// 最终确认首次建连暂时失败后，只能按持久时间重试配对；普通成员维护不得插队。
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn final_confirmation_retry_precedes_ordinary_membership_network_work() {
    let _scenario = TestScenario::start();
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let sponsor_harness = DeviceHarness::new(rendezvous.uri());
    let joiner_harness = DeviceHarness::new(rendezvous.uri());
    let sponsor = Arc::new(sponsor_harness.start().await);
    let joiner = Arc::new(joiner_harness.start().await);
    let (space_id, _) = create_space(&sponsor, "Sponsor").await;
    let invitation = issue_invitation(&sponsor).await;
    let baseline = arm_final_confirmation_connection_failure(&joiner).await;

    let joining_engine = Arc::clone(&joiner);
    let joining_space_id = space_id.clone();
    let joining = tokio::spawn(async move {
        join_with_invitation(
            &joining_engine,
            "Retrying Joiner",
            &joining_space_id,
            invitation,
        )
        .await
    });

    let failed = wait_for_space_work_event(
        &joiner,
        baseline,
        uc_engine::DevSpaceWorkEventKind::FinalConfirmationConnectionFailed,
    )
    .await;
    let retry = wait_for_space_work_event(
        &joiner,
        failed.sequence,
        uc_engine::DevSpaceWorkEventKind::FinalConfirmationRetryStarted,
    )
    .await;
    let events_before_retry = query_space_work_events(&joiner).await;
    assert!(
        events_before_retry.iter().all(|event| {
            event.sequence <= failed.sequence
                || event.sequence >= retry.sequence
                || !matches!(
                    event.kind,
                    uc_engine::DevSpaceWorkEventKind::OrdinaryMemberUpdateStarted
                        | uc_engine::DevSpaceWorkEventKind::MembershipHistorySyncStarted
                )
        }),
        "ordinary membership network work ran before the final confirmation retry: {events_before_retry:?}"
    );
    let succeeded = wait_for_space_work_event(
        &joiner,
        retry.sequence,
        uc_engine::DevSpaceWorkEventKind::FinalConfirmationReplyReceived,
    )
    .await;
    assert!(failed.sequence < retry.sequence);
    assert!(retry.sequence < succeeded.sequence);

    joining.await.expect("Joiner task must not panic");
    wait_for_active_member_count(&sponsor, 2).await;
    wait_for_active_member_count(&joiner, 2).await;
    sponsor
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shutdown Sponsor");
    joiner
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shutdown Joiner");
}

// 邀请方已持久提交后丢失第一个成功回复，加入方必须重试同一尝试并获得原成功结果。
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn committed_final_confirmation_replays_success_after_the_first_reply_is_lost() {
    let _scenario = TestScenario::start();
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let sponsor_harness = DeviceHarness::new(rendezvous.uri());
    let joiner_harness = DeviceHarness::new(rendezvous.uri());
    let sponsor = Arc::new(sponsor_harness.start().await);
    let joiner = Arc::new(joiner_harness.start().await);
    let (space_id, _) = create_space(&sponsor, "Sponsor").await;
    let invitation = issue_invitation(&sponsor).await;
    let baseline = arm_final_confirmation_success_reply_drop(&joiner).await;

    let joining_engine = Arc::clone(&joiner);
    let joining_space_id = space_id.clone();
    let joining = tokio::spawn(async move {
        join_with_invitation(
            &joining_engine,
            "Reply-loss Joiner",
            &joining_space_id,
            invitation,
        )
        .await
    });

    let committed = wait_for_space_work_event(
        &joiner,
        baseline,
        uc_engine::DevSpaceWorkEventKind::FinalConfirmationSponsorCommitted,
    )
    .await;
    let dropped = wait_for_space_work_event(
        &joiner,
        committed.sequence,
        uc_engine::DevSpaceWorkEventKind::FinalConfirmationSuccessReplyDropped,
    )
    .await;
    wait_for_active_member_count(&sponsor, 2).await;
    let retry = wait_for_space_work_event(
        &joiner,
        dropped.sequence,
        uc_engine::DevSpaceWorkEventKind::FinalConfirmationRetryStarted,
    )
    .await;
    let replayed = wait_for_space_work_event(
        &joiner,
        retry.sequence,
        uc_engine::DevSpaceWorkEventKind::FinalConfirmationReplyReceived,
    )
    .await;
    assert!(committed.sequence < dropped.sequence);
    assert!(dropped.sequence < retry.sequence);
    assert!(retry.sequence < replayed.sequence);

    joining.await.expect("Joiner task must not panic");
    wait_for_active_member_count(&joiner, 2).await;
    sponsor
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shutdown Sponsor");
    joiner
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shutdown Joiner");
}

// Joiner 已保存最终确认、但请求尚未送达时重启，必须从持久状态继续同一加入。
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn pending_final_confirmation_survives_joiner_restart() {
    let _scenario = TestScenario::start();
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let sponsor_harness = DeviceHarness::new(rendezvous.uri());
    let joiner_harness = DeviceHarness::new(rendezvous.uri());
    let sponsor = Arc::new(sponsor_harness.start().await);
    let joiner = Arc::new(joiner_harness.start().await);
    let (space_id, _) = create_space(&sponsor, "Sponsor").await;
    let sponsor_endpoint = query_endpoint_id(&sponsor, "Sponsor before Joiner restart").await;
    let joiner_endpoint = query_endpoint_id(&joiner, "Joiner before restart").await;
    let invitation = issue_invitation(&sponsor).await;
    arm_joiner_final_confirmation_pause(&joiner).await;
    let task_joiner = Arc::clone(&joiner);
    let task_space_id = space_id.clone();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let joining = tokio::spawn(async move {
        let OperationResult::JoinSpace(status) = task_joiner
            .execute(Operation::JoinSpace(JoinSpaceInput {
                invitation_code: invitation,
                device_name: Some("Restarting Joiner".to_owned()),
                passphrase: SecretString::new(PASSPHRASE),
                preserve_unreadable_history: false,
            }))
            .await
            .expect("start admission before Joiner restart")
        else {
            panic!("unexpected join result");
        };
        started_tx
            .send(status.clone())
            .expect("publish initial join status");
        wait_for_completed_join(&task_joiner, "Restarting Joiner", status, &task_space_id).await
    });
    let initial_status = started_rx.await.expect("receive initial join status");
    let initial_join_id = join_status_id(&initial_status).to_owned();
    wait_for_joiner_final_confirmation_pause(&joiner).await;
    set_network_partition(&joiner, vec![sponsor_endpoint]).await;
    set_network_partition(&sponsor, vec![joiner_endpoint]).await;
    release_joiner_final_confirmation_pause(&joiner).await;

    let pending_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let processing_join_id = loop {
        if let Ok(OperationResult::DeviceGroupChoices(summary)) =
            joiner.execute(Operation::QueryDeviceGroupChoices).await
        {
            if let Some(JoinSpaceStatusSummary::Processing {
                join_id,
                target_space_id,
                ..
            }) = summary.device_trust.current_join
            {
                if target_space_id == space_id {
                    break join_id;
                }
            }
        }
        assert!(
            tokio::time::Instant::now() < pending_deadline,
            "Joiner did not expose the saved pending confirmation"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(processing_join_id, initial_join_id);
    assert!(matches!(
        initial_status,
        JoinSpaceStatusSummary::Pending { .. }
    ));
    let OperationResult::MembershipDiagnostics(pending_diagnostics) = sponsor
        .execute(Operation::QueryMembershipDiagnostics)
        .await
        .expect("query Sponsor before final confirmation")
    else {
        panic!("unexpected membership diagnostics result");
    };
    assert_eq!(pending_diagnostics.effective_member_count, 1);
    joining.abort();
    let _ = joining.await;
    joiner
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shutdown Joiner with pending final confirmation");

    set_network_partition(&sponsor, Vec::new()).await;
    let restarted_joiner = joiner_harness.start().await;
    let current = loop {
        let OperationResult::DeviceGroupChoices(summary) = restarted_joiner
            .execute(Operation::QueryDeviceGroupChoices)
            .await
            .expect("query restarted Joiner")
        else {
            panic!("unexpected device group result");
        };
        if let Some(status) = summary.device_trust.current_join {
            break status;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(join_status_id(&current), initial_join_id);
    let OperationResult::DeviceGroupChoices(repeated) = restarted_joiner
        .execute(Operation::QueryDeviceGroupChoices)
        .await
        .expect("repeat current Joiner query after restart")
    else {
        panic!("unexpected repeated device group result");
    };
    let repeated = repeated
        .device_trust
        .current_join
        .expect("restarted Joiner remains queryable");
    assert_eq!(join_status_id(&repeated), initial_join_id);
    let joined =
        wait_for_completed_join(&restarted_joiner, "Restarting Joiner", current, &space_id).await;
    wait_for_active_member_count(&sponsor, 2).await;
    wait_for_active_member_count(&restarted_joiner, 2).await;
    let text = "confirmation survives Joiner restart";
    sponsor
        .execute(Operation::SendText(SendTextInput {
            text: text.to_owned(),
            target_devices: vec![joined.self_device_id],
        }))
        .await
        .expect("send to restarted confirmed Joiner");
    wait_for_received_text(&restarted_joiner, text).await;
    // 内容已可传输不代表设备状态已全部更新完成：整体状态只有在成员与安全资料都已确认时才能显示完成，
    // 不能由传输成功推断。
    assert_completed_only_when_settled(&sponsor, &restarted_joiner).await;
    assert_completed_only_when_settled(&restarted_joiner, &sponsor).await;

    sponsor
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shutdown Sponsor");
    restarted_joiner
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shutdown restarted Joiner");
}

fn join_status_id(status: &JoinSpaceStatusSummary) -> &str {
    match status {
        JoinSpaceStatusSummary::Active { join_id, .. }
        | JoinSpaceStatusSummary::Pending { join_id, .. }
        | JoinSpaceStatusSummary::Processing { join_id, .. }
        | JoinSpaceStatusSummary::NeedsAttention { join_id, .. }
        | JoinSpaceStatusSummary::Rejected { join_id, .. }
        | JoinSpaceStatusSummary::Terminated { join_id, .. } => join_id,
    }
}
