//! 计划 049 S0：移除相关的双端收敛行为固定。
//!
//! 场景只使用公开操作和查询，重写内部实现后原样保留；断言语义不得为通过而修改。R1–R3 在 S0 时
//! 标注为已知无法收敛，S3 切换到单一成员状态负责人后取消标注。

use super::*;

use uc_engine::{
    DeviceGroupRelationshipSummary, DeviceMembershipSummary, DeviceSyncRelationshipSummary,
    DeviceTrustSnapshotSummary, SpaceDeviceUpdatePhaseSummary,
};

/// 移除通知送达后，移除方最多等待这么久删除被移除设备；远小于 5 分钟离线收尾窗口。
const REMOVAL_SETTLE_TIMEOUT: Duration = Duration::from_secs(60);
/// 稳定结论需要连续观察的次数与间隔，避免把一次瞬时读数当成最终状态。
const STABLE_OBSERVATIONS: usize = 5;
const STABLE_OBSERVATION_INTERVAL: Duration = Duration::from_millis(400);
/// 被移除方晚于移除方撤销其身份才作决定：通知送达后移除方同一轮维护即撤销身份，这里留足余量。
const LATE_DECISION_DELAY: Duration = Duration::from_secs(3);

// R1：A 移除 B，移除通知送达后 A 的设备列表不再包含 B，且设备更新完成。
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn r1_sponsor_drops_removed_device_after_notice_is_delivered() {
    let _scenario = TestScenario::start();
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let mut topology = paired_pair(rendezvous.uri()).await;
    let removed = device_id(&topology, "B");

    remove_and_wait_for_notice(&mut topology).await;

    wait_until_device_absent(&topology, "A", &removed).await;
    wait_for_stable_update_phase(&topology, "A", SpaceDeviceUpdatePhaseSummary::Completed).await;
    topology.shutdown().await;
}

// R2：B 在 A 撤销其身份之后才接受移除（2026-09-23 双 Desktop 现场）。
// B 进入本机已移除终态且设备更新完成；A 不残留 B。
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn r2_late_local_removal_acceptance_reaches_removed_terminal_state() {
    let _scenario = TestScenario::start();
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let mut topology = paired_pair(rendezvous.uri()).await;
    let removed = device_id(&topology, "B");

    remove_and_wait_for_notice(&mut topology).await;
    tokio::time::sleep(LATE_DECISION_DELAY).await;
    topology.apply_local_removal_with_confirmation("B").await;

    wait_for_removed_local_device(&topology, "B").await;
    wait_for_stable_update_phase(&topology, "B", SpaceDeviceUpdatePhaseSummary::Completed).await;
    wait_until_device_absent(&topology, "A", &removed).await;
    wait_for_stable_update_phase(&topology, "A", SpaceDeviceUpdatePhaseSummary::Completed).await;
    topology.shutdown().await;
}

// R3：B 拒绝移除。B 保留本机分支并把 A 标为分叉，稳定停在需要处理而不是更新中；A 不残留 B。
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn r3_rejected_removal_settles_both_sides() {
    let _scenario = TestScenario::start();
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let mut topology = paired_pair(rendezvous.uri()).await;
    let removed = device_id(&topology, "B");
    let sponsor = device_id(&topology, "A");

    remove_and_wait_for_notice(&mut topology).await;
    tokio::time::sleep(LATE_DECISION_DELAY).await;
    topology
        .run(&[TopologyAction::Decide {
            node: "B",
            choice: PendingChangeChoice::Keep,
        }])
        .await;

    wait_for_peer_relationship(
        &topology,
        "B",
        &sponsor,
        DeviceGroupRelationshipSummary::Diverged,
        DeviceSyncRelationshipSummary::PausedGroupDiverged,
    )
    .await;
    wait_for_stable_update_phase(
        &topology,
        "B",
        SpaceDeviceUpdatePhaseSummary::NeedsAttention,
    )
    .await;
    wait_until_device_absent(&topology, "A", &removed).await;
    wait_for_stable_update_phase(&topology, "A", SpaceDeviceUpdatePhaseSummary::Completed).await;
    topology.shutdown().await;
}

