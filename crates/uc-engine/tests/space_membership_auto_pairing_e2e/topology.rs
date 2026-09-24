//! 多节点成员拓扑：F0–F7 分叉、交叉移除、交接预览与离线成员。

use super::*;

// 三台设备依次加入、移除、重新加入后发生交叉移除时，至少一台设备必须报告设备组分歧。
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn cross_removals_after_rejoin_must_keep_divergence_visible() {
    let _scenario = TestScenario::start();
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let mut topology = MembershipTopology::new(rendezvous.uri());
    topology
        .run(&[
            TopologyAction::Start { node: "A" },
            TopologyAction::Start { node: "B" },
            TopologyAction::Start { node: "C" },
            TopologyAction::Create { node: "A" },
            TopologyAction::Create { node: "B" },
            TopologyAction::Create { node: "C" },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "B",
            },
            TopologyAction::Join {
                sponsor: "B",
                joiner: "C",
            },
        ])
        .await;
    topology
        .wait_for_equivalent_branch_named(&["A", "B", "C"], 3, "initial A-B-C group")
        .await;

    topology
        .run(&[TopologyAction::Remove {
            sponsor: "C",
            target: "A",
        }])
        .await;
    topology.wait_for_pending_change(&["B"]).await;
    topology
        .run(&[TopologyAction::Decide {
            node: "B",
            choice: PendingChangeChoice::Apply,
        }])
        .await;
    topology.wait_for_pending_change(&["A"]).await;
    topology.apply_local_removal_with_confirmation("A").await;
    topology
        .wait_for_equivalent_branch_named(&["B", "C"], 2, "A removed from the first group")
        .await;

    topology
        .run(&[TopologyAction::Join {
            sponsor: "B",
            joiner: "A",
        }])
        .await;
    wait_for_active_member_count(topology.engine("A"), 3).await;

    topology
        .run(&[TopologyAction::Remove {
            sponsor: "C",
            target: "B",
        }])
        .await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    topology
        .run(&[TopologyAction::Remove {
            sponsor: "A",
            target: "C",
        }])
        .await;
    topology
        .wait_for_stable_pending_effects(&["A", "B", "C"])
        .await;

    let final_diagnostics = [
        topology.diagnostics("A").await,
        topology.diagnostics("B").await,
        topology.diagnostics("C").await,
    ];
    let effective_members = final_diagnostics
        .each_ref()
        .map(|diagnostics| diagnostics.effective_member_count);
    let pending_conflicts = final_diagnostics
        .each_ref()
        .map(|diagnostics| diagnostics.pending_conflict_count);
    let pending_confirmations = final_diagnostics
        .each_ref()
        .map(|diagnostics| diagnostics.pending_confirmation_count);
    assert!(
        pending_conflicts
            .iter()
            .chain(pending_confirmations.iter())
            .any(|count| *count > 0),
        "cross-removal split must remain visible instead of reporting every device group as healthy; effective members: {effective_members:?}; pending conflicts: {pending_conflicts:?}; pending confirmations: {pending_confirmations:?}"
    );

    topology.shutdown().await;
}

// F7：十节点不平衡树形成三个 sibling 后，冲突 peer 不得饿死合法 peer 的反熵。
#[tokio::test(flavor = "multi_thread", worker_threads = 16)]
async fn f7_three_sibling_branches_keep_fair_anti_entropy_for_legal_peers() {
    let _scenario = TestScenario::start();
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let mut topology = MembershipTopology::new(rendezvous.uri());
    topology
        .run(&[
            TopologyAction::Start { node: "A" },
            TopologyAction::Start { node: "B" },
            TopologyAction::Start { node: "C" },
            TopologyAction::Start { node: "D" },
            TopologyAction::Start { node: "E" },
            TopologyAction::Start { node: "F" },
            TopologyAction::Start { node: "G" },
            TopologyAction::Start { node: "H" },
            TopologyAction::Start { node: "I" },
            TopologyAction::Start { node: "J" },
            TopologyAction::Create { node: "A" },
        ])
        .await;
    let mut baseline = vec!["A"];
    for (sponsor, joiner) in [
        ("A", "B"),
        ("B", "C"),
        ("C", "D"),
        ("D", "E"),
        ("E", "F"),
        ("F", "G"),
    ] {
        topology.join(sponsor, joiner).await;
        baseline.push(joiner);
        topology
            .wait_for_equivalent_branch_named(&baseline, baseline.len() as u32, "F7 baseline")
            .await;
        let epoch = topology.diagnostics("A").await.group_epoch;
        topology
            .wait_for_group_epoch_named(&baseline, epoch, "F7 baseline")
            .await;
    }
    let invitation_h = issue_invitation_named(topology.engine("A"), "A").await;
    let invitation_i = issue_invitation_named(topology.engine("B"), "B").await;
    let invitation_j = issue_invitation_named(topology.engine("C"), "C").await;

    topology
        .run(&[TopologyAction::PartitionGroups {
            groups: &[
                &["A", "G", "H"][..],
                &["D"][..],
                &["B", "E", "I"][..],
                &["C", "F", "J"][..],
            ],
        }])
        .await;
    let space_id = topology
        .space_ids
        .get("A")
        .unwrap_or_else(|| panic!("node A has no space"))
        .clone();
    let (joined_h, joined_i, joined_j) = tokio::join!(
        join_with_invitation(topology.engine("H"), "H", &space_id, invitation_h,),
        join_with_invitation(topology.engine("I"), "I", &space_id, invitation_i,),
        join_with_invitation(topology.engine("J"), "J", &space_id, invitation_j,),
    );
    for (node, joined) in [("H", joined_h), ("I", joined_i), ("J", joined_j)] {
        topology.space_ids.insert(node.to_owned(), space_id.clone());
        topology
            .device_ids
            .insert(node.to_owned(), joined.self_device_id);
    }
    for (nodes, phase) in [
        (&["A", "G", "H"][..], "F7 branch H"),
        (&["B", "E", "I"][..], "F7 branch I"),
        (&["C", "F", "J"][..], "F7 branch J"),
    ] {
        topology
            .wait_for_equivalent_branch_named(nodes, 8, phase)
            .await;
        let epoch = topology.diagnostics(nodes[0]).await.group_epoch;
        topology
            .wait_for_group_epoch_named(nodes, epoch, phase)
            .await;
    }
    let target = topology.diagnostics("A").await;
    assert_eq!(topology.diagnostics("D").await.effective_member_count, 7);

    topology
        .run(&[TopologyAction::GroupedBridge {
            groups: &[
                &["A", "D", "G", "H"][..],
                &["B", "E", "I"][..],
                &["C", "F", "J"][..],
            ],
            left: "A",
            right: "B",
        }])
        .await;
    topology.wait_for_branch_conflict(&["A", "B"]).await;
    topology
        .wait_for_equivalent_branch_named(&["A", "D", "G", "H"], 8, "F7 fair legal peer")
        .await;
    topology
        .wait_for_group_epoch_named(
            &["A", "D", "G", "H"],
            target.group_epoch,
            "F7 fair legal peer",
        )
        .await;
    assert_eq!(topology.diagnostics("D").await.pending_conflict_count, 0);

    let branches: [&[&str]; 3] = [&["A", "D", "G", "H"], &["B", "E", "I"], &["C", "F", "J"]];
    let all_nodes = ["A", "B", "C", "D", "E", "F", "G", "H", "I", "J"];
    for branch in branches {
        for sender in branch {
            for receiver in branch {
                if sender != receiver {
                    topology.wait_for_connected_peer(sender, receiver).await;
                }
            }
        }
    }
    for sender in all_nodes {
        for receiver in all_nodes {
            if sender == receiver {
                continue;
            }
            let same_branch = branches
                .iter()
                .any(|branch| branch.contains(&sender) && branch.contains(&receiver));
            let text = format!("F7 matrix {sender}-{receiver}");
            let report = topology.send(sender, receiver, &text).await;
            assert_eq!(
                report.total_accepted + report.total_pending,
                usize::from(same_branch),
                "unexpected F7 transfer result for {sender}-{receiver}: {report:?}"
            );
            if same_branch {
                wait_for_received_text(topology.engine(receiver), &text).await;
            } else {
                assert!(!receiver_has_exact_text(topology.engine(receiver), &text).await);
            }
        }
    }
    topology.shutdown().await;
}

