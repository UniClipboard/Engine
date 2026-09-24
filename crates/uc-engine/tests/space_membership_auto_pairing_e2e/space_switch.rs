//! 空间切换、会话交接故障恢复与切换中的文件发送。

use super::*;

struct SlowReadableFiles {
    read_count: Arc<AtomicUsize>,
    size_bytes: u64,
    delay: Duration,
}

impl HostFileAccess for SlowReadableFiles {
    fn metadata(&self, _handle: &HostFileHandle) -> Result<HostFileMetadata, HostCapabilityError> {
        Ok(HostFileMetadata {
            display_name: "slow-file.bin".to_owned(),
            size_bytes: self.size_bytes,
            mime_type: Some("application/octet-stream".to_owned()),
        })
    }

    fn read_chunk(
        &self,
        _handle: &HostFileHandle,
        offset: u64,
        max_bytes: u32,
    ) -> Result<Vec<u8>, HostCapabilityError> {
        self.read_count.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(self.delay);
        let remaining = self.size_bytes.saturating_sub(offset);
        let length = remaining.min(u64::from(max_bytes)) as usize;
        Ok(vec![0x5a; length])
    }

    fn write_chunk(
        &self,
        _handle: &HostFileHandle,
        _offset: u64,
        _bytes: &[u8],
    ) -> Result<(), HostCapabilityError> {
        Ok(())
    }

    fn finish_write(&self, _handle: &HostFileHandle) -> Result<(), HostCapabilityError> {
        Ok(())
    }
}

// 已完成设置的设备通过同一 JoinSpace 入口切换到另一个 Space。
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn existing_device_switches_space_through_stable_operations() {
    let _scenario = TestScenario::start();
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let first_harness = DeviceHarness::new(rendezvous.uri());
    let joiner_harness = DeviceHarness::new(rendezvous.uri());
    let second_harness = DeviceHarness::new(rendezvous.uri());
    let first = first_harness.start().await;
    let joiner = joiner_harness.start().await;
    let second = second_harness.start().await;
    let first_space_id = create_space(&first, "First Sponsor").await.0;
    let second_space_id = create_space(&second, "Second Sponsor").await.0;
    join_through(&first, &joiner, "Joining Device", &first_space_id).await;

    join_through(&second, &joiner, "Joining Device", &second_space_id).await;

    for engine in [&first, &joiner, &second] {
        engine
            .shutdown(SHUTDOWN_TIMEOUT)
            .await
            .expect("shut down space switch engine");
    }
}

// Space 切换连续封口失败后必须重试，且不能重绑网络入口。
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn space_switch_recovers_after_repeated_session_quiesce_failures() {
    let _scenario = TestScenario::start();
    assert_space_switch_recovers_after_repeated_handover_failures(
        uc_engine::SessionHandoverFailurePoint::SessionQuiesce,
        2,
    )
    .await;
}

// Space 切换连续提交失败后必须从权威状态恢复，且不能重绑网络入口。
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn space_switch_recovers_after_repeated_transition_completion_failures() {
    let _scenario = TestScenario::start();
    assert_space_switch_recovers_after_repeated_handover_failures(
        uc_engine::SessionHandoverFailurePoint::TransitionCompletion,
        2,
    )
    .await;
}

// 已提交的 Space 切换即使连续会话准备失败，也只能恢复目标 Space，且不能重绑网络入口。
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn committed_space_switch_recovers_after_repeated_session_preparation_failures() {
    let _scenario = TestScenario::start();
    assert_space_switch_recovers_after_repeated_handover_failures(
        uc_engine::SessionHandoverFailurePoint::SessionPreparation,
        2,
    )
    .await;
}

// 已准备的新会话连续发布失败后必须被清理并重试，且不能重绑网络入口。
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn committed_space_switch_recovers_after_repeated_session_activation_failures() {
    let _scenario = TestScenario::start();
    assert_space_switch_recovers_after_repeated_handover_failures(
        uc_engine::SessionHandoverFailurePoint::SessionActivation,
        2,
    )
    .await;
}