// R4：B 接受移除后由 A 重新邀请，以新资格恢复双向可用与内容收发。
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn r4_removed_device_rejoins_and_becomes_usable_again() {
    let _scenario = TestScenario::start();
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let mut topology = paired_pair(rendezvous.uri()).await;

    remove_and_wait_for_notice(&mut topology).await;
    topology.apply_local_removal_with_confirmation("B").await;
    wait_for_removed_local_device(&topology, "B").await;

    topology
        .run(&[TopologyAction::Join {
            sponsor: "A",
            joiner: "B",
        }])
        .await;
    topology.wait_for_equivalent_branch(&["A", "B"], 2).await;
    topology.wait_for_connected_peer("A", "B").await;
    let text = "049 R4 rejoined device receives content";
    assert_eq!(topology.send("A", "B", text).await.total_accepted, 1);
    wait_for_received_text(topology.engine("B"), text).await;
    topology.shutdown().await;
}

async fn paired_pair(rendezvous_base_url: String) -> MembershipTopology {
    let mut topology = MembershipTopology::new(rendezvous_base_url);
    topology
        .run(&[
            TopologyAction::Start { node: "A" },
            TopologyAction::Start { node: "B" },
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

/// A 提交移除，并以 B 出现待决定移除作为“移除通知已送达”的公开证据。
async fn remove_and_wait_for_notice(topology: &mut MembershipTopology) {
    topology
        .run(&[TopologyAction::Remove {
            sponsor: "A",
            target: "B",
        }])
        .await;
    topology.wait_for_pending_change(&["B"]).await;
}

fn device_id(topology: &MembershipTopology, node: &str) -> String {
    topology
        .device_ids
        .get(node)
        .unwrap_or_else(|| panic!("node {node} has no device id"))
        .clone()
}

async fn device_trust(topology: &MembershipTopology, node: &str) -> DeviceTrustSnapshotSummary {
    topology.device_group_choices(node).await.device_trust
}

async fn wait_until_device_absent(topology: &MembershipTopology, node: &str, device_id: &str) {
    let deadline = tokio::time::Instant::now() + REMOVAL_SETTLE_TIMEOUT;
    loop {
        let snapshot = device_trust(topology, node).await;
        let Some(device) = snapshot
            .devices
            .iter()
            .find(|device| device.device_id == device_id)
        else {
            return;
        };
        assert!(
            tokio::time::Instant::now() < deadline,
            "node {node} still lists the removed device: membership={:?}, relationship={:?}, sync={:?}",
            device.membership,
            device.group_relationship,
            device.sync_relationship
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

async fn wait_for_removed_local_device(topology: &MembershipTopology, node: &str) {
    let deadline = tokio::time::Instant::now() + REMOVAL_SETTLE_TIMEOUT;
    loop {
        let snapshot = device_trust(topology, node).await;
        let local_sync = snapshot
            .devices
            .iter()
            .find(|device| device.is_local)
            .map(|device| device.sync_relationship);
        if snapshot.local_membership == DeviceMembershipSummary::Removed
            && local_sync == Some(DeviceSyncRelationshipSummary::RemovedLocalDevice)
        {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "node {node} did not reach the removed local device state: membership={:?}, local sync={local_sync:?}",
            snapshot.local_membership
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

async fn wait_for_peer_relationship(
    topology: &MembershipTopology,
    node: &str,
    peer_device_id: &str,
    relationship: DeviceGroupRelationshipSummary,
    sync: DeviceSyncRelationshipSummary,
) {
    let deadline = tokio::time::Instant::now() + REMOVAL_SETTLE_TIMEOUT;
    loop {
        let snapshot = device_trust(topology, node).await;
        let observed = snapshot
            .devices
            .iter()
            .find(|device| device.device_id == peer_device_id)
            .map(|device| (device.group_relationship, device.sync_relationship));
        if observed == Some((relationship, sync)) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "node {node} did not reach {relationship:?}/{sync:?} for the peer; observed {observed:?}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// 设备更新阶段必须到达期望值，并在连续观察中保持不变。
async fn wait_for_stable_update_phase(
    topology: &MembershipTopology,
    node: &str,
    expected: SpaceDeviceUpdatePhaseSummary,
) {
    let deadline = tokio::time::Instant::now() + REMOVAL_SETTLE_TIMEOUT;
    let mut stable = 0;
    loop {
        let phase = device_trust(topology, node).await.space_device_update.phase;
        if phase == expected {
            stable += 1;
            if stable == STABLE_OBSERVATIONS {
                return;
            }
        } else {
            stable = 0;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "node {node} space device update did not settle at {expected:?}; last phase {phase:?}"
        );
        tokio::time::sleep(STABLE_OBSERVATION_INTERVAL).await;
    }
}