// F6：深准入链的中间 Sponsor 离线后，叶子仍须经剩余相邻节点恢复到共同分支。
#[tokio::test(flavor = "multi_thread", worker_threads = 12)]
async fn f6_deep_chain_recovers_selected_branch_without_online_sponsors() {
    let _scenario = TestScenario::start();
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let mut topology = MembershipTopology::new(rendezvous.uri());
    topology
        .run(&[
            TopologyAction::Start { node: "A" },
            TopologyAction::Start { node: "B" },
            TopologyAction::Start { node: "C" },
            TopologyAction::Start { node: "D" },
            TopologyAction::Start { node: "E" },
            TopologyAction::Start { node: "F" },
            TopologyAction::Start { node: "G" },
            TopologyAction::Start { node: "H" },
            TopologyAction::Create { node: "A" },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "B",
            },
            TopologyAction::Join {
                sponsor: "B",
                joiner: "C",
            },
            TopologyAction::Join {
                sponsor: "C",
                joiner: "D",
            },
            TopologyAction::Join {
                sponsor: "D",
                joiner: "E",
            },
            TopologyAction::Join {
                sponsor: "E",
                joiner: "F",
            },
        ])
        .await;
    topology
        .wait_for_equivalent_branch_named(&["A", "B", "C", "D", "E", "F"], 6, "F6 common baseline")
        .await;
    let baseline_epoch = topology.diagnostics("A").await.group_epoch;
    topology
        .wait_for_group_epoch(&["B", "C", "D", "E", "F"], baseline_epoch)
        .await;
    let left_invitation = issue_invitation(topology.engine("A")).await;
    let right_invitation = issue_invitation(topology.engine("F")).await;

    topology
        .run(&[TopologyAction::Partition {
            left: &["A", "C", "E", "G"],
            right: &["B", "D", "F", "H"],
        }])
        .await;
    topology
        .join_with_invitation("A", "G", left_invitation)
        .await;
    topology
        .join_with_invitation("F", "H", right_invitation)
        .await;
    topology
        .wait_for_equivalent_branch_named(&["A", "C", "E", "G"], 7, "F6 selected left branch")
        .await;
    topology
        .wait_for_equivalent_branch_named(&["F", "H"], 7, "F6 sibling right branch")
        .await;
    let target_epoch = topology.diagnostics("A").await.group_epoch;
    topology
        .wait_for_group_epoch(&["C", "E", "G"], target_epoch)
        .await;
    let sibling_epoch = topology.diagnostics("F").await.group_epoch;
    topology
        .wait_for_group_epoch(&["B", "D", "H"], sibling_epoch)
        .await;
    let target = topology.diagnostics("E").await;

    topology
        .run(&[
            TopologyAction::Stop { node: "B" },
            TopologyAction::Stop { node: "D" },
            TopologyAction::Chain {
                nodes: &["A", "C", "E", "F"],
                offline: &["B", "D", "G", "H"],
            },
        ])
        .await;
    topology.wait_for_branch_conflict(&["E", "F"]).await;
    topology
        .run(&[TopologyAction::ResolveConflict {
            node: "F",
            branch_from: "E",
        }])
        .await;
    topology
        .wait_for_equivalent_branch_named(&["A", "C", "E", "F"], 7, "F6 recovered online chain")
        .await;
    topology
        .run(&[TopologyAction::Heal {
            nodes: &["A", "C", "E", "F"],
        }])
        .await;
    let recovered_epoch = topology.diagnostics("E").await.group_epoch;
    topology
        .wait_for_group_epoch_named(
            &["A", "C", "E", "F"],
            recovered_epoch,
            "F6 recovered online chain",
        )
        .await;
    for node in ["A", "C", "E", "F"] {
        refresh_available_members(topology.engine(node)).await;
    }
    let recovered = topology.diagnostics("F").await;
    assert_eq!(recovered.branch_id, target.branch_id);
    assert_eq!(recovered.head_event_id, target.head_event_id);

    for (sender, receiver) in [("A", "C"), ("C", "E"), ("E", "F")] {
        topology.wait_for_connected_peer(sender, receiver).await;
        let text = format!("F6 converged hop {sender}-{receiver}");
        let report = topology.send(sender, receiver, &text).await;
        assert!(
            report.total_accepted > 0,
            "F6 converged hop {sender}-{receiver} was rejected: {report:?}"
        );
        assert!(receiver_has_exact_text(topology.engine(receiver), &text).await);
    }
    topology.shutdown().await;
}

