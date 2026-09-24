use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use uc_core::membership::{
    MembershipHistoryExchangeEndpointPort, MembershipHistoryExchangeError, MembershipHistoryMessage,
};
use uc_core::DeviceId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum VirtualFrameOutcome {
    Accepted,
    Rejected,
    Unavailable,
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct VirtualFrameRecord {
    sequence: u64,
    source: &'static str,
    target: &'static str,
    outcome: VirtualFrameOutcome,
}

impl VirtualFrameRecord {
    pub(super) fn outcome(&self) -> VirtualFrameOutcome {
        self.outcome
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum VirtualMembershipNetworkError {
    InvalidNode,
    DuplicateNode,
    DuplicateIdentity,
    UnknownNode,
    Offline,
    Rejected,
    PairingInProgress,
    Transport,
    FrameBudgetExceeded,
}

struct VirtualNode {
    device_id: DeviceId,
    endpoint: Arc<dyn MembershipHistoryExchangeEndpointPort>,
}

struct VirtualMembershipNetworkState {
    nodes: BTreeMap<&'static str, VirtualNode>,
    blocked: BTreeSet<(&'static str, &'static str)>,
    trace: Vec<VirtualFrameRecord>,
    next_sequence: u64,
}

pub(super) struct VirtualMembershipNetwork {
    max_frames: usize,
    state: Mutex<VirtualMembershipNetworkState>,
}

impl VirtualMembershipNetwork {
    pub(super) fn new(max_frames: usize) -> Self {
        Self {
            max_frames,
            state: Mutex::new(VirtualMembershipNetworkState {
                nodes: BTreeMap::new(),
                blocked: BTreeSet::new(),
                trace: Vec::new(),
                next_sequence: 1,
            }),
        }
    }

    pub(super) fn register(
        &self,
        label: &'static str,
        device_id: DeviceId,
        endpoint: Arc<dyn MembershipHistoryExchangeEndpointPort>,
    ) -> Result<(), VirtualMembershipNetworkError> {
        if !valid_label(label) {
            return Err(VirtualMembershipNetworkError::InvalidNode);
        }
        let mut state = self.state.lock().expect("virtual network is available");
        if state.nodes.contains_key(label) {
            return Err(VirtualMembershipNetworkError::DuplicateNode);
        }
        if state.nodes.values().any(|node| node.device_id == device_id) {
            return Err(VirtualMembershipNetworkError::DuplicateIdentity);
        }
        state.nodes.insert(
            label,
            VirtualNode {
                device_id,
                endpoint,
            },
        );
        Ok(())
    }

    pub(super) fn partition(
        &self,
        source: &'static str,
        target: &'static str,
    ) -> Result<(), VirtualMembershipNetworkError> {
        let mut state = self.state.lock().expect("virtual network is available");
        ensure_nodes(&state, source, target)?;
        state.blocked.insert((source, target));
        Ok(())
    }

    pub(super) fn heal(
        &self,
        source: &'static str,
        target: &'static str,
    ) -> Result<(), VirtualMembershipNetworkError> {
        let mut state = self.state.lock().expect("virtual network is available");
        ensure_nodes(&state, source, target)?;
        state.blocked.remove(&(source, target));
        Ok(())
    }

    pub(super) async fn send(
        &self,
        source: &'static str,
        target: &'static str,
        message: MembershipHistoryMessage,
    ) -> Result<MembershipHistoryMessage, VirtualMembershipNetworkError> {
        let (source_device_id, endpoint, sequence) = {
            let mut state = self.state.lock().expect("virtual network is available");
            ensure_nodes(&state, source, target)?;
            if state.trace.len() >= self.max_frames {
                return Err(VirtualMembershipNetworkError::FrameBudgetExceeded);
            }
            let sequence = state.next_sequence;
            state.next_sequence = state
                .next_sequence
                .checked_add(1)
                .ok_or(VirtualMembershipNetworkError::FrameBudgetExceeded)?;
            if state.blocked.contains(&(source, target)) {
                state.trace.push(VirtualFrameRecord {
                    sequence,
                    source,
                    target,
                    outcome: VirtualFrameOutcome::Unavailable,
                });
                return Err(VirtualMembershipNetworkError::Offline);
            }
            let source_device_id = state
                .nodes
                .get(source)
                .expect("validated source node")
                .device_id
                .clone();
            let endpoint = Arc::clone(
                &state
                    .nodes
                    .get(target)
                    .expect("validated target node")
                    .endpoint,
            );
            (source_device_id, endpoint, sequence)
        };

        let result = endpoint
            .handle_membership_history_exchange(&source_device_id, message)
            .await;
        let outcome = match &result {
            Ok(_) => VirtualFrameOutcome::Accepted,
            Err(MembershipHistoryExchangeError::Offline) => VirtualFrameOutcome::Unavailable,
            Err(MembershipHistoryExchangeError::Rejected) => VirtualFrameOutcome::Rejected,
            Err(MembershipHistoryExchangeError::PairingInProgress) => VirtualFrameOutcome::Rejected,
            Err(MembershipHistoryExchangeError::Transport) => VirtualFrameOutcome::Invalid,
        };
        self.state
            .lock()
            .expect("virtual network is available")
            .trace
            .push(VirtualFrameRecord {
                sequence,
                source,
                target,
                outcome,
            });
        result.map_err(|error| match error {
            MembershipHistoryExchangeError::Offline => VirtualMembershipNetworkError::Offline,
            MembershipHistoryExchangeError::Rejected => VirtualMembershipNetworkError::Rejected,
            MembershipHistoryExchangeError::PairingInProgress => {
                VirtualMembershipNetworkError::PairingInProgress
            }
            MembershipHistoryExchangeError::Transport => VirtualMembershipNetworkError::Transport,
        })
    }

    pub(super) fn trace(&self) -> Vec<VirtualFrameRecord> {
        self.state
            .lock()
            .expect("virtual network is available")
            .trace
            .clone()
    }
}

fn ensure_nodes(
    state: &VirtualMembershipNetworkState,
    source: &'static str,
    target: &'static str,
) -> Result<(), VirtualMembershipNetworkError> {
    if state.nodes.contains_key(source) && state.nodes.contains_key(target) {
        Ok(())
    } else {
        Err(VirtualMembershipNetworkError::UnknownNode)
    }
}

fn valid_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 32
        && label
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use uc_core::membership::{
        AdmissionChangeFacts, MembershipActivationBaselineV2, MembershipCredential,
        MembershipEventId, MembershipHistoryAckV3, MembershipHistoryExchangeEndpointPort,
        MembershipHistoryMessage, MembershipHistorySummaryV3, VersionedMembershipHistory,
    };
    use uc_core::security::IdentityFingerprint;
    use uc_core::DeviceId;

    use super::{VirtualFrameOutcome, VirtualMembershipNetwork, VirtualMembershipNetworkError};
    use crate::space::membership::testing::{started_record, FixedSpaceWorkMode, OwnerFixture};
    use crate::space::membership::HandleMembershipHistoryMessageUseCase;
    use crate::test_support::membership_scenario::{finish, require, scenario};

    const REPRODUCE: &str =
        "cargo nextest run -p uc-application -E 'test(two_member_nodes_partition_and_heal)'";

    #[tokio::test]
    async fn two_member_nodes_partition_and_heal() {
        let scenario = scenario(
            "two-member-history-partition-heal",
            0x0040_3401,
            Duration::from_secs(1),
            REPRODUCE,
        );
        let result = async {
            let nodes = TwoMemberHistoryScenario::prepare(&scenario, 3)?;

            let first = nodes
                .exchange()
                .await
                .map_err(|_| fixture_failure("open-delivery"))?;
            require(
                matches!(
                    first,
                    MembershipHistoryMessage::AckV3(MembershipHistoryAckV3::Confirmed { .. })
                ),
                "open-link-did-not-confirm-history",
            )?;

            nodes.partition()?;
            let blocked = nodes.exchange().await;
            require(
                matches!(blocked, Err(VirtualMembershipNetworkError::Offline)),
                "partition-did-not-block-delivery",
            )?;

            nodes.heal()?;
            let healed = nodes
                .exchange()
                .await
                .map_err(|_| fixture_failure("healed-delivery"))?;
            require(
                matches!(
                    healed,
                    MembershipHistoryMessage::AckV3(MembershipHistoryAckV3::Confirmed { .. })
                ),
                "healed-link-did-not-confirm-history",
            )?;

            let trace = nodes.trace();
            require(trace.len() == 3, "trace-frame-count")?;
            require(
                trace
                    .iter()
                    .map(|frame| frame.outcome())
                    .collect::<Vec<_>>()
                    == vec![
                        VirtualFrameOutcome::Accepted,
                        VirtualFrameOutcome::Unavailable,
                        VirtualFrameOutcome::Accepted,
                    ],
                "trace-outcome-order",
            )?;
            require(
                !format!("{trace:?}").contains("device-a")
                    && !format!("{trace:?}").contains("device-b"),
                "trace-leaked-device-identity",
            )?;
            require(
                matches!(
                    nodes.exchange().await,
                    Err(VirtualMembershipNetworkError::FrameBudgetExceeded)
                ),
                "frame-budget-was-not-enforced",
            )?;
            scenario.record_event("frame-budget-enforced");
            Ok(())
        }
        .await;
        finish(scenario, result);
    }

    fn fixture_failure(condition: &'static str) -> uc_testkit::ScenarioFailure {
        uc_testkit::ScenarioFailure::new(uc_testkit::FailureKind::FixtureInvalid, condition)
    }

    struct TwoMemberHistoryScenario<'a> {
        scenario: &'a uc_testkit::Scenario,
        network: VirtualMembershipNetwork,
        history: HistoryFixture,
    }

    impl<'a> TwoMemberHistoryScenario<'a> {
        fn prepare(
            scenario: &'a uc_testkit::Scenario,
            max_frames: usize,
        ) -> Result<Self, uc_testkit::ScenarioFailure> {
            let _stage = scenario.stage("prepare-two-member-history");
            let history = HistoryFixture::new();
            let network = VirtualMembershipNetwork::new(max_frames);
            network
                .register(
                    "node-a",
                    history.device_a.clone(),
                    Arc::clone(&history.endpoint_a),
                )
                .map_err(|_| fixture_failure("register-node-a"))?;
            network
                .register(
                    "node-b",
                    history.device_b.clone(),
                    Arc::clone(&history.endpoint_b),
                )
                .map_err(|_| fixture_failure("register-node-b"))?;
            scenario.record_event("two-member-history-ready");
            Ok(Self {
                scenario,
                network,
                history,
            })
        }

        async fn exchange(
            &self,
        ) -> Result<MembershipHistoryMessage, VirtualMembershipNetworkError> {
            let _stage = self.scenario.stage("exchange-membership-history");
            let result = self
                .network
                .send("node-a", "node-b", self.history.summary())
                .await;
            match &result {
                Ok(_) => self.scenario.record_event("membership-history-accepted"),
                Err(VirtualMembershipNetworkError::Offline) => {
                    self.scenario.record_event("membership-history-unavailable")
                }
                Err(VirtualMembershipNetworkError::FrameBudgetExceeded) => self
                    .scenario
                    .record_event("membership-frame-budget-exceeded"),
                Err(_) => self.scenario.record_event("membership-history-failed"),
            }
            result
        }

        fn partition(&self) -> Result<(), uc_testkit::ScenarioFailure> {
            self.network
                .partition("node-a", "node-b")
                .map_err(|_| fixture_failure("partition"))?;
            self.scenario.record_event("membership-link-partitioned");
            Ok(())
        }

        fn heal(&self) -> Result<(), uc_testkit::ScenarioFailure> {
            self.network
                .heal("node-a", "node-b")
                .map_err(|_| fixture_failure("heal"))?;
            self.scenario.record_event("membership-link-healed");
            Ok(())
        }

        fn trace(&self) -> Vec<super::VirtualFrameRecord> {
            self.network.trace()
        }
    }

    struct HistoryFixture {
        device_a: DeviceId,
        device_b: DeviceId,
        sender: AdmissionChangeFacts,
        history: VersionedMembershipHistory,
        endpoint_a: Arc<dyn MembershipHistoryExchangeEndpointPort>,
        endpoint_b: Arc<dyn MembershipHistoryExchangeEndpointPort>,
    }

    impl HistoryFixture {
        fn new() -> Self {
            let (sender, sender_credential) = member_facts("device-a", 0x41);
            let (receiver, receiver_credential) = member_facts("device-b", 0x42);
            let history = VersionedMembershipHistory::from_activation_baseline(
                MembershipActivationBaselineV2::Established {
                    lineage_id: "space-a".to_owned(),
                    head_event_id: MembershipEventId::from_hex(&"11".repeat(32))
                        .expect("valid event id"),
                    head_depth: 0,
                    current_members: vec![
                        (sender.clone(), sender_credential),
                        (receiver.clone(), receiver_credential),
                    ],
                },
            )
            .expect("valid shared history");
            Self {
                device_a: sender.device_id.clone(),
                device_b: receiver.device_id.clone(),
                endpoint_a: endpoint(history.clone(), &sender),
                endpoint_b: endpoint(history.clone(), &receiver),
                sender,
                history,
            }
        }

        fn summary(&self) -> MembershipHistoryMessage {
            let position = self.history.current_position().expect("history position");
            MembershipHistoryMessage::SummaryV3(MembershipHistorySummaryV3 {
                lineage_id: self.history.lineage_id().to_owned(),
                current_position: position.clone(),
                transfer_id: position.history_digest,
                sender_admission: self.sender.clone(),
            })
        }
    }

    fn endpoint(
        history: VersionedMembershipHistory,
        local: &AdmissionChangeFacts,
    ) -> Arc<dyn MembershipHistoryExchangeEndpointPort> {
        let fixture = OwnerFixture::new(started_record(
            history,
            local.device_id,
            local.member_instance,
            1,
        ));
        Arc::new(HandleMembershipHistoryMessageUseCase::new(
            fixture.owner,
            FixedSpaceWorkMode::active(),
        ))
    }

    fn member_facts(
        device: &str,
        credential_byte: u8,
    ) -> (AdmissionChangeFacts, MembershipCredential) {
        let credential = MembershipCredential::new(1, vec![credential_byte; 32]);
        let device_id = DeviceId::new(device);
        let facts = AdmissionChangeFacts {
            member_instance: credential.member_instance_id(&device_id),
            device_id,
            device_name: "Scenario device".to_owned(),
            identity_fingerprint: IdentityFingerprint::from_display_string("ABCD-EFGH-IJKL-MNOP")
                .expect("valid fingerprint"),
            transport_public_key: vec![0x53; 32],
            transport_address_blob: vec![0x54; 32],
            identity_signature: vec![0x55; 64],
        };
        (facts, credential)
    }
}
