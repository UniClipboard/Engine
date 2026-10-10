//! 组密钥更新投递：接收方的暂时失败与真实拒绝必须区分，暂时失败自动恢复，真实拒绝保持阻塞。
//!
//! 场景使用完整 Engine（真实 SQLite、MLS 与本机回环 QUIC）。接收方的暂时失败由 dev-tools 在接收端口
//! 注入；真实拒绝由发送方在线上篡改 MLS 提交字节，接收方走真实校验得到。

use super::*;

use uc_engine::{
    DevGroupUpdateReceiveFault, DevOperation, DevOperationResult, DevSpaceWorkEventKind,
    SpaceDeviceUpdatePhaseSummary,
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

/// A 创建空间，B 加入；随后 C 经 A 加入，A 必须把新 epoch 投递给已有成员 B。
async fn space_with_member_b(rendezvous_base_url: String) -> MembershipTopology {
    let mut topology = MembershipTopology::new(rendezvous_base_url);
    topology
        .run(&[
            TopologyAction::Start { node: "A" },
            TopologyAction::Start { node: "B" },
            TopologyAction::Start { node: "C" },
            TopologyAction::Create { node: "A" },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "B",
            },
        ])
        .await;
    topology.wait_for_equivalent_branch(&["A", "B"], 2).await;
    topology
}

// G1：接收方暂时不可用时，发送方延后重试而不是永久拒绝；故障解除后 B 自动应用该 epoch，内容双向可达。
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn g1_receiver_not_ready_defers_then_applies_after_recovery() {
    let _scenario = TestScenario::start();
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let mut topology = space_with_member_b(rendezvous.uri()).await;
    let baseline = topology.diagnostics("B").await.group_epoch;

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
        0,
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