// F5：同一 sibling conflict 沿四节点环的两个方向传播时，只能提示一次且不得形成消息环。
#[tokio::test(flavor = "multi_thread", worker_threads = 12)]
async fn f5_ring_propagates_one_conflict_without_message_or_effect_loops() {
    let _scenario = TestScenario::start();
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let mut topology = MembershipTopology::new(rendezvous.uri());
    topology
        .run(&[
            TopologyAction::Start { node: "A" },
            TopologyAction::Start { node: "B" },
            TopologyAction::Start { node: "C" },
            TopologyAction::Start { node: "D" },
            TopologyAction::Start { node: "E" },
            TopologyAction::Start { node: "F" },
            TopologyAction::Create { node: "A" },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "B",
            },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "C",
            },
        ])
        .await;
    topology
        .wait_for_equivalent_branch(&["A", "B", "C"], 3)
        .await;
    let epoch_three = topology.diagnostics("A").await.group_epoch;
    topology
        .wait_for_group_epoch(&["B", "C"], epoch_three)
        .await;
    topology
        .run(&[TopologyAction::Join {
            sponsor: "A",
            joiner: "D",
        }])
        .await;
    topology
        .wait_for_equivalent_branch(&["A", "B", "C", "D"], 4)
        .await;
    let epoch_four = topology.diagnostics("A").await.group_epoch;
    topology
        .wait_for_group_epoch(&["B", "C", "D"], epoch_four)
        .await;
    topology
        .run(&[TopologyAction::Partition {
            left: &["A", "B", "E"],
            right: &["C", "D", "F"],
        }])
        .await;
    topology
        .run(&[
            TopologyAction::Join {
                sponsor: "A",
                joiner: "E",
            },
            TopologyAction::Join {
                sponsor: "C",
                joiner: "F",
            },
        ])
        .await;
    topology
        .wait_for_equivalent_branch(&["A", "B", "E"], 5)
        .await;
    topology
        .wait_for_equivalent_branch(&["C", "D", "F"], 5)
        .await;
    let left_branch = topology.diagnostics("A").await.branch_id;
    let right_branch = topology.diagnostics("C").await.branch_id;
    assert_ne!(left_branch, right_branch);
    let effects_before_ring = topology
        .wait_for_stable_pending_effects(&["A", "B", "C", "D"])
        .await;

    topology
        .run(&[TopologyAction::Ring {
            nodes: &["A", "B", "C", "D"],
            isolated: &["E", "F"],
        }])
        .await;
    topology
        .wait_for_branch_conflict(&["A", "B", "C", "D"])
        .await;

    for (index, node) in ["A", "B", "C", "D"].into_iter().enumerate() {
        let choices = topology.device_group_choices(node).await;
        assert_eq!(
            choices
                .issues
                .iter()
                .filter(|issue| issue.issue_id.starts_with("c:"))
                .count(),
            1,
            "node {node} must expose one conflict prompt"
        );
        assert!(
            topology.diagnostics(node).await.pending_effect_count <= effects_before_ring[index],
            "node {node} must not enqueue an effect while propagating conflict evidence"
        );
    }
    assert_eq!(topology.diagnostics("A").await.branch_id, left_branch);
    assert_eq!(topology.diagnostics("B").await.branch_id, left_branch);
    assert_eq!(topology.diagnostics("C").await.branch_id, right_branch);
    assert_eq!(topology.diagnostics("D").await.branch_id, right_branch);

    let effects_before_refresh = topology
        .wait_for_stable_pending_effects(&["A", "B", "C", "D"])
        .await;
    for _ in 0..2 {
        for node in ["A", "B", "C", "D"] {
            refresh_available_members(topology.engine(node)).await;
        }
    }
    topology
        .wait_for_branch_conflict(&["A", "B", "C", "D"])
        .await;
    let effects_after_refresh = topology
        .wait_for_stable_pending_effects(&["A", "B", "C", "D"])
        .await;
    assert_eq!(effects_after_refresh, effects_before_refresh);
    topology.shutdown().await;
}

// F4：两个三节点 sibling 分支只开放一条 bridge 后，不得被拼成六节点联合历史。
#[tokio::test(flavor = "multi_thread", worker_threads = 12)]
async fn f4_single_bridge_cannot_splice_sibling_histories_into_a_union() {
    let _scenario = TestScenario::start();
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let mut topology = MembershipTopology::new(rendezvous.uri());
    topology
        .run(&[
            TopologyAction::Start { node: "A" },
            TopologyAction::Start { node: "B" },
            TopologyAction::Start { node: "C" },
            TopologyAction::Start { node: "D" },
            TopologyAction::Start { node: "E" },
            TopologyAction::Start { node: "F" },
            TopologyAction::Create { node: "A" },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "B",
            },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "C",
            },
        ])
        .await;
    topology
        .wait_for_equivalent_branch(&["A", "B", "C"], 3)
        .await;
    let epoch_three = topology.diagnostics("A").await.group_epoch;
    topology
        .wait_for_group_epoch(&["B", "C"], epoch_three)
        .await;
    for (joiner, established) in [
        ("D", &["A", "B", "C", "D"][..]),
        ("E", &["A", "B", "C", "D", "E"][..]),
        ("F", &["A", "B", "C", "D", "E", "F"][..]),
    ] {
        topology
            .run(&[TopologyAction::Join {
                sponsor: "A",
                joiner,
            }])
            .await;
        topology
            .wait_for_equivalent_branch(established, established.len() as u32)
            .await;
        let epoch = topology.diagnostics("A").await.group_epoch;
        topology
            .wait_for_group_epoch(&established[1..], epoch)
            .await;
    }
    topology
        .wait_for_equivalent_branch(&["A", "B", "C", "D", "E", "F"], 6)
        .await;
    let baseline_epoch = topology.diagnostics("A").await.group_epoch;
    topology
        .wait_for_group_epoch(&["B", "C", "D", "E", "F"], baseline_epoch)
        .await;
    topology
        .run(&[TopologyAction::Partition {
            left: &["A", "B", "C"],
            right: &["D", "E", "F"],
        }])
        .await;

    for target in ["C", "E", "F"] {
        topology
            .run(&[TopologyAction::Remove {
                sponsor: "A",
                target,
            }])
            .await;
    }
    for target in ["B", "C", "F"] {
        topology
            .run(&[TopologyAction::Remove {
                sponsor: "D",
                target,
            }])
            .await;
    }

    topology.assert_snapshot("A", 3, 0).await;
    topology.assert_snapshot("D", 3, 0).await;
    let left_before = topology.diagnostics("A").await;
    let right_before = topology.diagnostics("D").await;
    assert_ne!(left_before.branch_id, right_before.branch_id);

    topology
        .run(&[TopologyAction::Bridge {
            left: "A",
            right: "D",
            left_group: &["A", "B", "C"],
            right_group: &["D", "E", "F"],
        }])
        .await;
    topology.wait_for_branch_conflict(&["A", "D"]).await;

    topology.assert_snapshot("A", 3, 1).await;
    topology.assert_snapshot("D", 3, 1).await;
    assert_eq!(
        topology.diagnostics("A").await.branch_id,
        left_before.branch_id
    );
    assert_eq!(
        topology.diagnostics("D").await.branch_id,
        right_before.branch_id
    );
    let bridge_text = "F4 sibling bridge must not carry content";
    assert_eq!(topology.send("A", "D", bridge_text).await.total_accepted, 0);
    assert!(!receiver_has_exact_text(topology.engine("D"), bridge_text).await);
    topology.shutdown().await;
}

