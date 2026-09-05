#![cfg(feature = "dev-tools")]

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value as OtlpValue;
use prost::Message;
use tempfile::TempDir;
use uc_engine::{
    ChooseDeviceGroupInput, CreateSpaceInput, Engine, EngineConfig, HistoryEntryInput,
    HostCapabilities, HostCapabilityError, HostCapabilityErrorCategory, HostClipboard,
    HostClipboardSnapshot, HostDirectories, HostFileAccess, HostFileHandle, HostFileMetadata,
    HostSecureStorage, JoinSpaceInput, JoinSpaceStatusSummary, ListHistoryEntriesInput, Operation,
    OperationResult, RemoveMemberInput, SecretString, SendTextInput,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const PASSPHRASE: &str = "space-membership-e2e-passphrase";
const WAIT_TIMEOUT: Duration = Duration::from_secs(60);
const ADMISSION_WAIT_TIMEOUT: Duration = Duration::from_secs(120);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(15);
const EXPIRES_AT_MS: i64 = 2_000_000_000_000;
const PAIRING_HOT_PATH_BUDGET: Duration = Duration::from_secs(1);

#[derive(Clone, Default)]
struct MemorySecureStorage(Arc<Mutex<HashMap<String, Vec<u8>>>>);

impl MemorySecureStorage {
    fn values(&self) -> MutexGuard<'_, HashMap<String, Vec<u8>>> {
        match self.0.lock() {
            Ok(values) => values,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

impl HostSecureStorage for MemorySecureStorage {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, HostCapabilityError> {
        Ok(self.values().get(key).cloned())
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<(), HostCapabilityError> {
        self.values().insert(key.to_owned(), value.to_vec());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), HostCapabilityError> {
        self.values().remove(key);
        Ok(())
    }
}

struct EmptyClipboard;

impl HostClipboard for EmptyClipboard {
    fn read(&self) -> Result<HostClipboardSnapshot, HostCapabilityError> {
        Ok(HostClipboardSnapshot {
            observed_at_ms: 0,
            representations: Vec::new(),
        })
    }

    fn write(&self, _snapshot: HostClipboardSnapshot) -> Result<(), HostCapabilityError> {
        Ok(())
    }
}

struct EmptyFiles;

impl HostFileAccess for EmptyFiles {
    fn metadata(&self, _handle: &HostFileHandle) -> Result<HostFileMetadata, HostCapabilityError> {
        Err(HostCapabilityError::new(
            HostCapabilityErrorCategory::InvalidHandle,
            "missing test file",
        ))
    }

    fn read_chunk(
        &self,
        _handle: &HostFileHandle,
        _offset: u64,
        _max_bytes: u32,
    ) -> Result<Vec<u8>, HostCapabilityError> {
        Ok(Vec::new())
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

struct DeviceHarness {
    root: TempDir,
    secure_storage: MemorySecureStorage,
    rendezvous_base_url: String,
}

impl DeviceHarness {
    fn new(rendezvous_base_url: String) -> Self {
        Self {
            root: TempDir::new().expect("create device directory"),
            secure_storage: MemorySecureStorage::default(),
            rendezvous_base_url,
        }
    }

    async fn start(&self) -> Engine {
        let root = self.root.path();
        let host = HostCapabilities::new(
            HostDirectories::new(
                root.join("private"),
                root.join("cache"),
                root.join("temporary"),
                root.join("logs"),
            ),
            Box::new(self.secure_storage.clone()),
            Box::new(EmptyClipboard),
            Box::new(EmptyFiles),
        );
        let config = EngineConfig::new("1.1.0")
            .with_rendezvous_base_url(self.rendezvous_base_url.clone())
            .with_test_relay_fallback(true);
        let (engine, _events) = Engine::start(config, host)
            .await
            .expect("start complete engine");
        engine
    }
}

#[derive(Default)]
struct TicketState {
    next_code: u16,
    tickets: HashMap<String, String>,
}

type TicketVault = Arc<Mutex<TicketState>>;

struct CreatePairing(TicketVault);

impl Respond for CreatePairing {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: serde_json::Value =
            serde_json::from_slice(&request.body).expect("pairing create request must be JSON");
        let ticket = body["sponsorTicket"]
            .as_str()
            .expect("sponsor ticket missing")
            .to_owned();
        assert!(
            ticket.starts_with("ucspace1_"),
            "directory must store a full admission invitation"
        );
        let mut state = lock_ticket_vault(&self.0);
        state.next_code += 1;
        let code = ticket.clone();
        state.tickets.insert(code.clone(), ticket);
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": code,
            "expiresAtMs": EXPIRES_AT_MS,
        }))
    }
}

struct ResolvePairing(TicketVault);

impl Respond for ResolvePairing {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: serde_json::Value =
            serde_json::from_slice(&request.body).expect("pairing resolve request must be JSON");
        let code = body["code"].as_str().expect("pairing code missing");
        let state = lock_ticket_vault(&self.0);
        let ticket = state
            .tickets
            .get(code)
            .or_else(|| state.tickets.values().next())
            .cloned()
            .expect("pairing ticket was not registered");
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "sponsorTicket": ticket,
            "sponsorEndpointId": "local-e2e",
            "expiresAtMs": EXPIRES_AT_MS,
        }))
    }
}

fn lock_ticket_vault(vault: &TicketVault) -> MutexGuard<'_, TicketState> {
    match vault.lock() {
        Ok(state) => state,
        Err(poisoned) => poisoned.into_inner(),
    }
}

