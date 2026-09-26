//! 计划 049 S3 的应用层多节点场景：W1 被移除方离线时的离开窗口，W2 三节点固定种子交错后的收敛。

use std::collections::BTreeSet;

use uc_core::ids::DeviceId;
use uc_core::membership::{LedgerMemberStatus, MemberInstanceId, PeerLink, DEPARTURE_WINDOW_MS};

use super::membership_nodes::{admit, VirtualMembershipNodes, VirtualNode};
use super::{member_facts, EstablishedSpace, TestSigner};
use crate::space::membership::{
    DeviceTrustChangeChoice, DeviceTrustMembership, MembershipRecord, SpaceDeviceUpdatePhase,
};

const START_MS: i64 = 1_000_000;

fn device(name: &str) -> DeviceId {
    DeviceId::new(name)
}

fn is_departing(node: &VirtualNode, peer: &DeviceId) -> bool {
    matches!(
        node.ledger().and_then(|ledger| ledger.peer(peer).cloned()),
        Some(PeerLink::Departing(_))
    )
}

// W1：被移除方离线时，移除方在离开窗口内保留一次通知责任；第 299999 毫秒仍存在，第 300000 毫秒结束。
#[tokio::test]
async fn w1_offline_removed_device_disappears_exactly_when_the_departure_window_ends() {
    let mut nodes = VirtualMembershipNodes::new(START_MS, 10_000);
    let space = EstablishedSpace::new(&["device-a", "device-b"]);
    let a = nodes.add_node(
        "node-a",
        space.record("device-a", 1),
        space.signer("device-a"),
    );
    let b = nodes.add_node(
        "node-b",
        space.record("device-b", 1),
        space.signer("device-b"),
    );
    assert!(nodes.settle(16).await, "two members settle");

    nodes.partition(a.label(), b.label());
    let removed_at = nodes.now_ms();
    a.remove(&device("device-b")).await.unwrap();
    a.run_worker().await;

    assert!(is_departing(&a, &device("device-b")));
    assert!(a
        .wake()
        .deadlines()
        .contains(&(removed_at + DEPARTURE_WINDOW_MS)));
    let status = a.status().await;
    assert_eq!(
        status.space_device_update.phase,
        SpaceDeviceUpdatePhase::Completed,
        "移除通知不阻塞设备更新"
    );

    nodes.clock().set(removed_at + DEPARTURE_WINDOW_MS - 1);
    a.run_worker().await;
    assert!(is_departing(&a, &device("device-b")));

    nodes.clock().set(removed_at + DEPARTURE_WINDOW_MS);
    a.run_worker().await;
    assert!(a.ledger().unwrap().peer(&device("device-b")).is_none());
    let status = a.status().await;
    assert!(status
        .devices
        .iter()
        .all(|listed| listed.device_id != device("device-b")));
    assert_eq!(
        status.space_device_update.phase,
        SpaceDeviceUpdatePhase::Completed
    );
}

/// 固定种子伪随机序列。
struct Seeded(u64);

impl Seeded {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }
}

const LABELS: [&str; 3] = ["node-a", "node-b", "node-c"];

struct Trio {
    nodes: VirtualMembershipNodes,
    admitted: BTreeSet<MemberInstanceId>,
    paired: bool,
    removed_b: bool,
}

impl Trio {
    fn new() -> Self {
        let mut nodes = VirtualMembershipNodes::new(START_MS, 1_000_000);
        let space = EstablishedSpace::new(&["device-a", "device-b"]);
        let (c_facts, c_credential) = member_facts("device-c", 0x53);
        nodes.add_node(
            "node-a",
            space.record("device-a", 1),
            space.signer("device-a"),
        );
        nodes.add_node(
            "node-b",
            space.record("device-b", 1),
            space.signer("device-b"),
        );
        nodes.add_node(
            "node-c",
            MembershipRecord::NoSpace { revision: 0 },
            TestSigner {
                local_device_id: c_facts.device_id,
                local_member: c_facts.member_instance,
                credential: c_credential,
            },
        );
        let admitted = [space.member("device-a"), space.member("device-b")]
            .into_iter()
            .collect();
        Self {
            nodes,
            admitted,
            paired: false,
            removed_b: false,
        }
    }

    fn node(&self, index: usize) -> &VirtualNode {
        &self.nodes.nodes()[index]
    }

    async fn step(&mut self, random: &mut Seeded) {
        match random.below(9) {
            0 if !self.paired => {
                let member = admit(self.node(0), self.node(2), 0x53, 0x71).await;
                self.admitted.insert(member);
                self.paired = true;
            }
            1 if !self.removed_b => {
                if self.node(0).remove(&device("device-b")).await.is_ok() {
                    self.removed_b = true;
                }
            }
            2 => {
                let index = 1 + random.below(2) as usize;
                if let Some(change) = self.node(index).pending_decision() {
                    let choice = if random.below(3) == 0 {
                        DeviceTrustChangeChoice::KeepCurrentDeviceGroup
                    } else {
                        DeviceTrustChangeChoice::ApplyChange
                    };
                    let _ = self.node(index).decide(change, choice).await;
                }
            }
            3 => {
                let left = random.below(3) as usize;
                let right = (left + 1 + random.below(2) as usize) % 3;
                self.nodes.partition(LABELS[left], LABELS[right]);
            }
            4 => {
                let left = random.below(3) as usize;
                let right = (left + 1 + random.below(2) as usize) % 3;
                self.nodes.heal(LABELS[left], LABELS[right]);
            }
            5 => self.node(random.below(3) as usize).restart(),
            6 => self.nodes.clock().advance(random.below(120_000) as i64),
            _ => {
                self.node(random.below(3) as usize).run_worker().await;
            }
        }
    }