// 交接复现：真实四实例分别接受、保留，最后对照事前预览和事后成员。
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn handoff_four_device_removal_preview_matches_executed_choice() {
    let _scenario = TestScenario::start();
    let rendezvous = mount_rendezvous().await;
    let mut topology = MembershipTopology::new(rendezvous.uri());
    topology
        .run(&[
            TopologyAction::Start { node: "A" },
            TopologyAction::Start { node: "B" },
            TopologyAction::Start { node: "C" },
            TopologyAction::Start { node: "D" },
            TopologyAction::Create { node: "A" },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "B",
            },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "C",
            },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "D",
            },
        ])
        .await;
    topology
        .wait_for_equivalent_branch(&["A", "B", "C", "D"], 4)
        .await;
    let epoch = topology.diagnostics("A").await.group_epoch;
    topology.wait_for_group_epoch(&["B", "C", "D"], epoch).await;
    topology
        .run(&[TopologyAction::Remove {
            sponsor: "B",
            target: "C",
        }])
        .await;
    topology.wait_for_pending_change(&["A", "C", "D"]).await;
    let apply_preview = topology.device_group_choices("A").await;
    let keep_preview = topology.device_group_choices("D").await;
    let pending = apply_preview.issues.first().unwrap();
    assert_eq!(
        pending.reason.kind,
        uc_engine::DeviceGroupChoiceReasonKind::PendingRemoval
    );
    assert_eq!(pending.reason.changes.len(), 1);
    let removed = &pending.reason.changes[0].target.device_id;
    let candidate = pending
        .choices
        .iter()
        .find(|choice| !choice.is_current_group)
        .unwrap();
    assert!(candidate.members_complete);
    assert_eq!(candidate.members.len(), 3);
    assert!(candidate
        .members
        .iter()
        .all(|member| !member.display_name.is_empty() && &member.device_id != removed));
    assert!(!candidate
        .impact
        .as_ref()
        .unwrap()
        .sync_scope_device_ids
        .contains(removed));
    topology
        .decide_pending_change("A", PendingChangeChoice::Apply)
        .await;
    topology
        .decide_pending_change("D", PendingChangeChoice::Keep)
        .await;
    let applied = topology.device_group_choices("A").await;
    let kept = topology.device_group_choices("D").await;
    let applied_diagnostics = topology.diagnostics("A").await;
    let kept_diagnostics = topology.diagnostics("D").await;
    topology.run(&[TopologyAction::Restart { node: "D" }]).await;
    let restarted_kept = topology.device_group_choices("D").await;
    let restarted_diagnostics = topology.diagnostics("D").await;
    topology.shutdown().await;

    assert_eq!(applied_diagnostics.effective_member_count, 3);
    assert_eq!(kept_diagnostics.effective_member_count, 4);
    assert_eq!(restarted_diagnostics.effective_member_count, 4);
    assert!(restarted_kept.device_trust.current_change.is_none());
    assert!(restarted_kept.issues.is_empty());
    assert!(applied.device_trust.current_change.is_none());
    assert!(kept.device_trust.current_change.is_none());
    let change = apply_preview.device_trust.current_change.unwrap();
    let kept_change = keep_preview.device_trust.current_change.unwrap();
    assert!(change.target_device_ids.iter().all(|id| applied
        .device_trust
        .devices
        .iter()
        .any(|device| &device.device_id == id
            && device.membership == uc_engine::DeviceMembershipSummary::Removed)));
    assert!(kept_change.target_device_ids.iter().all(|id| kept
        .device_trust
        .devices
        .iter()
        .any(|device| &device.device_id == id
            && device.membership == uc_engine::DeviceMembershipSummary::Active)));
    assert!(
        change
            .apply_impact
            .usable_device_ids
            .iter()
            .all(|id| !change.target_device_ids.contains(id)),
        "真实选择已经移除目标，但事前继续同步名单仍包含目标"
    );
}

