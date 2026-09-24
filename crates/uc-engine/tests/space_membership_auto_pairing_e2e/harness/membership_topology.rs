//! 多节点拓扑驱动：节点生命周期、邀请、分区与收敛等待。

use super::*;

#[derive(Debug, Clone, Copy)]
pub(crate) enum TopologyAction<'a> {
    Start {
        node: &'a str,
    },
    Create {
        node: &'a str,
    },
    Join {
        sponsor: &'a str,
        joiner: &'a str,
    },
    Remove {
        sponsor: &'a str,
        target: &'a str,
    },
    Restart {
        node: &'a str,
    },
    Stop {
        node: &'a str,
    },
    Decide {
        node: &'a str,
        choice: PendingChangeChoice,
    },
    AssertSnapshot {
        node: &'a str,
        active_members: usize,
        pending_choices: usize,
    },
    AssertDiagnostics {
        node: &'a str,
        effective_members: u32,
        pending_conflicts: u32,
        pending_effects: u32,
    },
    Partition {
        left: &'a [&'a str],
        right: &'a [&'a str],
    },
    PartitionGroups {
        groups: &'a [&'a [&'a str]],
    },
    GroupedBridge {
        groups: &'a [&'a [&'a str]],
        left: &'a str,
        right: &'a str,
    },
    Bridge {
        left: &'a str,
        right: &'a str,
        left_group: &'a [&'a str],
        right_group: &'a [&'a str],
    },
    Ring {
        nodes: &'a [&'a str],
        isolated: &'a [&'a str],
    },
    Chain {
        nodes: &'a [&'a str],
        offline: &'a [&'a str],
    },
    Heal {
        nodes: &'a [&'a str],
    },
    ResolveConflict {
        node: &'a str,
        branch_from: &'a str,
    },
}