    /// 网络恢复后反复：运行到静止、补齐全部待决定的移除、越过一个离开窗口（不短于最长同步退避），
    /// 直到一整轮既没有新决定也没有业务状态变化。同步退避计数不计入：错过离开窗口的被移除方
    /// 会按上限退避持续重试，这是 ADR 接受的结果。
    async fn converge(&mut self, random: &mut Seeded) {
        self.nodes.heal_all();
        for _ in 0..16 {
            let before = self.business_state();
            assert!(self.nodes.settle(64).await, "nodes settle after healing");
            let mut decided = false;
            for index in 0..3 {
                if let Some(change) = self.node(index).pending_decision() {
                    let choice = if random.below(3) == 0 {
                        DeviceTrustChangeChoice::KeepCurrentDeviceGroup
                    } else {
                        DeviceTrustChangeChoice::ApplyChange
                    };
                    self.node(index).decide(change, choice).await.unwrap();
                    decided = true;
                }
            }
            self.nodes.clock().advance(DEPARTURE_WINDOW_MS);
            assert!(self.nodes.settle(64).await, "nodes settle after the window");
            let after = self.business_state();
            if !decided && before == after {
                return;
            }
        }
        panic!(
            "the three nodes did not converge: {:#?}",
            self.business_state()
        );
    }

    /// 各节点不含同步退避簿记的业务状态。
    fn business_state(&self) -> Vec<String> {
        self.nodes
            .nodes()
            .iter()
            .map(|node| {
                let Some(ledger) = node.ledger() else {
                    return format!("{:?}", node.record());
                };
                let peers: Vec<_> = ledger
                    .peers()
                    .map(|(peer, link)| match link {
                        PeerLink::Member(member) => format!(
                            "{peer:?}:{:?}:{:?}:{:?}",
                            member.relation(),
                            member.confirmed_position(),
                            member.outgoing_decision()
                        ),
                        PeerLink::Departing(_) => format!("{peer:?}:departing"),
                    })
                    .collect();
                format!(
                    "{:?}|{:?}|{peers:?}|{:?}",
                    ledger.local_status(),
                    ledger.history().current_head(),
                    ledger.unfinished_effects().collect::<Vec<_>>()
                )
            })
            .collect()
    }

    async fn assert_converged(&self, seed: u64) {
        for node in self.nodes.nodes() {
            let Some(ledger) = node.ledger() else {
                continue;
            };
            let status = node.status().await;
            // 任何一方不永久停留在更新中。
            assert_ne!(
                status.space_device_update.phase,
                SpaceDeviceUpdatePhase::Updating,
                "seed {seed}: {} stays updating",
                node.label()
            );
            // 移除方不残留设备：离开窗口过后只列出本机与当前成员。
            let effective: BTreeSet<_> = ledger
                .history()
                .effective_members()
                .into_iter()
                .filter_map(|member| ledger.history().admission_facts_for(member))
                .map(|facts| facts.device_id)
                .collect();
            assert!(
                ledger
                    .peers()
                    .all(|(_, link)| matches!(link, PeerLink::Member(_))),
                "seed {seed}: {} keeps a departing device after the window",
                node.label()
            );
            for listed in &status.devices {
                assert!(
                    listed.is_local || effective.contains(&listed.device_id),
                    "seed {seed}: {} lists a device outside its members",
                    node.label()
                );
            }
            // 被移除方进入已移除终态。
            if ledger.local_status() == LedgerMemberStatus::Removed {
                assert_eq!(status.local_membership, DeviceTrustMembership::Removed);
                assert_eq!(
                    status.space_device_update.phase,
                    SpaceDeviceUpdatePhase::Completed,
                    "seed {seed}: removed {} is not completed",
                    node.label()
                );
            }
            // 资格从不在缺少签名事件时扩大：有效成员只来自基线或已签名的加入。
            for member in ledger.history().active_members() {
                assert!(
                    self.admitted.contains(&member),
                    "seed {seed}: {} activated an unknown member",
                    node.label()
                );
            }
        }
    }
}

// W2：三节点在配对、移除、延迟决定、离线与重启任意交错后收敛。
#[tokio::test]
async fn w2_seeded_three_node_interleavings_converge() {
    for seed in 1..=24u64 {
        let mut random = Seeded(0x9e37_79b9_7f4a_7c15 ^ seed.wrapping_mul(0x1000_0001));
        let mut trio = Trio::new();
        for _ in 0..48 {
            trio.step(&mut random).await;
        }
        trio.converge(&mut random).await;
        trio.assert_converged(seed).await;
    }
}