// F3：同一远端移除被不同设备接受和拒绝后，决定必须跨重启持久并保持内容隔离。
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn f3_opposite_removal_decisions_persist_divergence_across_restart() {
    let _scenario = TestScenario::start();
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let mut topology = MembershipTopology::new(rendezvous.uri());
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
            TopologyAction::Join {
                sponsor: "A",
                joiner: "C",
            },
        ])
        .await;
    topology
        .wait_for_equivalent_branch(&["A", "B", "C"], 3)
        .await;
    let baseline_epoch = topology.diagnostics("A").await.group_epoch;
    topology
        .wait_for_group_epoch(&["B", "C"], baseline_epoch)
        .await;
    topology
        .run(&[TopologyAction::Remove {
            sponsor: "A",
            target: "C",
        }])
        .await;
    topology.wait_for_pending_change(&["B", "C"]).await;
    topology
        .run(&[
            TopologyAction::Decide {
                node: "B",
                choice: PendingChangeChoice::Apply,
            },
            TopologyAction::Decide {
                node: "C",
                choice: PendingChangeChoice::Keep,
            },
        ])
        .await;

    topology.assert_snapshot("B", 2, 0).await;
    topology.assert_snapshot("C", 3, 0).await;
    let accepted_before = topology.diagnostics("B").await;
    let rejected_before = topology.diagnostics("C").await;
    assert_ne!(accepted_before.branch_id, rejected_before.branch_id);
    assert_ne!(accepted_before.head_event_id, rejected_before.head_event_id);
    let epoch_deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
    loop {
        // Key distribution may advance while the two snapshots are read.
        let source = topology.diagnostics("A").await.group_epoch;
        let target = topology.diagnostics("B").await.group_epoch;
        if source == target && source >= accepted_before.group_epoch {
            break;
        }
        assert!(
            tokio::time::Instant::now() < epoch_deadline,
            "accepted peers did not converge to their current group epoch"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let accepted_text = "F3 accepted branch transfer";
    assert_eq!(
        topology.send("A", "B", accepted_text).await.total_accepted,
        1
    );
    wait_for_received_text(topology.engine("B"), accepted_text).await;
    let rejected_text = "F3 rejected branch must stay isolated";
    assert_eq!(
        topology.send("B", "C", rejected_text).await.total_accepted,
        0
    );
    assert!(!receiver_has_exact_text(topology.engine("C"), rejected_text).await);

    topology
        .run(&[
            TopologyAction::Restart { node: "B" },
            TopologyAction::Restart { node: "C" },
        ])
        .await;
    let accepted_after = topology.diagnostics("B").await;
    let rejected_after = topology.diagnostics("C").await;
    assert_eq!(accepted_after.branch_id, accepted_before.branch_id);
    assert_eq!(accepted_after.head_event_id, accepted_before.head_event_id);
    assert!(accepted_after.revision >= accepted_before.revision);
    assert_eq!(rejected_after.branch_id, rejected_before.branch_id);
    assert_eq!(rejected_after.head_event_id, rejected_before.head_event_id);
    assert!(rejected_after.revision >= rejected_before.revision);
    topology.assert_snapshot("B", 2, 0).await;
    topology.assert_snapshot("C", 3, 0).await;
    let restarted_text = "F3 restart preserves divergence";
    assert_eq!(
        topology.send("C", "B", restarted_text).await.total_accepted,
        0
    );
    assert!(!receiver_has_exact_text(topology.engine("B"), restarted_text).await);
    topology.shutdown().await;
}

// F2：不同 Sponsor 从共同父 head 移除不同叶子，明确选择后必须精确切换到目标分支。
#[tokio::test(flavor = "multi_thread", worker_threads = 10)]
async fn f2_concurrent_leaf_removals_resolve_to_selected_branch() {
    let _scenario = TestScenario::start();
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let mut topology = MembershipTopology::new(rendezvous.uri());
    topology
        .run(&[
            TopologyAction::Start { node: "A" },
            TopologyAction::Start { node: "B" },
            TopologyAction::Start { node: "C" },
            TopologyAction::Start { node: "D" },
            TopologyAction::Start { node: "E" },
            TopologyAction::Create { node: "A" },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "B",
            },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "C",
            },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "D",
            },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "E",
            },
        ])
        .await;
    topology
        .wait_for_equivalent_branch(&["A", "B", "C", "D", "E"], 5)
        .await;
    let baseline_epoch = topology.diagnostics("A").await.group_epoch;
    topology
        .wait_for_group_epoch(&["B", "C", "D", "E"], baseline_epoch)
        .await;
    topology
        .run(&[
            TopologyAction::Partition {
                left: &["A", "B", "D"],
                right: &["C", "E"],
            },
            TopologyAction::Remove {
                sponsor: "B",
                target: "D",
            },
            TopologyAction::Remove {
                sponsor: "C",
                target: "E",
            },
            TopologyAction::Heal {
                nodes: &["A", "B", "C", "D", "E"],
            },
        ])
        .await;
    topology.assert_snapshot("B", 4, 1).await;
    topology.assert_snapshot("C", 4, 1).await;
    let selected = topology.diagnostics("B").await;
    let choices = topology
        .device_group_choices_for_branch("C", &selected.branch_id)
        .await;
    let choice_id = format!("b:{}", selected.branch_id);
    let conflict = choices
        .issues
        .iter()
        .find(|issue| {
            issue
                .choices
                .iter()
                .any(|choice| choice.choice_id == choice_id)
        })
        .unwrap();
    let remote = choices
        .issues
        .iter()
        .flat_map(|issue| &issue.choices)
        .find(|choice| choice.choice_id == choice_id)
        .unwrap();
    assert!(
        remote.members_complete,
        "已验证远端分支必须提供完整候选名单"
    );
    assert_eq!(remote.member_device_ids.len(), 4);
    assert_eq!(remote.members.len(), 4);
    assert!(remote
        .members
        .iter()
        .all(|member| !member.display_name.is_empty()));
    assert_eq!(
        conflict.reason.kind,
        uc_engine::DeviceGroupChoiceReasonKind::DifferentRemovals
    );
    assert_eq!(conflict.reason.changes.len(), 2);
    let removed = &conflict
        .reason
        .changes
        .iter()
        .find(|change| change.side == uc_engine::DeviceGroupChangeSideSummary::Remote)
        .unwrap()
        .target
        .device_id;
    let impact = remote.impact.as_ref().unwrap();
    assert!(impact.paused_device_ids.contains(removed));
    assert!(impact.requires_rejoin_device_ids.contains(removed));
    assert!(!impact.sync_scope_device_ids.contains(removed));
    assert_eq!(
        impact.pending_confirmation_device_ids,
        impact.sync_scope_device_ids
    );
    topology
        .run(&[TopologyAction::ResolveConflict {
            node: "C",
            branch_from: "B",
        }])
        .await;
    topology
        .wait_for_equivalent_branch_named(&["B", "C"], 4, "F2 selected candidate recovery")
        .await;
    topology.assert_snapshot("C", 4, 0).await;
    let selected_after_recovery = topology.diagnostics("B").await;
    topology
        .wait_for_group_epoch(&["C"], selected_after_recovery.group_epoch)
        .await;
    let resolved = topology.diagnostics("C").await;
    assert_eq!(resolved.branch_id, selected.branch_id);
    assert_eq!(resolved.head_event_id, selected.head_event_id);
    assert_eq!(resolved.group_epoch, selected_after_recovery.group_epoch);
    // 切到保留 E 的新分支后，旧 Remove(E) 不得在重启维护中删除 E 的资料。
    topology.run(&[TopologyAction::Restart { node: "C" }]).await;
    let restarted = topology.device_group_choices("C").await;
    assert_eq!(
        restarted
            .device_trust
            .devices
            .iter()
            .filter(|device| device.membership == uc_engine::DeviceMembershipSummary::Active)
            .count(),
        4,
        "选中组的有效成员在重启后必须仍可完整查询"
    );
    topology.shutdown().await;
}