// Space 切换恢复空窗内暂停后，旧恢复任务不得重建网络；恢复时只允许重建一次。
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn suspend_during_space_switch_recovery_does_not_resurrect_the_network() {
    let _scenario = TestScenario::start();
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let sponsor_harness = DeviceHarness::new(rendezvous.uri());
    let joiner_harness = DeviceHarness::new(rendezvous.uri());
    let sponsor = sponsor_harness.start().await;
    let joiner = joiner_harness.start().await;
    let target_space_id = create_space(&sponsor, "Sponsor").await.0;
    create_space(&joiner, "Joiner").await;
    assert_eq!(
        query_session_handover_diagnostics(&joiner)
            .await
            .network_build_count,
        1
    );
    arm_session_handover_failure(
        &joiner,
        uc_engine::SessionHandoverFailurePoint::SessionPreparation,
    )
    .await;

    let invitation = issue_invitation(&sponsor).await;
    let OperationResult::JoinSpace(status) = joiner
        .execute(Operation::JoinSpace(JoinSpaceInput {
            invitation_code: invitation,
            device_name: Some("Joiner".to_owned()),
            passphrase: SecretString::new(PASSPHRASE),
            preserve_unreadable_history: false,
        }))
        .await
        .expect("start suspend-during-transition join")
    else {
        panic!("unexpected join result");
    };

    let failure_deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let diagnostics = query_session_handover_diagnostics(&joiner).await;
        if diagnostics.session_preparation_failure_count == 1 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < failure_deadline,
            "the injected session preparation failure was not observed"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    joiner
        .suspend()
        .await
        .expect("suspend joiner during recovery");
    tokio::time::sleep(Duration::from_millis(1_200)).await;
    joiner
        .resume()
        .await
        .expect("resume joiner after recovery gap");

    wait_for_completed_join(&joiner, "Joiner", status, &target_space_id).await;
    wait_for_active_member_count(&sponsor, 2).await;
    wait_for_active_member_count(&joiner, 2).await;
    assert_eq!(
        query_session_handover_diagnostics(&joiner)
            .await
            .network_build_count,
        2,
        "suspend must close the original network and resume must rebuild it exactly once"
    );

    for engine in [&sponsor, &joiner] {
        engine
            .shutdown(SHUTDOWN_TIMEOUT)
            .await
            .expect("shut down suspend-during-transition engine");
    }
}