impl TopologyAction<'_> {
    /// 场景事件名，按动作类型记录到场景工件。
    fn event_kind(&self) -> &'static str {
        match self {
            Self::Start { .. } => "topology-start",
            Self::Create { .. } => "topology-create",
            Self::Join { .. } => "topology-join",
            Self::Remove { .. } => "topology-remove",
            Self::Restart { .. } => "topology-restart",
            Self::Stop { .. } => "topology-stop",
            Self::Decide { .. } => "topology-decide",
            Self::AssertSnapshot { .. } => "topology-assert-snapshot",
            Self::AssertDiagnostics { .. } => "topology-assert-diagnostics",
            Self::Partition { .. } => "topology-partition",
            Self::PartitionGroups { .. } => "topology-partition-groups",
            Self::GroupedBridge { .. } => "topology-grouped-bridge",
            Self::Bridge { .. } => "topology-bridge",
            Self::Ring { .. } => "topology-ring",
            Self::Chain { .. } => "topology-chain",
            Self::Heal { .. } => "topology-heal",
            Self::ResolveConflict { .. } => "topology-resolve-conflict",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum PendingChangeChoice {
    Apply,
    Keep,
}

pub(crate) struct MembershipTopology {
    pub(crate) rendezvous_base_url: String,
    pub(crate) harnesses: HashMap<String, DeviceHarness>,
    pub(crate) engines: HashMap<String, Engine>,
    pub(crate) endpoint_ids_by_node: HashMap<String, [u8; 32]>,
    pub(crate) space_ids: HashMap<String, String>,
    pub(crate) device_ids: HashMap<String, String>,
}

impl MembershipTopology {
    pub(crate) fn new(rendezvous_base_url: String) -> Self {
        Self {
            rendezvous_base_url,
            harnesses: HashMap::new(),
            engines: HashMap::new(),
            endpoint_ids_by_node: HashMap::new(),
            space_ids: HashMap::new(),
            device_ids: HashMap::new(),
        }
    }

    pub(crate) async fn run(&mut self, actions: &[TopologyAction<'_>]) {
        for action in actions {
            scenario::record_event(action.event_kind());
            match *action {
                TopologyAction::Start { node } => self.start(node).await,
                TopologyAction::Create { node } => self.create(node).await,
                TopologyAction::Join { sponsor, joiner } => self.join(sponsor, joiner).await,
                TopologyAction::Remove { sponsor, target } => self.remove(sponsor, target).await,
                TopologyAction::Restart { node } => self.restart(node).await,
                TopologyAction::Stop { node } => self.stop(node).await,
                TopologyAction::Decide { node, choice } => {
                    self.decide_pending_change(node, choice).await
                }
                TopologyAction::AssertSnapshot {
                    node,
                    active_members,
                    pending_choices,
                } => {
                    self.assert_snapshot(node, active_members, pending_choices)
                        .await
                }
                TopologyAction::AssertDiagnostics {
                    node,
                    effective_members,
                    pending_conflicts,
                    pending_effects,
                } => {
                    self.assert_diagnostics(
                        node,
                        effective_members,
                        pending_conflicts,
                        pending_effects,
                    )
                    .await
                }
                TopologyAction::Partition { left, right } => self.partition(left, right).await,
                TopologyAction::PartitionGroups { groups } => {
                    self.partition_groups(groups, None).await
                }
                TopologyAction::GroupedBridge {
                    groups,
                    left,
                    right,
                } => self.partition_groups(groups, Some((left, right))).await,
                TopologyAction::Bridge {
                    left,
                    right,
                    left_group,
                    right_group,
                } => {
                    self.bridge(left, right, left_group, right_group).await;
                }
                TopologyAction::Ring { nodes, isolated } => self.ring(nodes, isolated).await,
                TopologyAction::Chain { nodes, offline } => self.chain(nodes, offline).await,
                TopologyAction::Heal { nodes } => self.heal(nodes).await,
                TopologyAction::ResolveConflict { node, branch_from } => {
                    self.resolve_conflict(node, branch_from).await
                }
            }
        }
    }

    pub(crate) async fn start(&mut self, node: &str) {
        assert!(
            !self.engines.contains_key(node),
            "node {node} started twice"
        );
        let harness = DeviceHarness::new(self.rendezvous_base_url.clone());
        let engine = harness.start().await;
        let endpoint_id = query_endpoint_id(&engine, node).await;
        self.harnesses.insert(node.to_owned(), harness);
        self.engines.insert(node.to_owned(), engine);
        self.endpoint_ids_by_node
            .insert(node.to_owned(), endpoint_id);
    }

    pub(crate) async fn stop(&mut self, node: &str) {
        let engine = self
            .engines
            .remove(node)
            .unwrap_or_else(|| panic!("node {node} is not started"));
        engine
            .shutdown(SHUTDOWN_TIMEOUT)
            .await
            .unwrap_or_else(|error| panic!("node {node} shutdown failed: {error}"));
    }

    pub(crate) async fn restart(&mut self, node: &str) {
        let engine = self
            .engines
            .remove(node)
            .unwrap_or_else(|| panic!("node {node} is not started"));
        engine
            .shutdown(SHUTDOWN_TIMEOUT)
            .await
            .unwrap_or_else(|error| panic!("node {node} shutdown failed: {error}"));
        let restarted = self
            .harnesses
            .get(node)
            .unwrap_or_else(|| panic!("node {node} has no harness"))
            .start()
            .await;
        self.engines.insert(node.to_owned(), restarted);
        self.wait_for_membership_ready(node).await;
    }

    pub(crate) async fn wait_for_membership_ready(&self, node: &str) {
        let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
        loop {
            match self
                .engine(node)
                .execute(Operation::QueryDeviceGroupChoices)
                .await
            {
                Ok(OperationResult::DeviceGroupChoices(_)) => return,
                Ok(_) => panic!("node {node} returned an unexpected device group result"),
                Err(error)
                    if error.code() == 1211
                        && error.is_retryable()
                        && tokio::time::Instant::now() < deadline =>
                {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Err(error) => panic!("node {node} did not become membership-ready: {error}"),
            }
        }
    }

    pub(crate) async fn create(&mut self, node: &str) {
        let engine = self.engine(node);
        let (space_id, device_id) = create_space(engine, node).await;
        self.space_ids.insert(node.to_owned(), space_id);
        self.device_ids.insert(node.to_owned(), device_id);
    }

    pub(crate) async fn join(&mut self, sponsor: &str, joiner: &str) {
        let invitation = issue_invitation(self.engine(sponsor)).await;
        self.join_with_invitation(sponsor, joiner, invitation).await;
    }

    pub(crate) async fn join_with_invitation(
        &mut self,
        sponsor: &str,
        joiner: &str,
        full_invitation: String,
    ) {
        let space_id = self
            .space_ids
            .get(sponsor)
            .unwrap_or_else(|| panic!("sponsor {sponsor} has no space"))
            .clone();
        let joined =
            join_with_invitation(self.engine(joiner), joiner, &space_id, full_invitation).await;
        self.space_ids.insert(joiner.to_owned(), space_id);
        self.device_ids
            .insert(joiner.to_owned(), joined.self_device_id);
    }

    pub(crate) async fn remove(&self, sponsor: &str, target: &str) {
        let target_device_id = self
            .device_ids
            .get(target)
            .unwrap_or_else(|| panic!("target {target} has no device id"))
            .clone();
        let initial_members = self.diagnostics(sponsor).await.effective_member_count;
        let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
        loop {
            let result = self
                .engine(sponsor)
                .execute(Operation::RemoveMember(RemoveMemberInput {
                    device_id: target_device_id.clone(),
                }))
                .await;
            if matches!(&result, Err(error) if error.code() == 1393 && error.is_retryable()) {
                return;
            }
            if matches!(&result, Err(error) if error.code() == 1394)
                && self.diagnostics(sponsor).await.effective_member_count == initial_members
                && tokio::time::Instant::now() < deadline
            {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
            // 成员状态暂时不可用（例如加入后仍在切换会话）时，按可重试错误稍后再试。
            if matches!(&result, Err(error) if error.code() == 1392 && error.is_retryable())
                && tokio::time::Instant::now() < deadline
            {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
            let result =
                result.unwrap_or_else(|error| panic!("node {sponsor} remove failed: {error}"));
            assert!(matches!(result, OperationResult::DeviceTrust(_)));
            return;
        }
    }

    pub(crate) async fn partition(&self, left: &[&str], right: &[&str]) {
        let left_ids = self.endpoint_ids(left).await;
        let right_ids = self.endpoint_ids(right).await;
        for node in left {
            self.set_partition(node, right_ids.clone()).await;
        }
        for node in right {
            self.set_partition(node, left_ids.clone()).await;
        }
    }

    pub(crate) async fn partition_groups(&self, groups: &[&[&str]], bridge: Option<(&str, &str)>) {
        let all = groups
            .iter()
            .flat_map(|group| group.iter().copied())
            .collect::<Vec<_>>();
        let unique = all
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(all.len(), unique.len(), "partition groups must be disjoint");
        if let Some((left, right)) = bridge {
            assert!(
                groups.iter().any(|group| group.contains(&left))
                    && groups.iter().any(|group| group.contains(&right)),
                "grouped bridge endpoints must belong to partition groups"
            );
        }
        for group in groups {
            for node in *group {
                let bridge_peer = bridge.and_then(|(left, right)| {
                    if *node == left {
                        Some(right)
                    } else if *node == right {
                        Some(left)
                    } else {
                        None
                    }
                });
                let blocked = all
                    .iter()
                    .copied()
                    .filter(|candidate| {
                        candidate != node
                            && !group.contains(candidate)
                            && Some(*candidate) != bridge_peer
                    })
                    .collect::<Vec<_>>();
                self.set_partition(node, self.endpoint_ids(&blocked).await)
                    .await;
            }
        }
    }

    pub(crate) async fn bridge(
        &self,
        left: &str,
        right: &str,
        left_group: &[&str],
        right_group: &[&str],
    ) {
        assert!(left_group.contains(&left));
        assert!(right_group.contains(&right));
        let left_blocked = right_group
            .iter()
            .copied()
            .filter(|node| *node != right)
            .collect::<Vec<_>>();
        let right_blocked = left_group
            .iter()
            .copied()
            .filter(|node| *node != left)
            .collect::<Vec<_>>();
        self.set_partition(left, self.endpoint_ids(&left_blocked).await)
            .await;
        self.set_partition(right, self.endpoint_ids(&right_blocked).await)
            .await;
    }

    pub(crate) async fn ring(&self, nodes: &[&str], isolated: &[&str]) {
        assert!(nodes.len() >= 4, "ring requires at least four nodes");
        let all = nodes
            .iter()
            .chain(isolated.iter())
            .copied()
            .collect::<Vec<_>>();
        for (index, node) in nodes.iter().enumerate() {
            let previous = nodes[(index + nodes.len() - 1) % nodes.len()];
            let next = nodes[(index + 1) % nodes.len()];
            let blocked = all
                .iter()
                .copied()
                .filter(|candidate| {
                    candidate != node && *candidate != previous && *candidate != next
                })
                .collect::<Vec<_>>();
            self.set_partition(node, self.endpoint_ids(&blocked).await)
                .await;
        }
        for node in isolated {
            let blocked = all
                .iter()
                .copied()
                .filter(|candidate| candidate != node)
                .collect::<Vec<_>>();
            self.set_partition(node, self.endpoint_ids(&blocked).await)
                .await;
        }
    }

    pub(crate) async fn chain(&self, nodes: &[&str], offline: &[&str]) {
        assert!(nodes.len() >= 2, "chain requires at least two online nodes");
        let all = nodes
            .iter()
            .chain(offline.iter())
            .copied()
            .collect::<Vec<_>>();
        for (index, node) in nodes.iter().enumerate() {
            let previous = index.checked_sub(1).map(|previous| nodes[previous]);
            let next = nodes.get(index + 1).copied();
            let blocked = all
                .iter()
                .copied()
                .filter(|candidate| {
                    candidate != node && Some(*candidate) != previous && Some(*candidate) != next
                })
                .collect::<Vec<_>>();
            self.set_partition(node, self.endpoint_ids(&blocked).await)
                .await;
        }
    }

    pub(crate) async fn heal(&self, nodes: &[&str]) {
        for node in nodes {
            self.set_partition(node, Vec::new()).await;
        }
    }

    pub(crate) async fn endpoint_ids(&self, nodes: &[&str]) -> Vec<[u8; 32]> {
        nodes
            .iter()
            .map(|node| {
                *self
                    .endpoint_ids_by_node
                    .get(*node)
                    .unwrap_or_else(|| panic!("node {node} has no endpoint id"))
            })
            .collect()
    }

    pub(crate) async fn set_partition(&self, node: &str, blocked_endpoint_ids: Vec<[u8; 32]>) {
        let expected_count = blocked_endpoint_ids.len();
        let result = self
            .engine(node)
            .execute_dev(uc_engine::DevOperation::SetNetworkPartition {
                blocked_endpoint_ids,
            })
            .await
            .unwrap_or_else(|error| panic!("node {node} partition update failed: {error}"));
        assert_eq!(
            result,
            uc_engine::DevOperationResult::NetworkPartitionUpdated {
                blocked_peer_count: expected_count,
            }
        );
    }

    pub(crate) async fn send(
        &self,
        sender: &str,
        receiver: &str,
        text: &str,
    ) -> uc_engine::SendReportSummary {
        let receiver_id = self
            .device_ids
            .get(receiver)
            .unwrap_or_else(|| panic!("receiver {receiver} has no device id"));
        let result = self
            .engine(sender)
            .execute(Operation::SendText(SendTextInput {
                text: text.to_owned(),
                target_devices: vec![receiver_id.clone()],
            }))
            .await
            .unwrap_or_else(|error| panic!("node {sender} send failed: {error}"));
        let OperationResult::EntrySent(report) = result else {
            panic!("node {sender} returned an unexpected send result");
        };
        report
    }

    pub(crate) async fn wait_for_connected_peer(&self, sender: &str, receiver: &str) {
        let receiver_id = self
            .device_ids
            .get(receiver)
            .unwrap_or_else(|| panic!("receiver {receiver} has no device id"));
        let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
        loop {
            match self
                .engine(sender)
                .execute(Operation::QueryPeerConnections)
                .await
            {
                Ok(OperationResult::PeerConnections(peers))
                    if peers
                        .iter()
                        .any(|peer| &peer.peer_id == receiver_id && peer.connected) =>
                {
                    return;
                }
                Ok(OperationResult::PeerConnections(_)) => {}
                Ok(_) => panic!("node {sender} returned an unexpected peer connection result"),
                Err(error) if error.is_retryable() && tokio::time::Instant::now() < deadline => {}
                Err(error) => panic!("node {sender} peer connection query failed: {error}"),
            }
            scenario::ensure_before(deadline, "wait-for-connected-peer", || {
                format!("same-branch peer {sender}-{receiver} did not become connected")
            });
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    pub(crate) async fn wait_for_admission_ready(&self, nodes: &[&str]) {
        let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
        loop {
            let mut ready = true;
            for node in nodes {
                let diagnostics = self.diagnostics(node).await;
                if diagnostics.pending_confirmation_count != 0
                    || diagnostics.pending_effect_count != 0
                {
                    ready = false;
                }
            }
            if ready {
                return;
            }
            scenario::ensure_before(deadline, "wait-for-admission-ready", || {
                "membership admission did not become ready".to_owned()
            });
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    pub(crate) async fn diagnostics(&self, node: &str) -> uc_engine::MembershipDiagnosticsSummary {
        let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
        loop {
            match self
                .engine(node)
                .execute(Operation::QueryMembershipDiagnostics)
                .await
            {
                Ok(OperationResult::MembershipDiagnostics(summary)) => return summary,
                Ok(_) => panic!("node {node} returned an unexpected diagnostics result"),
                Err(error) if error.is_retryable() && tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Err(error) => panic!("node {node} diagnostics failed: {error}"),
            }
        }
    }

    pub(crate) async fn device_group_choices(
        &self,
        node: &str,
    ) -> uc_engine::DeviceGroupChoicesSummary {
        let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
        loop {
            match self
                .engine(node)
                .execute(Operation::QueryDeviceGroupChoices)
                .await
            {
                Ok(OperationResult::DeviceGroupChoices(summary)) => return summary,
                Ok(_) => panic!("node {node} returned an unexpected device group result"),
                Err(error) if error.is_retryable() && tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Err(error) => panic!("node {node} device group query failed: {error}"),
            }
        }
    }

    pub(crate) async fn device_group_choices_for_branch(
        &self,
        node: &str,
        branch_id: &str,
    ) -> uc_engine::DeviceGroupChoicesSummary {
        let choice_id = format!("b:{branch_id}");
        let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
        loop {
            let choices = self.device_group_choices(node).await;
            if choices.issues.iter().any(|issue| {
                issue.issue_id.starts_with("c:")
                    && issue
                        .choices
                        .iter()
                        .any(|choice| choice.choice_id == choice_id)
            }) {
                return choices;
            }
            scenario::ensure_before(deadline, "device-group-choices-for-branch", || {
                format!("node {node} did not offer the requested branch")
            });
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    pub(crate) async fn resolve_conflict(&self, node: &str, branch_from: &str) {
        let target = self.diagnostics(branch_from).await.branch_id;
        let choice_id = format!("b:{target}");
        let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
        loop {
            let choices = self.device_group_choices_for_branch(node, &target).await;
            let issue = choices
                .issues
                .iter()
                .find(|issue| {
                    issue.issue_id.starts_with("c:")
                        && issue
                            .choices
                            .iter()
                            .any(|choice| choice.choice_id == choice_id)
                })
                .unwrap_or_else(|| panic!("node {node} has no branch conflict"));
            assert!(issue
                .choices
                .iter()
                .any(|choice| choice.choice_id == choice_id));
            let result = match self
                .engine(node)
                .execute(Operation::ChooseDeviceGroup(ChooseDeviceGroupInput {
                    issue_id: issue.issue_id.clone(),
                    choice_id: choice_id.clone(),
                    expected_revision: choices.revision,
                    confirm_local_removal: false,
                }))
                .await
            {
                Ok(result) => result,
                Err(error) if error.is_retryable() && tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
                Err(error) => panic!("node {node} conflict resolution failed: {error}"),
            };
            let OperationResult::DeviceGroupChosen(result) = result else {
                panic!("node {node} returned an unexpected conflict resolution result");
            };
            match result.outcome {
                uc_engine::DeviceGroupChoiceOutcomeSummary::Pending
                | uc_engine::DeviceGroupChoiceOutcomeSummary::Completed
                | uc_engine::DeviceGroupChoiceOutcomeSummary::AlreadyCompleted => return,
                uc_engine::DeviceGroupChoiceOutcomeSummary::StateChanged => {
                    scenario::ensure_before(deadline, "resolve-conflict", || {
                        format!("node {node} conflict selection revision did not stabilize")
                    });
                }
                outcome => panic!("node {node} conflict selection returned {outcome:?}"),
            }
        }
    }

    pub(crate) async fn wait_for_pending_change(&self, nodes: &[&str]) {
        let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
        loop {
            let mut ready = true;
            for node in nodes {
                if !self
                    .device_group_choices(node)
                    .await
                    .issues
                    .iter()
                    .any(|issue| issue.issue_id.starts_with("p:"))
                {
                    ready = false;
                }
            }
            if ready {
                return;
            }
            scenario::ensure_before(deadline, "wait-for-pending-change", || {
                format!("pending removal did not reach every decision node {nodes:?}")
            });
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    pub(crate) async fn wait_for_branch_conflict(&self, nodes: &[&str]) {
        let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
        loop {
            let mut ready = true;
            for node in nodes {
                if !self
                    .device_group_choices(node)
                    .await
                    .issues
                    .iter()
                    .any(|issue| issue.issue_id.starts_with("c:"))
                {
                    ready = false;
                }
            }
            if ready {
                return;
            }
            scenario::ensure_before(deadline, "wait-for-branch-conflict", || {
                "branch conflict did not reach every bridge endpoint".to_owned()
            });
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    pub(crate) async fn wait_for_stable_pending_effects(&self, nodes: &[&str]) -> Vec<u32> {
        const REQUIRED_STABLE_SAMPLES: usize = 10;
        let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
        let mut previous = None;
        let mut stable_samples = 0;
        loop {
            let mut state = Vec::with_capacity(nodes.len());
            for node in nodes {
                state.push(self.diagnostics(node).await.pending_effect_count);
            }
            if previous.as_ref() == Some(&state) {
                stable_samples += 1;
                if stable_samples >= REQUIRED_STABLE_SAMPLES {
                    return state;
                }
            } else {
                previous = Some(state);
                stable_samples = 0;
            }
            scenario::ensure_before(deadline, "wait-for-stable-pending-effects", || {
                "membership pending effects did not stabilize".to_owned()
            });
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    pub(crate) async fn decide_pending_change(&self, node: &str, choice: PendingChangeChoice) {
        let choice_id = match choice {
            PendingChangeChoice::Apply => "apply",
            PendingChangeChoice::Keep => "keep",
        };
        let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
        loop {
            let choices = self.device_group_choices(node).await;
            let issue = choices
                .issues
                .iter()
                .find(|issue| issue.issue_id.starts_with("p:"))
                .unwrap_or_else(|| panic!("node {node} has no pending removal"));
            let result = match self
                .engine(node)
                .execute(Operation::ChooseDeviceGroup(ChooseDeviceGroupInput {
                    issue_id: issue.issue_id.clone(),
                    choice_id: choice_id.to_owned(),
                    expected_revision: choices.revision,
                    confirm_local_removal: false,
                }))
                .await
            {
                Ok(result) => result,
                Err(error) if error.is_retryable() && tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
                Err(error) => panic!("node {node} decision failed: {error}"),
            };
            let OperationResult::DeviceGroupChosen(result) = result else {
                panic!("node {node} returned an unexpected decision result");
            };
            match result.outcome {
                uc_engine::DeviceGroupChoiceOutcomeSummary::Completed
                | uc_engine::DeviceGroupChoiceOutcomeSummary::AlreadyCompleted => return,
                uc_engine::DeviceGroupChoiceOutcomeSummary::StateChanged => {
                    scenario::ensure_before(deadline, "decide-pending-change", || {
                        format!("node {node} pending decision revision did not stabilize")
                    });
                }
                outcome => panic!("node {node} pending decision returned {outcome:?}"),
            }
        }
    }

    pub(crate) async fn apply_local_removal_with_confirmation(&self, node: &str) {
        let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
        loop {
            let choices = self.device_group_choices(node).await;
            let issue = choices
                .issues
                .iter()
                .find(|issue| issue.issue_id.starts_with("p:"))
                .unwrap_or_else(|| panic!("node {node} has no pending local removal"));
            let submit = |confirm_local_removal| {
                self.engine(node)
                    .execute(Operation::ChooseDeviceGroup(ChooseDeviceGroupInput {
                        issue_id: issue.issue_id.clone(),
                        choice_id: "apply".to_owned(),
                        expected_revision: choices.revision,
                        confirm_local_removal,
                    }))
            };

            let first = submit(false)
                .await
                .unwrap_or_else(|error| panic!("node {node} local removal prompt failed: {error}"));
            let OperationResult::DeviceGroupChosen(first) = first else {
                panic!("node {node} returned an unexpected local removal prompt result");
            };
            if first.outcome == uc_engine::DeviceGroupChoiceOutcomeSummary::StateChanged
                && tokio::time::Instant::now() < deadline
            {
                continue;
            }
            assert_eq!(
                first.outcome,
                uc_engine::DeviceGroupChoiceOutcomeSummary::LocalDeviceConfirmationRequired
            );

            let confirmed = submit(true).await.unwrap_or_else(|error| {
                panic!("node {node} local removal confirmation failed: {error}")
            });
            let OperationResult::DeviceGroupChosen(confirmed) = confirmed else {
                panic!("node {node} returned an unexpected local removal confirmation result");
            };
            match confirmed.outcome {
                uc_engine::DeviceGroupChoiceOutcomeSummary::Completed
                | uc_engine::DeviceGroupChoiceOutcomeSummary::AlreadyCompleted => return,
                uc_engine::DeviceGroupChoiceOutcomeSummary::StateChanged
                    if tokio::time::Instant::now() < deadline => {}
                outcome => panic!("node {node} local removal confirmation returned {outcome:?}"),
            }
        }
    }

    pub(crate) async fn wait_for_equivalent_branch(&self, nodes: &[&str], effective_members: u32) {
        self.wait_for_equivalent_branch_named(nodes, effective_members, "unnamed topology phase")
            .await;
    }

    /// 历史等价：各节点位于同一分支与 head、有效成员数相符，且本机成员效果全部落实。历史提交后效果由执行器
    /// 随后执行，只比较 head 会在效果落实前读到旧的组 epoch。
    pub(crate) async fn wait_for_equivalent_branch_named(
        &self,
        nodes: &[&str],
        effective_members: u32,
        phase: &str,
    ) {
        let _stage = scenario::stage("wait-branch-equivalence");
        let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
        loop {
            let mut snapshots = Vec::with_capacity(nodes.len());
            for node in nodes {
                snapshots.push(self.diagnostics(node).await);
            }
            let first = snapshots
                .first()
                .unwrap_or_else(|| panic!("branch equivalence requires at least one node"));
            if snapshots.iter().all(|snapshot| {
                snapshot.branch_id == first.branch_id
                    && snapshot.head_event_id == first.head_event_id
                    && snapshot.effective_member_count == effective_members
                    && snapshot.pending_effect_count == 0
            }) {
                return;
            }
            scenario::ensure_before(deadline, "wait-for-equivalent-branch-named", || {
                format!("nodes did not converge to the expected branch during {phase}")
            });
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    pub(crate) async fn wait_for_group_epoch(&self, nodes: &[&str], expected_epoch: u64) {
        self.wait_for_group_epoch_named(nodes, expected_epoch, "unnamed topology phase")
            .await;
    }

    pub(crate) async fn wait_for_group_epoch_named(
        &self,
        nodes: &[&str],
        expected_epoch: u64,
        phase: &str,
    ) {
        let _stage = scenario::stage("wait-group-epoch");
        let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
        let mut observed_epochs = Vec::new();
        loop {
            let mut all_match = true;
            observed_epochs.clear();
            for node in nodes {
                let epoch = self.diagnostics(node).await.group_epoch;
                observed_epochs.push(epoch);
                if epoch != expected_epoch {
                    all_match = false;
                }
            }
            if all_match {
                return;
            }
            scenario::ensure_before(deadline, "wait-for-group-epoch-named", || {
                format!("nodes did not reach group epoch {expected_epoch} during {phase}; observed={observed_epochs:?}")
            });
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    pub(crate) async fn assert_snapshot(
        &self,
        node: &str,
        expected_active_members: usize,
        expected_pending_choices: usize,
    ) {
        let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
        loop {
            let observation = match self
                .engine(node)
                .execute(Operation::QueryDeviceGroupChoices)
                .await
            {
                Ok(OperationResult::DeviceGroupChoices(summary)) => {
                    let active_members = summary
                        .device_trust
                        .devices
                        .iter()
                        .filter(|device| {
                            device.membership == uc_engine::DeviceMembershipSummary::Active
                        })
                        .count();
                    let observation = format!(
                        "active={active_members}, issues={}, revision={}",
                        summary.issues.len(),
                        summary.revision
                    );
                    if active_members == expected_active_members
                        && summary.issues.len() == expected_pending_choices
                    {
                        return;
                    }
                    observation
                }
                Ok(_) => "unexpected result".to_owned(),
                Err(error) => format!("error={error}"),
            };
            scenario::ensure_before(deadline, "assert-snapshot", || {
                format!("node {node} did not reach the expected public snapshot; last observation: {observation}")
            });
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    pub(crate) async fn assert_diagnostics(
        &self,
        node: &str,
        expected_effective_members: u32,
        expected_pending_conflicts: u32,
        expected_pending_effects: u32,
    ) {
        let result = self
            .engine(node)
            .execute(Operation::QueryMembershipDiagnostics)
            .await
            .unwrap_or_else(|error| panic!("node {node} diagnostics failed: {error}"));
        let OperationResult::MembershipDiagnostics(summary) = result else {
            panic!("node {node} returned an unexpected diagnostics result");
        };

        assert_eq!(summary.branch_id.len(), 64);
        assert_eq!(summary.head_event_id.len(), 64);
        assert!(summary.group_epoch > 0);
        assert_eq!(summary.effective_member_count, expected_effective_members);
        assert_eq!(summary.pending_conflict_count, expected_pending_conflicts);
        assert_eq!(summary.pending_effect_count, expected_pending_effects);
        assert!(summary.transition_phases.is_empty());
    }

    pub(crate) fn engine(&self, node: &str) -> &Engine {
        self.engines
            .get(node)
            .unwrap_or_else(|| panic!("node {node} is not started"))
    }

    pub(crate) async fn shutdown(&self) {
        for engine in self.engines.values() {
            engine
                .shutdown(SHUTDOWN_TIMEOUT)
                .await
                .expect("shut down topology node");
        }
    }
}