// F1：共同父 head 上并发移除与新增，两个合法分支必须保持各自成员语义。
#[tokio::test(flavor = "multi_thread", worker_threads = 10)]
async fn f1_remove_and_add_from_parent_head_preserve_branch_membership() {
    let _scenario = TestScenario::start();
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let mut topology = MembershipTopology::new(rendezvous.uri());
    topology
        .run(&[
            TopologyAction::Start { node: "A" },
            TopologyAction::Start { node: "B" },
            TopologyAction::Start { node: "C" },
            TopologyAction::Start { node: "D" },
            TopologyAction::Start { node: "E" },
            TopologyAction::Create { node: "A" },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "B",
            },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "C",
            },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "D",
            },
        ])
        .await;
    topology
        .wait_for_equivalent_branch(&["A", "B", "C", "D"], 4)
        .await;
    let baseline = topology.diagnostics("A").await;

    // 分区前确保现有成员已应用 group 更新，不能只依据成员列表已保存。
    topology
        .wait_for_group_epoch(&["B", "C", "D"], baseline.group_epoch)
        .await;

    // 与其他分区场景一致：网络切断前取得邀请，隔离期间只执行加入。
    let invitation = issue_invitation_named(topology.engine("B"), "B").await;
    topology
        .run(&[TopologyAction::Partition {
            left: &["A", "C", "D"],
            right: &["B", "E"],
        }])
        .await;
    topology.join_with_invitation("B", "E", invitation).await;
    topology
        .run(&[TopologyAction::Remove {
            sponsor: "A",
            target: "D",
        }])
        .await;

    // 移除提交允许 effect 留待恢复；先等待本机 group 更新，再断言分支状态。
    topology
        .wait_for_group_epoch(&["A"], baseline.group_epoch + 1)
        .await;
    let removed_branch = topology.diagnostics("A").await;
    let added_branch = topology.diagnostics("B").await;
    assert_ne!(removed_branch.branch_id, added_branch.branch_id);
    assert_ne!(removed_branch.head_event_id, added_branch.head_event_id);
    assert_eq!(removed_branch.effective_member_count, 3);
    assert_eq!(added_branch.effective_member_count, 5);
    assert!(removed_branch.group_epoch > baseline.group_epoch);
    assert!(added_branch.group_epoch > baseline.group_epoch);
    topology
        .wait_for_group_epoch(&["C"], removed_branch.group_epoch)
        .await;
    topology
        .wait_for_group_epoch(&["E"], added_branch.group_epoch)
        .await;

    // ADR-020：C 尚未决定 A 发起的移除，双方不越过该移除分享普通内容。A 明确看到 C 在等待确认，
    // 发送既不算离线，也不排队等待。
    let c_device_id = topology.device_ids.get("C").unwrap().clone();
    topology.wait_for_pending_change(&["C"]).await;
    wait_for_group_relationship(
        &topology,
        "A",
        &c_device_id,
        uc_engine::DeviceGroupRelationshipSummary::ConfirmationPending,
        Some(uc_engine::DeviceSyncRelationshipSummary::PausedUnverifiable),
    )
    .await;
    let undecided_text = "F1 undecided member must not receive";
    let undecided_report = topology.send("A", "C", undecided_text).await;
    assert_eq!(
        (
            undecided_report.total_accepted,
            undecided_report.total_pending,
            undecided_report.total_offline
        ),
        (0, 0, 0),
        "F1 transfer to an undecided member must be withheld: {undecided_report:?}"
    );
    assert!(!receiver_has_exact_text(topology.engine("C"), undecided_text).await);
    topology
        .run(&[TopologyAction::Decide {
            node: "C",
            choice: PendingChangeChoice::Apply,
        }])
        .await;
    wait_for_group_relationship(
        &topology,
        "A",
        &c_device_id,
        uc_engine::DeviceGroupRelationshipSummary::Consistent,
        None,
    )
    .await;

    let left_text = "F1 removal branch transfer";
    let left_report = topology.send("A", "C", left_text).await;
    assert_eq!(
        left_report.total_accepted + left_report.total_pending,
        1,
        "F1 removal branch transfer was neither accepted nor left in flight: {left_report:?}"
    );
    wait_for_received_text(topology.engine("C"), left_text).await;
    let right_text = "F1 addition branch transfer";
    let right_report = topology.send("B", "E", right_text).await;
    assert_eq!(
        right_report.total_accepted + right_report.total_pending,
        1,
        "F1 addition branch transfer was neither accepted nor left in flight: {right_report:?}"
    );
    wait_for_received_text(topology.engine("E"), right_text).await;
    let removed_text = "F1 removed member must not receive";
    assert_eq!(
        topology.send("A", "D", removed_text).await.total_accepted,
        0
    );
    assert!(!receiver_has_exact_text(topology.engine("D"), removed_text).await);

    topology
        .run(&[TopologyAction::Heal {
            nodes: &["A", "B", "C", "D", "E"],
        }])
        .await;
    for node in ["A", "B", "C", "D", "E"] {
        refresh_available_members(topology.engine(node)).await;
    }
    topology.assert_snapshot("A", 3, 1).await;
    topology.assert_snapshot("B", 5, 1).await;
    let choices_a = topology.device_group_choices("A").await;
    let choices_b = topology.device_group_choices("B").await;
    let d_device_id = topology.device_ids.get("D").unwrap();
    let e_device_id = topology.device_ids.get("E").unwrap();
    // 移除通知送达后移除方不再保留被移除设备（计划 049 R1）。
    assert_eq!(
        choices_a
            .device_trust
            .devices
            .iter()
            .find(|device| &device.device_id == d_device_id)
            .map(|device| device.membership),
        None
    );
    assert_eq!(
        choices_b
            .device_trust
            .devices
            .iter()
            .find(|device| &device.device_id == d_device_id)
            .map(|device| device.membership),
        Some(uc_engine::DeviceMembershipSummary::Active)
    );
    assert_eq!(
        choices_b
            .device_trust
            .devices
            .iter()
            .find(|device| &device.device_id == e_device_id)
            .map(|device| device.membership),
        Some(uc_engine::DeviceMembershipSummary::Active)
    );
    let healed_a = topology.diagnostics("A").await;
    let healed_b = topology.diagnostics("B").await;
    assert_eq!(healed_a.pending_conflict_count, 1);
    assert_eq!(healed_b.pending_conflict_count, 1);
    assert_ne!(healed_a.branch_id, healed_b.branch_id);
    let isolated_text = "F1 healed sibling branches remain isolated";
    assert_eq!(
        topology.send("A", "E", isolated_text).await.total_accepted,
        0
    );
    assert!(!receiver_has_exact_text(topology.engine("E"), isolated_text).await);
    topology.shutdown().await;
}