async fn mount_rendezvous() -> MockServer {
    let server = MockServer::start().await;
    let vault = Arc::new(Mutex::new(TicketState::default()));
    Mock::given(method("POST"))
        .and(path("/v1/pairings"))
        .respond_with(CreatePairing(Arc::clone(&vault)))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/pairings/resolve"))
        .respond_with(ResolvePairing(vault))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/pairings/consume"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    server
}

#[derive(Debug, Clone, Copy)]
enum TopologyAction<'a> {
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

#[derive(Debug, Clone, Copy)]
enum PendingChangeChoice {
    Apply,
    Keep,
}

// F7：十节点不平衡树形成三个 sibling 后，冲突 peer 不得饿死合法 peer 的反熵。
#[tokio::test(flavor = "multi_thread", worker_threads = 16)]
async fn f7_three_sibling_branches_keep_fair_anti_entropy_for_legal_peers() {
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
                    topology.wait_for_paired_peer(sender, receiver).await;
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
                report.total_accepted,
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
        wait_for_peer_refresh(topology.engine(node), node).await;
    }
    let recovered = topology.diagnostics("F").await;
    assert_eq!(recovered.branch_id, target.branch_id);
    assert_eq!(recovered.head_event_id, target.head_event_id);

    for (sender, receiver) in [("A", "C"), ("C", "E"), ("E", "F")] {
        topology.wait_for_paired_peer(sender, receiver).await;
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
            wait_for_peer_refresh(topology.engine(node), node).await;
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

// F3：同一远端移除被不同设备接受和拒绝后，决定必须跨重启持久并保持内容隔离。
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn f3_opposite_removal_decisions_persist_divergence_across_restart() {
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
    topology
        .wait_for_group_epoch(&["B"], topology.diagnostics("A").await.group_epoch)
        .await;

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
    topology
        .run(&[TopologyAction::ResolveConflict {
            node: "C",
            branch_from: "B",
        }])
        .await;
    topology.wait_for_equivalent_branch(&["B", "C"], 4).await;
    topology.assert_snapshot("C", 4, 0).await;
    let selected_after_recovery = topology.diagnostics("B").await;
    topology
        .wait_for_group_epoch(&["C"], selected_after_recovery.group_epoch)
        .await;
    let resolved = topology.diagnostics("C").await;
    assert_eq!(resolved.branch_id, selected.branch_id);
    assert_eq!(resolved.head_event_id, selected.head_event_id);
    assert_eq!(resolved.group_epoch, selected_after_recovery.group_epoch);
    topology.shutdown().await;
}

// F1：共同父 head 上并发移除与新增，两个合法分支必须保持各自成员语义。
#[tokio::test(flavor = "multi_thread", worker_threads = 10)]
async fn f1_remove_and_add_from_parent_head_preserve_branch_membership() {
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

    topology
        .run(&[
            TopologyAction::Partition {
                left: &["A", "C", "D"],
                right: &["B", "E"],
            },
            TopologyAction::Join {
                sponsor: "B",
                joiner: "E",
            },
            TopologyAction::Remove {
                sponsor: "A",
                target: "D",
            },
        ])
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
        wait_for_peer_refresh(topology.engine(node), node).await;
    }
    topology.assert_snapshot("A", 3, 1).await;
    topology.assert_snapshot("B", 5, 1).await;
    let choices_a = topology.device_group_choices("A").await;
    let choices_b = topology.device_group_choices("B").await;
    let d_device_id = topology.device_ids.get("D").unwrap();
    let e_device_id = topology.device_ids.get("E").unwrap();
    assert_eq!(
        choices_a
            .device_trust
            .devices
            .iter()
            .find(|device| &device.device_id == d_device_id)
            .map(|device| device.membership),
        Some(uc_engine::DeviceMembershipSummary::Removed)
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

struct MembershipTopology {
    rendezvous_base_url: String,
    harnesses: HashMap<String, DeviceHarness>,
    engines: HashMap<String, Engine>,
    endpoint_ids_by_node: HashMap<String, [u8; 32]>,
    space_ids: HashMap<String, String>,
    device_ids: HashMap<String, String>,
}

impl MembershipTopology {
    fn new(rendezvous_base_url: String) -> Self {
        Self {
            rendezvous_base_url,
            harnesses: HashMap::new(),
            engines: HashMap::new(),
            endpoint_ids_by_node: HashMap::new(),
            space_ids: HashMap::new(),
            device_ids: HashMap::new(),
        }
    }

    async fn run(&mut self, actions: &[TopologyAction<'_>]) {
        for action in actions {
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

    async fn start(&mut self, node: &str) {
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

    async fn stop(&mut self, node: &str) {
        let engine = self
            .engines
            .remove(node)
            .unwrap_or_else(|| panic!("node {node} is not started"));
        engine
            .shutdown(SHUTDOWN_TIMEOUT)
            .await
            .unwrap_or_else(|error| panic!("node {node} shutdown failed: {error}"));
    }

    async fn restart(&mut self, node: &str) {
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

    async fn wait_for_membership_ready(&self, node: &str) {
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

    async fn create(&mut self, node: &str) {
        let engine = self.engine(node);
        let (space_id, device_id) = create_space(engine, node).await;
        self.space_ids.insert(node.to_owned(), space_id);
        self.device_ids.insert(node.to_owned(), device_id);
    }

    async fn join(&mut self, sponsor: &str, joiner: &str) {
        let invitation = issue_invitation(self.engine(sponsor)).await;
        self.join_with_invitation(sponsor, joiner, invitation).await;
    }

    async fn join_with_invitation(&mut self, sponsor: &str, joiner: &str, full_invitation: String) {
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

    async fn remove(&self, sponsor: &str, target: &str) {
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
            let result =
                result.unwrap_or_else(|error| panic!("node {sponsor} remove failed: {error}"));
            assert!(matches!(result, OperationResult::DeviceTrust(_)));
            return;
        }
    }

    async fn partition(&self, left: &[&str], right: &[&str]) {
        let left_ids = self.endpoint_ids(left).await;
        let right_ids = self.endpoint_ids(right).await;
        for node in left {
            self.set_partition(node, right_ids.clone()).await;
        }
        for node in right {
            self.set_partition(node, left_ids.clone()).await;
        }
    }

    async fn partition_groups(&self, groups: &[&[&str]], bridge: Option<(&str, &str)>) {
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

    async fn bridge(&self, left: &str, right: &str, left_group: &[&str], right_group: &[&str]) {
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

    async fn ring(&self, nodes: &[&str], isolated: &[&str]) {
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

    async fn chain(&self, nodes: &[&str], offline: &[&str]) {
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

    async fn heal(&self, nodes: &[&str]) {
        for node in nodes {
            self.set_partition(node, Vec::new()).await;
        }
    }

    async fn endpoint_ids(&self, nodes: &[&str]) -> Vec<[u8; 32]> {
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

    async fn set_partition(&self, node: &str, blocked_endpoint_ids: Vec<[u8; 32]>) {
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

    async fn send(&self, sender: &str, receiver: &str, text: &str) -> uc_engine::SendReportSummary {
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

    async fn wait_for_paired_peer(&self, sender: &str, receiver: &str) {
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
                        .any(|peer| &peer.peer_id == receiver_id && peer.is_paired) =>
                {
                    return;
                }
                Ok(OperationResult::PeerConnections(_)) => {}
                Ok(_) => panic!("node {sender} returned an unexpected peer connection result"),
                Err(error) if error.is_retryable() && tokio::time::Instant::now() < deadline => {}
                Err(error) => panic!("node {sender} peer connection query failed: {error}"),
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "same-branch peer {sender}-{receiver} did not become paired"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn diagnostics(&self, node: &str) -> uc_engine::MembershipDiagnosticsSummary {
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

    async fn device_group_choices(&self, node: &str) -> uc_engine::DeviceGroupChoicesSummary {
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

    async fn resolve_conflict(&self, node: &str, branch_from: &str) {
        let target = self.diagnostics(branch_from).await.branch_id;
        let choice_id = format!("b:{target}");
        let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
        loop {
            let choices = self.device_group_choices(node).await;
            let issue = choices
                .issues
                .iter()
                .find(|issue| issue.issue_id.starts_with("c:"))
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
                    assert!(
                        tokio::time::Instant::now() < deadline,
                        "node {node} conflict selection revision did not stabilize"
                    );
                }
                outcome => panic!("node {node} conflict selection returned {outcome:?}"),
            }
        }
    }

    async fn wait_for_pending_change(&self, nodes: &[&str]) {
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
            assert!(
                tokio::time::Instant::now() < deadline,
                "pending removal did not reach every decision node"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn wait_for_branch_conflict(&self, nodes: &[&str]) {
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
            assert!(
                tokio::time::Instant::now() < deadline,
                "branch conflict did not reach every bridge endpoint"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn wait_for_stable_pending_effects(&self, nodes: &[&str]) -> Vec<u32> {
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
            assert!(
                tokio::time::Instant::now() < deadline,
                "membership pending effects did not stabilize"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn decide_pending_change(&self, node: &str, choice: PendingChangeChoice) {
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
                    assert!(
                        tokio::time::Instant::now() < deadline,
                        "node {node} pending decision revision did not stabilize"
                    );
                }
                outcome => panic!("node {node} pending decision returned {outcome:?}"),
            }
        }
    }

    async fn wait_for_equivalent_branch(&self, nodes: &[&str], effective_members: u32) {
        self.wait_for_equivalent_branch_named(nodes, effective_members, "unnamed topology phase")
            .await;
    }

    async fn wait_for_equivalent_branch_named(
        &self,
        nodes: &[&str],
        effective_members: u32,
        phase: &str,
    ) {
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
            }) {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "nodes did not converge to the expected branch during {phase}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn wait_for_group_epoch(&self, nodes: &[&str], expected_epoch: u64) {
        self.wait_for_group_epoch_named(nodes, expected_epoch, "unnamed topology phase")
            .await;
    }

    async fn wait_for_group_epoch_named(&self, nodes: &[&str], expected_epoch: u64, phase: &str) {
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
            assert!(
                tokio::time::Instant::now() < deadline,
                "nodes did not reach group epoch {expected_epoch} during {phase}; observed={observed_epochs:?}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn assert_snapshot(
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
            assert!(
                tokio::time::Instant::now() < deadline,
                "node {node} did not reach the expected public snapshot; last observation: {observation}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn assert_diagnostics(
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

    fn engine(&self, node: &str) -> &Engine {
        self.engines
            .get(node)
            .unwrap_or_else(|| panic!("node {node} is not started"))
    }

    async fn shutdown(&self) {
        for engine in self.engines.values() {
            engine
                .shutdown(SHUTDOWN_TIMEOUT)
                .await
                .expect("shut down topology node");
        }
    }
}

// 分区门必须同时拒绝新连接并关闭已存在连接；Heal 后使用同一 Engine 恢复通信。
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn topology_partition_blocks_all_iroh_channels_until_healed() {
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
        wait_for_peer_refresh(topology.engine(node), node).await;
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
async fn topology_script_builds_a_two_node_space_through_public_operations() {
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
#[ignore = "显式重启链路验收：独占进程级 OTLP receiver，使用 --ignored 精确运行"]
async fn in_flight_admission_restart_uses_new_traces_and_one_flow() {
    let telemetry = MockServer::start().await;
    for endpoint in ["/v1/traces", "/v1/logs"] {
        Mock::given(method("POST"))
            .and(path(endpoint))
            .respond_with(ResponseTemplate::new(200))
            .mount(&telemetry)
            .await;
    }
    assert!(uc_engine::init_test_tracing_with_otlp(
        &format!("{}/v1/traces", telemetry.uri()),
        &format!("{}/v1/logs", telemetry.uri()),
    ));

    let rendezvous = mount_rendezvous().await;
    let sponsor_harness = DeviceHarness::new(rendezvous.uri());
    let joiner_harness = DeviceHarness::new(rendezvous.uri());
    let sponsor = sponsor_harness.start().await;
    let joiner = joiner_harness.start().await;
    let space_id = create_space(&sponsor, "Sponsor").await.0;
    let invitation = issue_invitation(&sponsor).await;
    let OperationResult::JoinSpace(status) = joiner
        .execute(Operation::JoinSpace(JoinSpaceInput {
            invitation_code: invitation,
            device_name: Some("Restarted Joiner".to_owned()),
            passphrase: SecretString::new(PASSPHRASE),
            preserve_unreadable_history: false,
        }))
        .await
        .expect("start restartable admission")
    else {
        panic!("unexpected join result");
    };
    assert!(matches!(&status, JoinSpaceStatusSummary::Pending { .. }));

    let evidence_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let (flow_id, before_restart_traces) = loop {
        uc_engine::flush_test_tracing();
        let requests = telemetry
            .received_requests()
            .await
            .expect("pre-restart OTLP requests");
        let flows = complete_admission_flow_traces(&requests);
        if flows.len() == 1 {
            let (flow_id, traces) = flows.into_iter().next().expect("one admission flow");
            if !traces.is_empty() {
                break (
                    flow_id,
                    traces
                        .into_iter()
                        .map(|trace| trace.trace_id)
                        .collect::<BTreeSet<_>>(),
                );
            }
        }
        assert!(
            tokio::time::Instant::now() < evidence_deadline,
            "admission emitted no pre-restart trace"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    let OperationResult::DeviceGroupChoices(before_restart) = joiner
        .execute(Operation::QueryDeviceGroupChoices)
        .await
        .expect("query in-flight admission")
    else {
        panic!("unexpected device trust result");
    };
    assert!(matches!(
        before_restart.device_trust.current_join,
        Some(JoinSpaceStatusSummary::Pending { .. })
    ));

    joiner
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("stop in-flight joiner");
    let restarted_at_ns = unix_time_ns(SystemTime::now());
    let restarted_joiner = joiner_harness.start().await;
    wait_for_completed_join(&restarted_joiner, "Restarted Joiner", status, &space_id).await;
    wait_for_active_member_count(&sponsor, 2).await;
    wait_for_active_member_count(&restarted_joiner, 2).await;
    sponsor
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("stop sponsor");
    restarted_joiner
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("stop restarted joiner");
    uc_engine::flush_test_tracing();

    let requests = telemetry
        .received_requests()
        .await
        .expect("post-restart OTLP requests");
    let flows = complete_admission_flow_traces(&requests);
    assert_eq!(flows.len(), 1);
    let after_restart_traces = flows.get(&flow_id).expect("same flow after restart");
    assert!(
        after_restart_traces
            .iter()
            .any(|trace| trace.started_at_ns >= restarted_at_ns
                && !before_restart_traces.contains(&trace.trace_id)),
        "restart produced no new complete online trace"
    );
}

// 新设备只经过稳定 JoinSpace 入口，并最终形成可查询的活动 Space。
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "显式性能门禁：需要空闲的本机真实网络栈，使用 --ignored 运行"]
async fn two_device_hot_path_pairing_completes_within_one_second() {
    let benchmark_mode = std::env::var("UC_PAIRING_OBSERVABILITY_BENCHMARK").ok();
    let telemetry = if matches!(
        benchmark_mode.as_deref(),
        Some("none" | "disabled" | "unavailable")
    ) {
        None
    } else {
        let telemetry = MockServer::start().await;
        for endpoint in ["/v1/traces", "/v1/logs"] {
            Mock::given(method("POST"))
                .and(path(endpoint))
                .respond_with(ResponseTemplate::new(200))
                .mount(&telemetry)
                .await;
        }
        Some(telemetry)
    };
    match benchmark_mode.as_deref() {
        Some("none") => {}
        Some("disabled") => assert!(uc_engine::init_test_tracing_without_remote()),
        Some("unavailable") => assert!(uc_engine::init_test_tracing_with_otlp(
            "http://127.0.0.1:1/v1/traces",
            "http://127.0.0.1:1/v1/logs",
        )),
        Some("healthy") | None => {
            let telemetry = telemetry.as_ref().expect("healthy OTLP receiver");
            assert!(uc_engine::init_test_tracing_with_otlp(
                &format!("{}/v1/traces", telemetry.uri()),
                &format!("{}/v1/logs", telemetry.uri()),
            ));
        }
        Some(other) => panic!("unknown observability benchmark mode: {other}"),
    }
    let rendezvous = mount_rendezvous().await;
    let sponsor_harness = DeviceHarness::new(rendezvous.uri());
    let joiner_harness = DeviceHarness::new(rendezvous.uri());
    let sponsor = sponsor_harness.start().await;
    let joiner = joiner_harness.start().await;
    let space_id = create_space(&sponsor, "Sponsor").await.0;
    let invitation = issue_invitation(&sponsor).await;

    let started_at = SystemTime::now();
    let started = Instant::now();
    join_with_invitation(&joiner, "Joiner", &space_id, invitation).await;
    wait_for_active_member_count(&sponsor, 2).await;
    wait_for_active_member_count(&joiner, 2).await;
    let elapsed = started.elapsed();

    sponsor
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shut down sponsor");
    joiner
        .shutdown(SHUTDOWN_TIMEOUT)
        .await
        .expect("shut down joiner");
    uc_engine::flush_test_tracing();
    if let Some(mode) = benchmark_mode {
        eprintln!(
            "UC_PAIRING_OBSERVABILITY_RESULT mode={mode} elapsed_us={}",
            elapsed.as_micros()
        );
        assert!(elapsed < Duration::from_secs(30));
        return;
    }
    let telemetry = telemetry.expect("performance gate OTLP receiver");
    let evidence_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let trace_evidence = loop {
        uc_engine::flush_test_tracing();
        let requests = telemetry
            .received_requests()
            .await
            .expect("OTLP request capture");
        let evidence = pairing_trace_evidence(&requests, started_at, elapsed);
        if evidence.client_count >= 4
            && evidence.paired_server_count >= 4
            && evidence.paired_admission_endpoint_count >= evidence.client_count
            && evidence.invalid_completion_log_count == 0
        {
            break evidence;
        }
        assert!(
            tokio::time::Instant::now() < evidence_deadline,
            "OTLP receiver did not collect four complete admission exchanges"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    assert!(trace_evidence.client_count >= 4);
    assert!(trace_evidence.paired_server_count >= 4);
    assert_eq!(
        trace_evidence.paired_admission_endpoint_count,
        trace_evidence.client_count
    );
    assert_eq!(trace_evidence.invalid_pair_count, 0);
    assert_eq!(trace_evidence.invalid_completion_log_count, 0);
    assert_eq!(trace_evidence.flow_ids.len(), 1);
    assert!(trace_evidence.admission_trace_ids.len() >= 4);
    assert!(
        trace_evidence.local_elapsed < PAIRING_HOT_PATH_BUDGET,
        "two-device local pairing work took {:?}, network-related time {:?}, end-to-end {:?}, local budget {:?}",
        trace_evidence.local_elapsed,
        trace_evidence.network_elapsed,
        elapsed,
        PAIRING_HOT_PATH_BUDGET,
    );
}

struct PairingTraceEvidence {
    local_elapsed: Duration,
    network_elapsed: Duration,
    client_count: usize,
    paired_server_count: usize,
    paired_admission_endpoint_count: usize,
    invalid_pair_count: usize,
    invalid_completion_log_count: usize,
    flow_ids: BTreeSet<String>,
    admission_trace_ids: BTreeSet<Vec<u8>>,
}

fn pairing_trace_evidence(
    requests: &[Request],
    started_at: SystemTime,
    elapsed: Duration,
) -> PairingTraceEvidence {
    let started_ns = u64::try_from(
        started_at
            .duration_since(UNIX_EPOCH)
            .expect("wall clock after epoch")
            .as_nanos(),
    )
    .unwrap_or(u64::MAX);
    let finished_ns =
        started_ns.saturating_add(u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX));
    let mut spans = Vec::new();
    for request in requests
        .iter()
        .filter(|request| request.url.path() == "/v1/traces")
    {
        let batch = ExportTraceServiceRequest::decode(request.body.as_slice())
            .expect("decode pairing trace batch");
        spans.extend(
            batch
                .resource_spans
                .into_iter()
                .flat_map(|resource| resource.scope_spans)
                .flat_map(|scope| scope.spans),
        );
    }
    let mut completion_logs = HashMap::<(Vec<u8>, Vec<u8>), usize>::new();
    let mut log_flow_violation_count = 0;
    for request in requests
        .iter()
        .filter(|request| request.url.path() == "/v1/logs")
    {
        let batch = ExportLogsServiceRequest::decode(request.body.as_slice())
            .expect("decode pairing log batch");
        for log in batch
            .resource_logs
            .into_iter()
            .flat_map(|resource| resource.scope_logs)
            .flat_map(|scope| scope.log_records)
        {
            if otlp_string_attribute(&log.attributes, "event.name")
                != Some("uc.operation.completed")
                || otlp_string_attribute(&log.attributes, "uc.domain") != Some("space_admission")
            {
                continue;
            }
            if otlp_string_attribute(&log.attributes, "uc.flow.id").is_some() {
                log_flow_violation_count += 1;
            }
            *completion_logs
                .entry((log.trace_id, log.span_id))
                .or_default() += 1;
        }
    }

    let relevant = spans
        .iter()
        .filter(|span| {
            otlp_string_attribute(&span.attributes, "uc.domain") == Some("space_admission")
                && otlp_string_attribute(&span.attributes, "uc.operation")
                    == Some("network_transport")
        })
        .collect::<Vec<_>>();
    let client_kind = opentelemetry_proto::tonic::trace::v1::span::SpanKind::Client as i32;
    let server_kind = opentelemetry_proto::tonic::trace::v1::span::SpanKind::Server as i32;
    let internal_kind = opentelemetry_proto::tonic::trace::v1::span::SpanKind::Internal as i32;
    let clients = relevant
        .iter()
        .copied()
        .filter(|span| span.kind == client_kind)
        .collect::<Vec<_>>();
    let client_ids = clients
        .iter()
        .map(|span| ((span.trace_id.clone(), span.span_id.clone()), *span))
        .collect::<HashMap<_, _>>();
    let paired_servers = relevant
        .iter()
        .filter_map(|span| {
            (span.kind == server_kind)
                .then(|| {
                    client_ids
                        .get(&(span.trace_id.clone(), span.parent_span_id.clone()))
                        .map(|client| (*client, *span))
                })
                .flatten()
        })
        .collect::<Vec<_>>();
    let server_ids = paired_servers
        .iter()
        .map(|(_, server)| ((server.trace_id.clone(), server.span_id.clone()), *server))
        .collect::<HashMap<_, _>>();
    let paired_admission_endpoints = spans
        .iter()
        .filter(|span| {
            span.name == "space_admission"
                && span.kind == internal_kind
                && otlp_string_attribute(&span.attributes, "uc.role") == Some("sponsor")
                && otlp_string_attribute(&span.attributes, "uc.flow.id").is_none()
                && server_ids.contains_key(&(span.trace_id.clone(), span.parent_span_id.clone()))
        })
        .collect::<Vec<_>>();
    let paired_admission_endpoint_count = paired_admission_endpoints.len();
    let invalid_completion_log_count = clients
        .iter()
        .copied()
        .chain(paired_servers.iter().map(|(_, server)| *server))
        .chain(paired_admission_endpoints.iter().copied())
        .filter(|span| {
            completion_logs
                .get(&(span.trace_id.clone(), span.span_id.clone()))
                .copied()
                != Some(1)
        })
        .count()
        + log_flow_violation_count;
    let flow_ids = relevant
        .iter()
        .filter_map(|span| otlp_string_attribute(&span.attributes, "uc.flow.id"))
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    let admission_trace_ids = clients
        .iter()
        .map(|span| span.trace_id.clone())
        .collect::<BTreeSet<_>>();
    let mut invalid_pair_count = clients
        .iter()
        .filter(|client| {
            !client.parent_span_id.is_empty()
                || otlp_string_attribute(&client.attributes, "uc.flow.id").is_none()
                || !otlp_span_succeeded(client)
        })
        .count()
        + paired_servers
            .iter()
            .filter(|(_, server)| {
                otlp_string_attribute(&server.attributes, "uc.flow.id").is_some()
                    || !otlp_span_succeeded(server)
            })
            .count()
        + spans
            .iter()
            .filter(|span| {
                span.name == "space_admission"
                    && span.kind == internal_kind
                    && otlp_string_attribute(&span.attributes, "uc.role") == Some("sponsor")
                    && server_ids
                        .contains_key(&(span.trace_id.clone(), span.parent_span_id.clone()))
                    && !otlp_span_succeeded(span)
            })
            .count();
    let mut network_intervals = paired_servers
        .iter()
        .flat_map(|(client, server)| {
            if client.start_time_unix_nano > server.start_time_unix_nano
                || server.end_time_unix_nano > client.end_time_unix_nano
            {
                invalid_pair_count += 1;
                return Vec::new();
            }
            [
                (client.start_time_unix_nano, server.start_time_unix_nano),
                (server.end_time_unix_nano, client.end_time_unix_nano),
            ]
            .into_iter()
            .filter_map(|(start, end)| {
                let start = start.max(started_ns);
                let end = end.min(finished_ns);
                (start < end).then_some((start, end))
            })
            .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    network_intervals.sort_unstable();
    let mut network_ns = 0_u64;
    let mut merged: Option<(u64, u64)> = None;
    for (start, end) in network_intervals {
        match merged {
            Some((current_start, current_end)) if start <= current_end => {
                merged = Some((current_start, current_end.max(end)));
            }
            Some((current_start, current_end)) => {
                network_ns = network_ns.saturating_add(current_end - current_start);
                merged = Some((start, end));
            }
            None => merged = Some((start, end)),
        }
    }
    if let Some((start, end)) = merged {
        network_ns = network_ns.saturating_add(end - start);
    }
    let network_elapsed = Duration::from_nanos(network_ns);
    PairingTraceEvidence {
        local_elapsed: elapsed.saturating_sub(network_elapsed),
        network_elapsed,
        client_count: clients.len(),
        paired_server_count: paired_servers.len(),
        paired_admission_endpoint_count,
        invalid_pair_count,
        invalid_completion_log_count,
        flow_ids,
        admission_trace_ids,
    }
}

struct CompleteAdmissionTrace {
    trace_id: Vec<u8>,
    started_at_ns: u64,
}

fn complete_admission_flow_traces(
    requests: &[Request],
) -> HashMap<String, Vec<CompleteAdmissionTrace>> {
    let mut spans = Vec::new();
    for request in requests
        .iter()
        .filter(|request| request.url.path() == "/v1/traces")
    {
        let batch = ExportTraceServiceRequest::decode(request.body.as_slice())
            .expect("decode admission trace batch");
        spans.extend(
            batch
                .resource_spans
                .into_iter()
                .flat_map(|resource| resource.scope_spans)
                .flat_map(|scope| scope.spans),
        );
    }
    let client_kind = opentelemetry_proto::tonic::trace::v1::span::SpanKind::Client as i32;
    let server_kind = opentelemetry_proto::tonic::trace::v1::span::SpanKind::Server as i32;
    let internal_kind = opentelemetry_proto::tonic::trace::v1::span::SpanKind::Internal as i32;
    let mut flows = HashMap::<String, Vec<CompleteAdmissionTrace>>::new();
    for client in spans.iter().filter(|span| {
        span.kind == client_kind
            && span.parent_span_id.is_empty()
            && otlp_span_succeeded(span)
            && otlp_string_attribute(&span.attributes, "uc.domain") == Some("space_admission")
            && otlp_string_attribute(&span.attributes, "uc.operation") == Some("network_transport")
            && otlp_string_attribute(&span.attributes, "uc.role") == Some("joiner")
    }) {
        let Some(flow_id) = otlp_string_attribute(&client.attributes, "uc.flow.id") else {
            continue;
        };
        let Some(server) = spans.iter().find(|span| {
            span.trace_id == client.trace_id
                && span.parent_span_id == client.span_id
                && span.kind == server_kind
                && otlp_string_attribute(&span.attributes, "uc.domain") == Some("space_admission")
                && otlp_string_attribute(&span.attributes, "uc.operation")
                    == Some("network_transport")
                && otlp_string_attribute(&span.attributes, "uc.role") == Some("sponsor")
                && otlp_string_attribute(&span.attributes, "uc.flow.id").is_none()
                && otlp_span_succeeded(span)
        }) else {
            continue;
        };
        let endpoint_exists = spans.iter().any(|span| {
            span.trace_id == server.trace_id
                && span.parent_span_id == server.span_id
                && span.kind == internal_kind
                && span.name == "space_admission"
                && otlp_string_attribute(&span.attributes, "uc.role") == Some("sponsor")
                && otlp_string_attribute(&span.attributes, "uc.flow.id").is_none()
                && otlp_span_succeeded(span)
        });
        if endpoint_exists {
            flows
                .entry(flow_id.to_owned())
                .or_default()
                .push(CompleteAdmissionTrace {
                    trace_id: client.trace_id.clone(),
                    started_at_ns: client.start_time_unix_nano,
                });
        }
    }
    flows
}

fn unix_time_ns(time: SystemTime) -> u64 {
    u64::try_from(
        time.duration_since(UNIX_EPOCH)
            .expect("wall clock after epoch")
            .as_nanos(),
    )
    .unwrap_or(u64::MAX)
}

fn otlp_span_succeeded(span: &opentelemetry_proto::tonic::trace::v1::Span) -> bool {
    span.status.as_ref().is_some_and(|status| {
        status.code == opentelemetry_proto::tonic::trace::v1::status::StatusCode::Ok as i32
    })
}

fn otlp_string_attribute<'a>(
    attributes: &'a [opentelemetry_proto::tonic::common::v1::KeyValue],
    key: &str,
) -> Option<&'a str> {
    attributes
        .iter()
        .find(|attribute| attribute.key == key)
        .and_then(|attribute| attribute.value.as_ref())
        .and_then(|value| value.value.as_ref())
        .and_then(|value| match value {
            OtlpValue::StringValue(value) => Some(value.as_str()),
            _ => None,
        })
}

async fn wait_for_active_member_count(engine: &Engine, expected: u32) {
    let deadline = tokio::time::Instant::now() + ADMISSION_WAIT_TIMEOUT;
    loop {
        if let Ok(OperationResult::MembershipDiagnostics(summary)) =
            engine.execute(Operation::QueryMembershipDiagnostics).await
        {
            if summary.effective_member_count == expected {
                return;
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "active member count did not reach {expected}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

// 新设备只经过稳定 JoinSpace 入口，并最终形成可查询的活动 Space。
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn fresh_device_join_completes_through_stable_operations() {
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
    uc_engine::init_test_tracing();
    let rendezvous = mount_rendezvous().await;
    let sponsor_harness = DeviceHarness::new(rendezvous.uri());
    let existing_harness = DeviceHarness::new(rendezvous.uri());
    let newcomer_harness = DeviceHarness::new(rendezvous.uri());
    let sponsor = sponsor_harness.start().await;
    let existing = existing_harness.start().await;
    let newcomer = newcomer_harness.start().await;
    let space_id = create_space(&sponsor, "Sponsor").await.0;

    join_through(&sponsor, &existing, "Existing Member", &space_id).await;
    wait_for_active_member_count(&sponsor, 2).await;
    wait_for_active_member_count(&existing, 2).await;
    let newcomer_id = join_through(&sponsor, &newcomer, "New Member", &space_id)
        .await
        .self_device_id;

    for engine in [&sponsor, &existing, &newcomer] {
        wait_for_active_member_count(engine, 3).await;
    }
    wait_for_peer_refresh(&existing, "existing member").await;
    wait_for_peer_refresh(&newcomer, "new member").await;
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

// 已完成设置的设备通过同一 JoinSpace 入口切换到另一个 Space。
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn existing_device_switches_space_through_stable_operations() {
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

// 加入完成后重启 Joiner，持久化准入状态必须足以恢复成员权限并接收正文。
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn completed_admission_survives_restart_and_allows_transfer() {
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

async fn wait_for_peer_refresh(engine: &Engine, label: &str) {
    let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
    loop {
        if engine
            .execute(Operation::RefreshPeerConnections)
            .await
            .is_ok()
        {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{label} peer connection refresh timed out"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn query_endpoint_id(engine: &Engine, node: &str) -> [u8; 32] {
    let result = engine
        .execute_dev(uc_engine::DevOperation::QueryNetworkEndpointId)
        .await
        .unwrap_or_else(|error| panic!("node {node} endpoint query failed: {error}"));
    let uc_engine::DevOperationResult::NetworkEndpointId(endpoint_id) = result else {
        panic!("node {node} returned an unexpected endpoint result");
    };
    endpoint_id
}

async fn create_space(engine: &Engine, device_name: &str) -> (String, String) {
    let created = match engine
        .execute(Operation::CreateSpace(CreateSpaceInput {
            device_name: Some(device_name.to_owned()),
            passphrase: SecretString::new(PASSPHRASE),
            passphrase_confirmation: SecretString::new(PASSPHRASE),
        }))
        .await
        .expect("create space")
    {
        OperationResult::SpaceCreated {
            space_id,
            self_device_id,
            ..
        } => (space_id, self_device_id),
        other => panic!("unexpected create result: {other:?}"),
    };
    let OperationResult::SetupState(setup) = engine
        .execute(Operation::QuerySetupState)
        .await
        .expect("query created space")
    else {
        panic!("unexpected setup result after create");
    };
    assert_eq!(setup.space_id.as_deref(), Some(created.0.as_str()));
    created
}

struct JoinResult {
    self_device_id: String,
}

async fn join_through(
    sponsor: &Engine,
    joiner: &Engine,
    device_name: &str,
    expected_space_id: &str,
) -> JoinResult {
    let full_invitation = issue_invitation(sponsor).await;
    join_with_invitation(joiner, device_name, expected_space_id, full_invitation).await
}

async fn issue_invitation(sponsor: &Engine) -> String {
    issue_invitation_named(sponsor, "sponsor").await
}

async fn issue_invitation_named(sponsor: &Engine, sponsor_label: &str) -> String {
    let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
    loop {
        match sponsor.execute(Operation::IssueInvitation).await {
            Ok(OperationResult::InvitationIssued {
                full_invitation, ..
            }) => return full_invitation,
            Ok(_) => panic!("unexpected invitation result"),
            Err(error) if error.is_retryable() && tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(error) => panic!(
                "node {sponsor_label} issue admission invitation: {error}; retryable={}",
                error.is_retryable()
            ),
        }
    }
}

async fn join_with_invitation(
    joiner: &Engine,
    device_name: &str,
    expected_space_id: &str,
    full_invitation: String,
) -> JoinResult {
    let OperationResult::JoinSpace(status) = joiner
        .execute(Operation::JoinSpace(JoinSpaceInput {
            invitation_code: full_invitation,
            device_name: Some(device_name.to_owned()),
            passphrase: SecretString::new(PASSPHRASE),
            preserve_unreadable_history: false,
        }))
        .await
        .expect("start admission join")
    else {
        panic!("unexpected join result");
    };
    wait_for_completed_join(joiner, device_name, status, expected_space_id).await
}

async fn wait_for_completed_join(
    engine: &Engine,
    device_name: &str,
    mut status: JoinSpaceStatusSummary,
    expected_space_id: &str,
) -> JoinResult {
    let deadline = tokio::time::Instant::now() + ADMISSION_WAIT_TIMEOUT;
    loop {
        match status {
            JoinSpaceStatusSummary::Active { joined_space, .. } => {
                assert_eq!(joined_space.space_id, expected_space_id);
                return JoinResult {
                    self_device_id: joined_space.self_device_id,
                };
            }
            JoinSpaceStatusSummary::Rejected { reason, .. } => {
                panic!("admission was rejected: {reason:?}")
            }
            JoinSpaceStatusSummary::Pending { .. } => {}
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "space admission timed out for {device_name}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
        let snapshot = match engine.execute(Operation::QueryDeviceGroupChoices).await {
            Ok(OperationResult::DeviceGroupChoices(summary)) => summary.device_trust,
            Ok(_) => panic!("unexpected device trust result"),
            Err(_) => continue,
        };
        if let Some(current_join) = snapshot.current_join {
            status = current_join;
            continue;
        }
        let setup = match engine.execute(Operation::QuerySetupState).await {
            Ok(OperationResult::SetupState(setup)) => setup,
            Ok(_) => panic!("unexpected setup state result"),
            Err(_) => continue,
        };
        if setup.has_completed && setup.space_id.as_deref() == Some(expected_space_id) {
            let device = match engine.execute(Operation::QueryLocalDevice).await {
                Ok(OperationResult::LocalDevice(device)) => device,
                Ok(_) => panic!("unexpected local device result"),
                Err(_) => continue,
            };
            return JoinResult {
                self_device_id: device.device_id,
            };
        }
    }
}

async fn wait_for_received_text(engine: &Engine, expected_text: &str) {
    let deadline = tokio::time::Instant::now() + WAIT_TIMEOUT;
    loop {
        if receiver_has_exact_text(engine, expected_text).await {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "content delivery timed out"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn receiver_has_exact_text(engine: &Engine, expected_text: &str) -> bool {
    let OperationResult::HistoryEntries(entries) = engine
        .execute(Operation::ListHistoryEntries(ListHistoryEntriesInput {
            limit: 100,
            offset: 0,
        }))
        .await
        .expect("list history entries")
    else {
        panic!("unexpected history list result");
    };
    for entry in entries {
        match engine
            .execute(Operation::GetHistoryEntry(HistoryEntryInput {
                entry_id: entry.entry_id,
            }))
            .await
            .expect("get history entry")
        {
            OperationResult::HistoryEntry(detail) if detail.content == expected_text => {
                return true
            }
            OperationResult::HistoryEntry(_) => {}
            other => panic!("unexpected history detail result: {other:?}"),
        }
    }
    false
}
