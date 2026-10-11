//! 组密钥更新投递：接收方的暂时失败与真实拒绝必须区分，暂时失败自动恢复，真实拒绝保持阻塞。
//!
//! 场景使用完整 Engine（真实 SQLite、MLS 与本机回环 QUIC）。接收方的暂时失败由 dev-tools 在接收端口
//! 注入；真实拒绝由发送方在线上篡改 MLS 提交字节，接收方走真实校验得到。

use super::*;

use uc_engine::{
    DevGroupUpdateReceiveFault, DevOperation, DevOperationResult, DevSpaceWorkEventKind,
    DeviceGroupUpdateRecoverySummary, DeviceGroupUpdateStateSummary, DeviceGroupUpdateSummary,
    SpaceDeviceUpdatePhaseSummary, SpaceDeviceUpdateProblemSummary,
};

/// 暂时失败后的首次退避是 30 秒，留出一次退避加余量。
const RECOVERY_TIMEOUT: Duration = Duration::from_secs(120);
const OBSERVATIONS: usize = 5;
const OBSERVATION_INTERVAL: Duration = Duration::from_millis(400);

async fn set_receive_fault(engine: &Engine, fault: Option<DevGroupUpdateReceiveFault>) {
    let result = engine
        .execute_dev(DevOperation::SetGroupUpdateReceiveFault { fault })
        .await
        .expect("set group update receive fault");
    assert!(matches!(
        result,
        DevOperationResult::GroupUpdateReceiveFaultSet
    ));
}

async fn update_phase(topology: &MembershipTopology, node: &str) -> SpaceDeviceUpdatePhaseSummary {
    topology
        .device_group_choices(node)
        .await
        .device_trust
        .space_device_update
        .phase
}

/// 阶段必须到达期望值并连续保持，避免把瞬时读数当成最终状态。
async fn wait_for_stable_phase(
    topology: &MembershipTopology,
    node: &str,
    expected: SpaceDeviceUpdatePhaseSummary,
) {
    let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
    let mut stable = 0;
    loop {
        let phase = update_phase(topology, node).await;
        if phase == expected {
            stable += 1;
            if stable == OBSERVATIONS {
                return;
            }
        } else {
            stable = 0;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "node {node} device update did not settle at {expected:?}; last phase {phase:?}"
        );
        tokio::time::sleep(OBSERVATION_INTERVAL).await;
    }
}

/// 本节点作为接收方成功应用的组密钥更新数（dev-tools 在接收端口记录）。
async fn applied_updates(engine: &Engine) -> usize {
    query_space_work_events(engine)
        .await
        .iter()
        .filter(|event| event.kind == DevSpaceWorkEventKind::GroupUpdateApplied)
        .count()
}