/// 等待 `node` 看到对端 `peer_device_id` 处于指定设备组关系；给出 `sync` 时同步关系也须一致。
pub(crate) async fn wait_for_group_relationship(
    topology: &MembershipTopology,
    node: &str,
    peer_device_id: &str,
    relationship: uc_engine::DeviceGroupRelationshipSummary,
    sync: Option<uc_engine::DeviceSyncRelationshipSummary>,
) {
    let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
    loop {
        let observed = topology
            .device_group_choices(node)
            .await
            .device_trust
            .devices
            .into_iter()
            .find(|device| device.device_id == peer_device_id)
            .map(|device| (device.group_relationship, device.sync_relationship));
        if observed.is_some_and(|(group, observed_sync)| {
            group == relationship && sync.is_none_or(|expected| expected == observed_sync)
        }) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "node {node} did not reach {relationship:?}/{sync:?} for the peer; observed {observed:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

// 分区门必须同时拒绝新连接并关闭已存在连接；Heal 后使用同一 Engine 恢复通信。
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn topology_partition_blocks_all_iroh_channels_until_healed() {
    let _scenario = TestScenario::start();
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let mut topology = MembershipTopology::new(rendezvous.uri());
    topology
        .run(&[
            TopologyAction::Start { node: "A" },
            TopologyAction::Start { node: "B" },
            TopologyAction::Create { node: "A" },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "B",
            },
            TopologyAction::Partition {
                left: &["A"],
                right: &["B"],
            },
        ])
        .await;

    let blocked_text = "partitioned transfer must stay isolated";
    let blocked = topology.send("A", "B", blocked_text).await;
    assert_eq!(blocked.total_accepted, 0);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!receiver_has_exact_text(topology.engine("B"), blocked_text).await);

    topology
        .run(&[TopologyAction::Heal { nodes: &["A", "B"] }])
        .await;
    wait_for_peer_refresh(topology.engine("A"), "A after heal").await;
    wait_for_peer_refresh(topology.engine("B"), "B after heal").await;
    let healed_text = "healed transfer succeeds";
    let healed = topology.send("A", "B", healed_text).await;
    assert_eq!(healed.total_accepted, 1);
    wait_for_received_text(topology.engine("B"), healed_text).await;
    topology.shutdown().await;
}

// F0：共同 head 分区后由两个 Sponsor 分别准入新设备，必须形成隔离的 sibling 分支。
#[tokio::test(flavor = "multi_thread", worker_threads = 10)]
async fn f0_partitioned_sponsors_create_isolated_sibling_branches() {
    let _scenario = TestScenario::start();
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let mut topology = MembershipTopology::new(rendezvous.uri());
    topology
        .run(&[
            TopologyAction::Start { node: "A" },
            TopologyAction::Start { node: "B" },
            TopologyAction::Start { node: "C" },
            TopologyAction::Start { node: "D" },
            TopologyAction::Start { node: "E" },
            TopologyAction::Create { node: "A" },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "B",
            },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "C",
            },
        ])
        .await;
    topology
        .wait_for_equivalent_branch(&["A", "B", "C"], 3)
        .await;
    topology.wait_for_admission_ready(&["A", "B", "C"]).await;
    let baseline_a = topology.diagnostics("A").await;
    let baseline_b = topology.diagnostics("B").await;
    assert_eq!(baseline_a.branch_id, baseline_b.branch_id);
    assert_eq!(baseline_a.head_event_id, baseline_b.head_event_id);

    topology
        .run(&[
            TopologyAction::Partition {
                left: &["A", "C", "D"],
                right: &["B", "E"],
            },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "D",
            },
            TopologyAction::Join {
                sponsor: "B",
                joiner: "E",
            },
        ])
        .await;

    let branch_a = topology.diagnostics("A").await;
    let branch_b = topology.diagnostics("B").await;
    assert_ne!(branch_a.branch_id, branch_b.branch_id);
    assert_ne!(branch_a.head_event_id, branch_b.head_event_id);
    assert_eq!(branch_a.effective_member_count, 4);
    assert_eq!(branch_b.effective_member_count, 4);
    assert!(branch_a.group_epoch > baseline_a.group_epoch);
    assert!(branch_b.group_epoch > baseline_b.group_epoch);
    assert_eq!(
        branch_a.group_epoch,
        topology.diagnostics("D").await.group_epoch
    );
    assert_eq!(
        branch_b.group_epoch,
        topology.diagnostics("E").await.group_epoch
    );

    let left_text = "F0 left branch transfer";
    assert_eq!(topology.send("A", "D", left_text).await.total_accepted, 1);
    wait_for_received_text(topology.engine("D"), left_text).await;
    let right_text = "F0 right branch transfer";
    assert_eq!(topology.send("B", "E", right_text).await.total_accepted, 1);
    wait_for_received_text(topology.engine("E"), right_text).await;
    let isolated_text = "F0 cross branch transfer must fail";
    assert_eq!(
        topology.send("A", "E", isolated_text).await.total_accepted,
        0
    );
    assert!(!receiver_has_exact_text(topology.engine("E"), isolated_text).await);

    topology
        .run(&[TopologyAction::Heal {
            nodes: &["A", "B", "C", "D", "E"],
        }])
        .await;
    for node in ["A", "B", "C", "D", "E"] {
        refresh_available_members(topology.engine(node)).await;
    }
    topology.assert_snapshot("A", 4, 1).await;
    topology.assert_snapshot("B", 4, 1).await;
    let healed_a = topology.diagnostics("A").await;
    let healed_b = topology.diagnostics("B").await;
    assert_ne!(healed_a.branch_id, healed_b.branch_id);
    assert_eq!(healed_a.pending_conflict_count, 1);
    assert_eq!(healed_b.pending_conflict_count, 1);
    let healed_isolated_text = "F0 healed sibling branches remain isolated";
    assert_eq!(
        topology
            .send("A", "E", healed_isolated_text)
            .await
            .total_accepted,
        0
    );
    assert!(!receiver_has_exact_text(topology.engine("E"), healed_isolated_text).await);
    topology.shutdown().await;
}