// Space 切换必须取消仍在读取宿主文件的旧发送，并清理未完成的导入文件。
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn space_switch_cancels_in_flight_file_send_without_leaving_imports() {
    let _scenario = TestScenario::start();
    const FILE_CHUNKS: u64 = 70;
    const FILE_CHUNK_BYTES: u64 = 64 * 1024;

    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let sponsor_harness = DeviceHarness::new(rendezvous.uri());
    let joiner_harness = DeviceHarness::new(rendezvous.uri());
    let sponsor = sponsor_harness.start().await;
    let read_count = Arc::new(AtomicUsize::new(0));
    let joiner = Arc::new(
        joiner_harness
            .start_with_files(Box::new(SlowReadableFiles {
                read_count: Arc::clone(&read_count),
                size_bytes: FILE_CHUNKS * FILE_CHUNK_BYTES,
                delay: Duration::from_millis(100),
            }))
            .await,
    );
    let target_space_id = create_space(&sponsor, "Sponsor").await.0;
    create_space(&joiner, "Joiner").await;
    let endpoint_before = query_endpoint_id(&joiner, "joiner before in-flight send").await;

    let sending_engine = Arc::clone(&joiner);
    let send = tokio::spawn(async move {
        sending_engine
            .execute(Operation::SendFiles(uc_engine::SendFilesInput {
                files: vec![HostFileHandle::new("slow-file")],
                target_devices: Vec::new(),
            }))
            .await
    });
    let read_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while read_count.load(Ordering::SeqCst) == 0 {
        assert!(
            tokio::time::Instant::now() < read_deadline,
            "the file send did not start reading"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let invitation = issue_invitation(&sponsor).await;
    let OperationResult::JoinSpace(status) = joiner
        .execute(Operation::JoinSpace(JoinSpaceInput {
            invitation_code: invitation,
            device_name: Some("Joiner".to_owned()),
            passphrase: SecretString::new(PASSPHRASE),
            preserve_unreadable_history: false,
        }))
        .await
        .expect("start join while file send is in flight")
    else {
        panic!("unexpected join result");
    };

    let send_error = tokio::time::timeout(Duration::from_secs(10), send)
        .await
        .expect("in-flight file send must stop during Space switch")
        .expect("file send task must not panic")
        .expect_err("the old file send must be cancelled");
    assert_eq!(
        send_error.category(),
        uc_engine::EngineErrorCategory::Unavailable
    );
    let joined = wait_for_completed_join(&joiner, "Joiner", status, &target_space_id).await;
    wait_for_active_member_count(&sponsor, 2).await;
    wait_for_active_member_count(&joiner, 2).await;
    assert_eq!(
        query_endpoint_id(&joiner, "joiner after in-flight send").await,
        endpoint_before,
        "cancelling an old file send must keep the existing network"
    );
    assert_eq!(
        query_session_handover_diagnostics(&joiner)
            .await
            .network_build_count,
        1
    );

    let import_root = joiner_harness.root.path().join("cache/engine-imports");
    let import_count = std::fs::read_dir(&import_root)
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert_eq!(
        import_count, 0,
        "cancelled file send must remove its incomplete import"
    );

    let text = "new session remains usable after file cancellation";
    sponsor
        .execute(Operation::SendText(SendTextInput {
            text: text.to_owned(),
            target_devices: vec![joined.self_device_id],
        }))
        .await
        .expect("send through the new session after file cancellation");
    wait_for_received_text(&joiner, text).await;

    sponsor
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shut down sponsor after file-send cancellation");
    joiner
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shut down joiner after file-send cancellation");
}

async fn assert_space_switch_recovers_after_repeated_handover_failures(
    failure_point: uc_engine::SessionHandoverFailurePoint,
    failure_repetitions: usize,
) {
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let sponsor_harness = DeviceHarness::new(rendezvous.uri());
    let joiner_harness = DeviceHarness::new(rendezvous.uri());
    let sponsor = sponsor_harness.start().await;
    let joiner = joiner_harness.start().await;
    let target_space_id = create_space(&sponsor, "Sponsor").await.0;
    let source_space_id = create_space(&joiner, "Joiner").await.0;
    let endpoint_before = query_endpoint_id(&joiner, "joiner before injected failure").await;
    let initial_diagnostics = query_session_handover_diagnostics(&joiner).await;
    assert_eq!(initial_diagnostics.network_build_count, 1);
    assert_eq!(
        initial_diagnostics.failure_count(failure_point),
        0,
        "the selected failure point must start unused"
    );
    for _ in 0..failure_repetitions {
        arm_session_handover_failure(&joiner, failure_point).await;
    }

    let invitation = issue_invitation(&sponsor).await;
    let OperationResult::JoinSpace(status) = joiner
        .execute(Operation::JoinSpace(JoinSpaceInput {
            invitation_code: invitation,
            device_name: Some("Joiner".to_owned()),
            passphrase: SecretString::new(PASSPHRASE),
            preserve_unreadable_history: false,
        }))
        .await
        .expect("start injected-failure join")
    else {
        panic!("unexpected join result");
    };

    let failure_deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let diagnostics = query_session_handover_diagnostics(&joiner).await;
        if diagnostics.failure_count(failure_point) >= 1 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < failure_deadline,
            "the injected session handover failure was not observed"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let unavailable = joiner
        .execute(Operation::QuerySetupState)
        .await
        .expect_err("old Space operations must stay closed after handover failure");
    assert_eq!(
        unavailable.category(),
        uc_engine::EngineErrorCategory::Unavailable
    );
    assert_eq!(
        query_endpoint_id(&joiner, "joiner during injected failure").await,
        endpoint_before,
        "session recovery must keep the existing endpoint"
    );
    let failure_diagnostics = query_session_handover_diagnostics(&joiner).await;
    assert_eq!(failure_diagnostics.network_build_count, 1);
    assert!(failure_diagnostics.failure_count(failure_point) >= 1);

    wait_for_completed_join(&joiner, "Joiner", status, &target_space_id).await;
    assert_ne!(source_space_id, target_space_id);
    assert_eq!(
        query_endpoint_id(&joiner, "joiner after injected failure").await,
        endpoint_before,
        "successful retry must not rebind the endpoint"
    );
    let recovered_diagnostics = query_session_handover_diagnostics(&joiner).await;
    assert_eq!(
        recovered_diagnostics.network_build_count, 1,
        "repeated failures and the successful retry must reuse the original network"
    );
    assert_eq!(
        recovered_diagnostics.failure_count(failure_point),
        failure_repetitions
    );
    wait_for_active_member_count(&sponsor, 2).await;
    wait_for_active_member_count(&joiner, 2).await;

    for engine in [&sponsor, &joiner] {
        engine
            .shutdown(SHUTDOWN_TIMEOUT)
            .await
            .expect("shut down injected-failure engine");
    }
}

struct SessionHandoverDiagnostics {
    network_build_count: usize,
    session_quiesce_failure_count: usize,
    transition_completion_failure_count: usize,
    session_preparation_failure_count: usize,
    session_activation_failure_count: usize,
}

impl SessionHandoverDiagnostics {
    fn failure_count(&self, point: uc_engine::SessionHandoverFailurePoint) -> usize {
        match point {
            uc_engine::SessionHandoverFailurePoint::SessionQuiesce => {
                self.session_quiesce_failure_count
            }
            uc_engine::SessionHandoverFailurePoint::TransitionCompletion => {
                self.transition_completion_failure_count
            }
            uc_engine::SessionHandoverFailurePoint::SessionPreparation => {
                self.session_preparation_failure_count
            }
            uc_engine::SessionHandoverFailurePoint::SessionActivation => {
                self.session_activation_failure_count
            }
        }
    }
}

async fn arm_session_handover_failure(
    engine: &Engine,
    point: uc_engine::SessionHandoverFailurePoint,
) {
    let result = engine
        .execute_dev(uc_engine::DevOperation::FailNextSessionHandover { point })
        .await
        .expect("arm session handover failure");
    assert_eq!(
        result,
        uc_engine::DevOperationResult::SessionHandoverFailureArmed
    );
}

async fn query_session_handover_diagnostics(engine: &Engine) -> SessionHandoverDiagnostics {
    let result = engine
        .execute_dev(uc_engine::DevOperation::QuerySessionHandoverDiagnostics)
        .await
        .expect("query session handover diagnostics");
    let uc_engine::DevOperationResult::SessionHandoverDiagnostics {
        network_build_count,
        session_quiesce_failure_count,
        transition_completion_failure_count,
        session_preparation_failure_count,
        session_activation_failure_count,
    } = result
    else {
        panic!("unexpected session handover diagnostics result");
    };
    SessionHandoverDiagnostics {
        network_build_count,
        session_quiesce_failure_count,
        transition_completion_failure_count,
        session_preparation_failure_count,
        session_activation_failure_count,
    }
}

// 同一设备切换到其他 Space 并重启后再次加入原 Space，新实例必须接替旧实例。
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn same_device_returns_to_a_previous_space_after_switch_and_restart() {
    let _scenario = TestScenario::start();
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let first_harness = DeviceHarness::new(rendezvous.uri());
    let joiner_harness = DeviceHarness::new(rendezvous.uri());
    let second_harness = DeviceHarness::new(rendezvous.uri());
    let first = first_harness.start().await;
    let joiner = joiner_harness.start().await;
    let second = second_harness.start().await;
    let first_space_id = create_space(&first, "First Sponsor").await.0;
    let second_space_id = create_space(&second, "Second Sponsor").await.0;

    let first_join = join_through(&first, &joiner, "Joining Device", &first_space_id).await;
    join_through(&second, &joiner, "Joining Device", &second_space_id).await;
    joiner
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shutdown joiner before restart");

    let restarted_joiner = joiner_harness.start().await;
    let returned = join_through(
        &first,
        &restarted_joiner,
        "Returning Device",
        &first_space_id,
    )
    .await;

    assert_eq!(returned.self_device_id, first_join.self_device_id);
    wait_for_active_member_count(&first, 2).await;
    wait_for_active_member_count(&restarted_joiner, 2).await;
    first
        .execute(Operation::SendText(SendTextInput {
            text: "same-device return completed".to_owned(),
            target_devices: vec![returned.self_device_id],
        }))
        .await
        .expect("send after same-device return");
    wait_for_received_text(&restarted_joiner, "same-device return completed").await;

    first
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shutdown first sponsor");
    restarted_joiner
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shutdown returned joiner");
    second
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shutdown second sponsor");
}