/// 启动 `nodes` 全部节点，A 创建空间，`members` 中除 A 外的节点依次经 A 加入；其余节点留作后续加入者。
async fn space_with_members(
    rendezvous_base_url: String,
    nodes: &[&'static str],
    members: &[&'static str],
) -> MembershipTopology {
    let mut topology = MembershipTopology::new(rendezvous_base_url);
    for node in nodes {
        topology.run(&[TopologyAction::Start { node }]).await;
    }
    topology.run(&[TopologyAction::Create { node: "A" }]).await;
    for joiner in members.iter().filter(|member| **member != "A") {
        topology
            .run(&[TopologyAction::Join {
                sponsor: "A",
                joiner,
            }])
            .await;
    }
    topology
        .wait_for_equivalent_branch(members, members.len() as u32)
        .await;
    // 加入本身产生的投递先结清，后续断言只看场景动作带来的更新。
    wait_for_stable_phase(&topology, "A", SpaceDeviceUpdatePhaseSummary::Completed).await;
    topology
}

async fn space_with_member_b(rendezvous_base_url: String) -> MembershipTopology {
    space_with_members(rendezvous_base_url, &["A", "B", "C"], &["A", "B"]).await
}

/// 本机对 `peer` 尚未确认的组密钥更新投影。
async fn delivery_to(
    topology: &MembershipTopology,
    node: &str,
    peer: &str,
) -> Option<DeviceGroupUpdateSummary> {
    let peer_id = topology.device_ids[peer].clone();
    topology
        .device_group_choices(node)
        .await
        .device_trust
        .devices
        .into_iter()
        .find(|device| device.device_id == peer_id)
        .and_then(|device| device.group_update)
}

async fn wait_for_delivery(
    topology: &MembershipTopology,
    node: &str,
    peer: &str,
    state: DeviceGroupUpdateStateSummary,
) -> DeviceGroupUpdateSummary {
    let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
    loop {
        let observed = delivery_to(topology, node, peer).await;
        if let Some(update) = observed.filter(|update| update.state == state) {
            return update;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "node {node} never projected {state:?} for {peer}; last {observed:?}"
        );
        tokio::time::sleep(OBSERVATION_INTERVAL).await;
    }
}

async fn wait_for_no_delivery(topology: &MembershipTopology, node: &str, peer: &str) {
    let deadline = tokio::time::Instant::now() + RECOVERY_TIMEOUT;
    loop {
        let observed = delivery_to(topology, node, peer).await;
        if observed.is_none() {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "node {node} still projects an unconfirmed update for {peer}: {observed:?}"
        );
        tokio::time::sleep(OBSERVATION_INTERVAL).await;
    }
}

async fn arm_dev(engine: &Engine, operation: DevOperation) {
    engine
        .execute_dev(operation)
        .await
        .expect("arm dev-tools control");
}

async fn update_reason(
    topology: &MembershipTopology,
    node: &str,
) -> Option<SpaceDeviceUpdateProblemSummary> {
    topology
        .device_group_choices(node)
        .await
        .device_trust
        .space_device_update
        .reason
}

// G1：接收方暂时不可用时，发送方延后重试而不是永久拒绝；故障解除后 B 自动应用该 epoch，内容双向可达。
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn g1_receiver_not_ready_defers_then_applies_after_recovery() {
    let _scenario = TestScenario::start();
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let mut topology = space_with_member_b(rendezvous.uri()).await;
    let baseline = topology.diagnostics("B").await.group_epoch;
    let applied_before = applied_updates(topology.engine("B")).await;
    set_receive_fault(
        topology.engine("B"),
        Some(DevGroupUpdateReceiveFault::NotReady),
    )
    .await;
    topology
        .run(&[TopologyAction::Join {
            sponsor: "A",
            joiner: "C",
        }])
        .await;
    topology.wait_for_equivalent_branch(&["A", "C"], 3).await;
    let target = topology.diagnostics("A").await.group_epoch;
    assert!(target > baseline, "joining C must advance the group epoch");

    wait_for_space_work_event(
        topology.engine("B"),
        0,
        DevSpaceWorkEventKind::GroupUpdateReceiveFaultInjected,
    )
    .await;
    wait_for_stable_phase(
        &topology,
        "A",
        SpaceDeviceUpdatePhaseSummary::RetryableFailure,
    )
    .await;
    assert_eq!(
        applied_updates(topology.engine("B")).await,
        applied_before,
        "B must not apply any update while its apply path is failing"
    );

    set_receive_fault(topology.engine("B"), None).await;
    tokio::time::timeout(
        RECOVERY_TIMEOUT,
        wait_for_space_work_event(
            topology.engine("B"),
            0,
            DevSpaceWorkEventKind::GroupUpdateApplied,
        ),
    )
    .await
    .expect("B applies the deferred update after the fault is cleared");
    wait_for_stable_phase(&topology, "A", SpaceDeviceUpdatePhaseSummary::Completed).await;

    let text = "g1 content from A after recovery";
    assert_eq!(topology.send("A", "B", text).await.total_accepted, 1);
    wait_for_received_text(topology.engine("B"), text).await;
    let reply = "g1 content from B after recovery";
    assert_eq!(topology.send("B", "A", reply).await.total_accepted, 1);
    wait_for_received_text(topology.engine("A"), reply).await;
    topology.shutdown().await;
}

// G2：接收方的真实密码学拒绝（线上提交字节被篡改）保持阻塞、原因与设备可见；重连不会绕过，
// 同一收件人的后序 epoch 不越过它；移除该设备后欠账结清。
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn g2_genuine_rejection_stays_blocked_and_visible_until_the_device_is_removed() {
    let _scenario = TestScenario::start();
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let mut topology =
        space_with_members(rendezvous.uri(), &["A", "B", "C", "D"], &["A", "B"]).await;

    arm_dev(
        topology.engine("A"),
        DevOperation::ArmGroupUpdateTamper { count: 1 },
    )
    .await;
    topology
        .run(&[TopologyAction::Join {
            sponsor: "A",
            joiner: "C",
        }])
        .await;
    topology.wait_for_equivalent_branch(&["A", "C"], 3).await;
    let rejected = wait_for_delivery(
        &topology,
        "A",
        "B",
        DeviceGroupUpdateStateSummary::RejectedByReceiver,
    )
    .await;
    assert_eq!(
        rejected.recovery,
        DeviceGroupUpdateRecoverySummary::ReviewDevice
    );
    assert_eq!(rejected.attempts, 1);
    assert_eq!(rejected.next_retry_at_ms, None);
    wait_for_stable_phase(
        &topology,
        "A",
        SpaceDeviceUpdatePhaseSummary::NeedsAttention,
    )
    .await;
    assert_eq!(
        update_reason(&topology, "A").await,
        Some(SpaceDeviceUpdateProblemSummary::DeviceSecurityUpdateRejected)
    );

    // 重新认证（B 重启）不是放行证据：不重发，也不应用。
    topology.restart("B").await;
    topology.wait_for_connected_peer("A", "B").await;
    // 后序 epoch 不越过队首的拒绝。
    topology
        .run(&[TopologyAction::Join {
            sponsor: "A",
            joiner: "D",
        }])
        .await;
    topology
        .wait_for_equivalent_branch(&["A", "C", "D"], 4)
        .await;
    tokio::time::sleep(Duration::from_secs(8)).await;
    let still = delivery_to(&topology, "A", "B")
        .await
        .expect("rejection retained");
    assert_eq!(
        still.state,
        DeviceGroupUpdateStateSummary::RejectedByReceiver
    );
    assert_eq!(
        still.epoch, rejected.epoch,
        "the rejected head must keep blocking later epochs"
    );
    assert_eq!(
        still.attempts, 1,
        "a genuine rejection must never be resent"
    );

    // 移除被拒绝的设备：它不再是收件人，欠账结清。
    topology
        .run(&[TopologyAction::Remove {
            sponsor: "A",
            target: "B",
        }])
        .await;
    wait_for_no_delivery(&topology, "A", "B").await;
    topology.shutdown().await;
}

// G3：升级前遗留（或旧协议接收方）的无原因拒绝不靠清标志“试试看”：重启后仍保留；
// 没有重新认证证据或评估间隔未过时不重发；设备重新通过认证后评估一次并成功应用，投影随之清除。
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn g3_legacy_rejection_is_reevaluated_only_after_reauthentication_and_converges() {
    let _scenario = TestScenario::start();
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let mut topology = space_with_member_b(rendezvous.uri()).await;

    arm_dev(
        topology.engine("A"),
        DevOperation::ArmLegacyReceiverRejection { count: 1 },
    )
    .await;
    topology
        .run(&[TopologyAction::Join {
            sponsor: "A",
            joiner: "C",
        }])
        .await;
    topology.wait_for_equivalent_branch(&["A", "C"], 3).await;
    let legacy = wait_for_delivery(
        &topology,
        "A",
        "B",
        DeviceGroupUpdateStateSummary::RejectedUnclassified,
    )
    .await;
    assert_eq!(
        legacy.recovery,
        DeviceGroupUpdateRecoverySummary::AutomaticOnReconnect
    );
    wait_for_stable_phase(
        &topology,
        "A",
        SpaceDeviceUpdatePhaseSummary::NeedsAttention,
    )
    .await;

    // 发送方重启后拒绝仍在，评估间隔未过，不会因为重启证据而重发。
    topology.restart("A").await;
    topology.wait_for_connected_peer("A", "B").await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    let persisted = delivery_to(&topology, "A", "B")
        .await
        .expect("persisted rejection");
    assert_eq!(
        persisted.state,
        DeviceGroupUpdateStateSummary::RejectedUnclassified
    );
    assert_eq!(persisted.attempts, legacy.attempts);

    // 评估间隔（首次退避 30 秒）过后，设备重新通过认证才会评估；成功应用才清除。
    tokio::time::sleep(Duration::from_secs(30)).await;
    topology.restart("B").await;
    wait_for_no_delivery(&topology, "A", "B").await;
    wait_for_stable_phase(&topology, "A", SpaceDeviceUpdatePhaseSummary::Completed).await;

    let text = "g3 content from A after reevaluation";
    assert_eq!(topology.send("A", "B", text).await.total_accepted, 1);
    wait_for_received_text(topology.engine("B"), text).await;
    let reply = "g3 content from B after reevaluation";
    assert_eq!(topology.send("B", "A", reply).await.total_accepted, 1);
    wait_for_received_text(topology.engine("A"), reply).await;
    topology.shutdown().await;
}

// G4：多个收件人互不影响，同一收件人的多个 epoch 按顺序送达，暂时失败期间重试有界。
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn g4_healthy_recipients_are_unaffected_and_epochs_arrive_in_order() {
    let _scenario = TestScenario::start();
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let mut topology = space_with_members(
        rendezvous.uri(),
        &["A", "B", "C", "D", "E"],
        &["A", "B", "C"],
    )
    .await;

    let applied_before = applied_updates(topology.engine("B")).await;
    set_receive_fault(
        topology.engine("B"),
        Some(DevGroupUpdateReceiveFault::StorageUnavailable),
    )
    .await;
    for joiner in ["D", "E"] {
        topology
            .run(&[TopologyAction::Join {
                sponsor: "A",
                joiner,
            }])
            .await;
    }
    topology
        .wait_for_equivalent_branch(&["A", "D", "E"], 5)
        .await;

    // C 健康：两个 epoch 都已送达确认；B 的队首停在第一个 epoch。
    wait_for_no_delivery(&topology, "A", "C").await;
    assert!(applied_updates(topology.engine("C")).await >= 2);
    let waiting = wait_for_delivery(
        &topology,
        "A",
        "B",
        DeviceGroupUpdateStateSummary::ReceiverUnavailable,
    )
    .await;
    assert_eq!(
        waiting.recovery,
        DeviceGroupUpdateRecoverySummary::Automatic
    );
    assert!(waiting.next_retry_at_ms.is_some());
    // 成员变化会带来重新认证证据而加速重试；证据停止后，A 对 B 的持续失败只按退避重试，不会紧循环。
    let attempts_before = delivery_to(&topology, "A", "B")
        .await
        .expect("pending")
        .attempts;
    tokio::time::sleep(Duration::from_secs(25)).await;
    let attempts_after = delivery_to(&topology, "A", "B")
        .await
        .expect("pending")
        .attempts;
    assert!(
        attempts_after - attempts_before <= 1,
        "a persistent failure must follow the bounded backoff, saw {} retries in a quiet window",
        attempts_after - attempts_before
    );
    assert_eq!(applied_updates(topology.engine("B")).await, applied_before);

    set_receive_fault(topology.engine("B"), None).await;
    wait_for_no_delivery(&topology, "A", "B").await;
    wait_for_stable_phase(&topology, "A", SpaceDeviceUpdatePhaseSummary::Completed).await;
    let text = "g4 content to B after ordered recovery";
    assert_eq!(topology.send("A", "B", text).await.total_accepted, 1);
    wait_for_received_text(topology.engine("B"), text).await;
    topology.shutdown().await;
}