// 声明式拓扑脚本只能通过稳定 Engine operation 观察和推进节点。
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn offline_member_returns_after_repeated_invitation_cancellation_and_new_join() {
    let _scenario = TestScenario::start();
    let rendezvous = mount_rendezvous().await;
    let mut topology = MembershipTopology::new(rendezvous.uri());
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
    topology
        .wait_for_equivalent_branch_named(&["A", "B"], 2, "initial members")
        .await;
    topology.stop("B").await;
    let before = topology.diagnostics("A").await;
    for _ in 0..3 {
        issue_invitation(topology.engine("A")).await;
        topology
            .engine("A")
            .execute(Operation::CancelInvitation)
            .await
            .unwrap();
        let OperationResult::SetupState(setup) = topology
            .engine("A")
            .execute(Operation::QuerySetupState)
            .await
            .unwrap()
        else {
            panic!("expected setup state");
        };
        assert!(setup.current_invitation.is_none());
        let after = topology.diagnostics("A").await;
        assert_eq!(after.head_event_id, before.head_event_id);
        assert_eq!(after.group_epoch, before.group_epoch);
        assert_eq!(after.effective_member_count, 2);
    }
    topology.join("A", "C").await;
    let returning = topology.harnesses.get("B").unwrap().start().await;
    topology.engines.insert("B".to_owned(), returning);
    topology
        .wait_for_equivalent_branch_named(
            &["A", "B", "C"],
            3,
            "offline member catches new admission",
        )
        .await;
    let epoch = topology.diagnostics("A").await.group_epoch;
    topology.wait_for_group_epoch(&["B", "C"], epoch).await;
    for node in ["A", "B", "C"] {
        wait_for_peer_refresh(topology.engine(node), node).await;
    }
    let text = "returning member after cancelled invitations";
    assert_eq!(topology.send("A", "B", text).await.total_accepted, 1);
    wait_for_received_text(topology.engine("B"), text).await;
    let reverse = "returning member can send";
    assert_eq!(topology.send("B", "C", reverse).await.total_accepted, 1);
    wait_for_received_text(topology.engine("C"), reverse).await;
    topology.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn topology_script_builds_a_two_node_space_through_public_operations() {
    let _scenario = TestScenario::start();
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let mut topology = MembershipTopology::new(rendezvous.uri());

    topology
        .run(&[
            TopologyAction::Start { node: "A" },
            TopologyAction::Start { node: "B" },
            TopologyAction::Create { node: "A" },
            TopologyAction::Join {
                sponsor: "A",
                joiner: "B",
            },
            TopologyAction::AssertSnapshot {
                node: "B",
                active_members: 2,
                pending_choices: 0,
            },
            TopologyAction::AssertDiagnostics {
                node: "B",
                effective_members: 2,
                pending_conflicts: 0,
                pending_effects: 0,
            },
        ])
        .await;
    topology.shutdown().await;
    uc_engine::flush_test_tracing();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn offline_member_catches_multiple_removals_without_blocking_new_invitations() {
    let _scenario = TestScenario::start();
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let mut topology = MembershipTopology::new(rendezvous.uri());
    for node in ["A", "B", "C", "D"] {
        topology.start(node).await;
    }
    topology.create("A").await;
    for node in ["B", "C", "D"] {
        topology.join("A", node).await;
    }
    topology
        .wait_for_equivalent_branch_named(&["A", "B", "C", "D"], 4, "initial four members")
        .await;
    for node in ["B", "C", "D"] {
        topology.stop(node).await;
    }
    for node in ["C", "D"] {
        topology.remove("A", node).await;
        issue_invitation(topology.engine("A")).await;
        topology
            .engine("A")
            .execute(Operation::CancelInvitation)
            .await
            .unwrap();
    }
    topology.restart("A").await;
    issue_invitation(topology.engine("A")).await;
    let returning = topology.harnesses.get("B").unwrap().start().await;
    topology.engines.insert("B".to_owned(), returning);
    for _ in 0..2 {
        topology.wait_for_pending_change(&["B"]).await;
        topology
            .decide_pending_change("B", PendingChangeChoice::Apply)
            .await;
        if topology.diagnostics("B").await.effective_member_count == 2 {
            break;
        }
    }
    topology
        .wait_for_equivalent_branch_named(&["A", "B"], 2, "returning member accepts removals")
        .await;
    let epoch = topology.diagnostics("A").await.group_epoch;
    topology.wait_for_group_epoch(&["B"], epoch).await;
    for node in ["A", "B"] {
        wait_for_peer_refresh(topology.engine(node), node).await;
    }
    let text = "multiple offline removals recovered";
    assert_eq!(topology.send("A", "B", text).await.total_accepted, 1);
    wait_for_received_text(topology.engine("B"), text).await;
    topology.shutdown().await;
}

async fn refresh_available_members(engine: &Engine) {
    // These topologies deliberately contain isolated or divergent members.
    // Refresh must settle; branch and content assertions prove the result.
    let result = tokio::time::timeout(
        WAIT_TIMEOUT,
        engine.execute(Operation::RefreshPeerConnections),
    )
    .await
    .expect("peer refresh exceeded its budget")
    .expect("peer refresh failed");
    let OperationResult::PeerConnectionsRefreshed(report) = result else {
        panic!("peer refresh result expected")
    };
    assert_eq!(report.total, report.online + report.offline + report.errors);
}
