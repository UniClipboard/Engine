//! 成员移除协调器流程测试。
//!
//! 覆盖规格 015 的核心验收场景:创建与本机生效、验证与合并、迟到意图、
//! 状态推进、幂等、失败关闭与恢复编排。

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use uc_core::ids::DeviceId;
use uc_core::membership::{
    MemberInstanceId, RemovalAdmissionGatePort, RemovalCausalProof, RemovalCausalProofMember,
    RemovalCompletionReceipt, RemovalIntentContent, RemovalIntentRepositoryError,
    RemovalIntentRepositoryPort, RemovalLateAcceptance, RemovalLateRejectionReason,
    RemovalLateSubmission, RemovalPhase, RemovalRecoveryPersisted, RemovalRecoveryPort,
    SignedRemovalIntent,
};

use super::test_support::{
    AcceptingNoticeVerifier, ConfigurableVerifier, FakeRemovalRecovery, FixedSigner,
    MemoryMemberRepository, MemoryRemovalExchange, MemoryRemovalIntentRepository,
    MemoryRemovalLateExchange, MemoryRemovalNoticeExchange,
};
use super::{RemovalCoordinator, RemovalCoordinatorDeps, RemovalCoordinatorError};

#[derive(Clone, Default)]
struct CapturedWriter(Arc<Mutex<Vec<u8>>>);

impl CapturedWriter {
    fn dump(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

impl Write for CapturedWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedWriter {
    type Writer = CapturedWriter;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

fn device(name: &str) -> DeviceId {
    DeviceId::new(name)
}

fn instance(device_name: &str, key: u8) -> MemberInstanceId {
    MemberInstanceId::derive(device_name, &[key; 32])
}

fn sorted_view(members: Vec<MemberInstanceId>) -> Vec<MemberInstanceId> {
    let mut members = members;
    members.sort_unstable();
    members.dedup();
    members
}

fn proof_for(content: &RemovalIntentContent) -> RemovalCausalProof {
    let identities = [
        ("alice", 1_u8),
        ("bob", 2),
        ("charlie", 3),
        ("dave", 4),
        ("mallory", 5),
    ];
    RemovalCausalProof::new(
        content.view_epoch,
        content
            .view_members
            .iter()
            .enumerate()
            .map(|(index, member_instance)| {
                let (device_name, _) = identities
                    .iter()
                    .find(|(name, key)| instance(name, *key) == *member_instance)
                    .copied()
                    .unwrap_or(("unknown", index as u8));
                RemovalCausalProofMember {
                    device_id: device(device_name),
                    instance: *member_instance,
                    signing_public_key: member_instance.as_bytes().to_vec(),
                }
            })
            .collect(),
    )
}

fn signed_intent(content: RemovalIntentContent) -> SignedRemovalIntent {
    let proof = proof_for(&content);
    SignedRemovalIntent::new(content, vec![1], proof)
}

struct Harness {
    coordinator: Arc<RemovalCoordinator>,
    exchange: MemoryRemovalExchange,
    late_exchange: MemoryRemovalLateExchange,
    notice_exchange: MemoryRemovalNoticeExchange,
    notice_verifier: AcceptingNoticeVerifier,
    repository: MemoryRemovalIntentRepository,
    recovery: FakeRemovalRecovery,
    verifier: ConfigurableVerifier,
    member_repo: MemoryMemberRepository,
    signer: FixedSigner,
}

impl Harness {
    fn build(own_device: &DeviceId, members: Vec<(&str, u8)>) -> Self {
        let repository = MemoryRemovalIntentRepository::with_lineage("space-a");
        let exchange = MemoryRemovalExchange::default();
        let late_exchange = MemoryRemovalLateExchange::default();
        let notice_exchange = MemoryRemovalNoticeExchange::default();
        let verifier = ConfigurableVerifier::default();
        let member_repo = MemoryMemberRepository::default();
        let signer = FixedSigner::default();
        let members_vec = members
            .iter()
            .map(|(name, key)| (device(name), instance(name, *key)))
            .collect();
        let recovery = FakeRemovalRecovery::new(own_device.clone(), members_vec);
        let notice_verifier = AcceptingNoticeVerifier::default();
        let coordinator = Arc::new(RemovalCoordinator::new(RemovalCoordinatorDeps {
            repository: Arc::new(repository.clone()),
            verification: Arc::new(verifier.clone()),
            exchange: Arc::new(exchange.clone()),
            late_submission: Arc::new(late_exchange.clone()),
            notice: Arc::new(notice_exchange.clone()),
            notice_verification: Arc::new(notice_verifier.clone()),
            recovery: Arc::new(recovery.clone()),
            member_signatures: Arc::new(signer.clone()),
            member_repo: Arc::new(member_repo.clone()),
            own_device: own_device.clone(),
        }));
        for (name, _) in &members {
            member_repo.add(device(name));
        }
        Harness {
            coordinator,
            exchange,
            late_exchange,
            notice_exchange,
            notice_verifier,
            repository,
            recovery,
            verifier,
            member_repo,
            signer,
        }
    }

    fn restart(&self) -> Arc<RemovalCoordinator> {
        Arc::new(RemovalCoordinator::new(RemovalCoordinatorDeps {
            repository: Arc::new(self.repository.clone()),
            verification: Arc::new(self.verifier.clone()),
            exchange: Arc::new(self.exchange.clone()),
            late_submission: Arc::new(self.late_exchange.clone()),
            notice: Arc::new(self.notice_exchange.clone()),
            notice_verification: Arc::new(self.notice_verifier.clone()),
            recovery: Arc::new(self.recovery.clone()),
            member_signatures: Arc::new(self.signer.clone()),
            member_repo: Arc::new(self.member_repo.clone()),
            own_device: self.recovery.own_device.clone(),
        }))
    }
}

/// 模拟一次完整的意图投递(把 A 发过的所有消息交给 B 处理)。
async fn deliver_to(sender: &MemoryRemovalExchange, receiver: &RemovalCoordinator, now_ms: i64) {
    let messages = sender.sent.lock().unwrap().drain(..).collect::<Vec<_>>();
    for (recipient, message) in messages {
        let _ = receiver.ingest_exchange(&recipient, message, now_ms).await;
    }
}

/// 模拟一次同步请求-响应交换:发送方的每条消息经接收方处理后,
/// 接收方的响应回传给发送方(双方在线)。
async fn round_trip(sender: &Harness, receiver: &Harness, now_ms: i64) {
    let messages = sender
        .exchange
        .sent
        .lock()
        .unwrap()
        .drain(..)
        .collect::<Vec<_>>();
    let sender_device_id = sender.recovery.own_device.clone();
    let receiver_device_id = receiver.recovery.own_device.clone();
    for (_recipient, message) in messages {
        let response = receiver
            .coordinator
            .ingest_exchange(&sender_device_id, message, now_ms)
            .await
            .unwrap();
        let _ = sender
            .coordinator
            .ingest_exchange(&receiver_device_id, response, now_ms)
            .await;
    }
}

#[tokio::test]
async fn submitting_a_removal_applies_locally_and_reports_applied() {
    let alice = device("alice");
    let harness = Harness::build(&alice, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);
    let summary = harness
        .coordinator
        .submit_removal(&device("bob"), 1000)
        .await
        .unwrap();
    assert_eq!(summary.phase, RemovalPhase::Applied);
    assert_eq!(summary.intent_count, 1);
    assert_eq!(summary.effective_member_count, 2);
    assert!(summary.convergence_digest.is_some());
    let state = harness.repository.load_state().await.unwrap().unwrap();
    assert!(state.locally_removed.contains(&instance("bob", 2)));
    assert_eq!(state.causal_history.len(), 1);
    assert_eq!(harness.repository.atomic_save_count(), 1);
}

#[tokio::test]
async fn pending_removal_blocks_new_member_admission() {
    let alice = device("alice");
    let harness = Harness::build(&alice, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);
    harness
        .coordinator
        .submit_removal(&device("bob"), 1_000)
        .await
        .unwrap();

    assert_eq!(
        harness.coordinator.admission_decision(1_001).await,
        uc_core::membership::RemovalAdmissionDecision::AwaitingConvergence
    );
}

#[tokio::test]
async fn pending_removal_cannot_issue_a_new_invitation() {
    let alice = device("alice");
    let harness = Harness::build(&alice, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);
    harness
        .coordinator
        .submit_removal(&device("bob"), 1_000)
        .await
        .unwrap();

    assert_eq!(
        harness.coordinator.invitation_generation().await,
        Err(uc_core::membership::RemovalAdmissionDecision::AwaitingConvergence)
    );
}

#[tokio::test]
async fn completed_removal_rejects_old_invitation_generation_but_allows_a_new_one() {
    let alice = device("alice");
    let harness = Harness::build(&alice, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);
    harness
        .coordinator
        .submit_removal(&device("bob"), 1_000)
        .await
        .unwrap();

    let mut state = harness.repository.load_state().await.unwrap().unwrap();
    state.phase = RemovalPhase::Complete;
    state.applied_digest = Some(state.convergence().convergence_digest());
    harness.repository.save_state(&state).await.unwrap();

    assert_eq!(
        harness.coordinator.admission_decision(0).await,
        uc_core::membership::RemovalAdmissionDecision::SupersededInvitation
    );
    assert_eq!(
        harness.coordinator.admission_decision(1).await,
        uc_core::membership::RemovalAdmissionDecision::Allowed
    );
    assert_eq!(harness.coordinator.invitation_generation().await, Ok(1));
}

#[tokio::test]
async fn restart_after_intent_persistence_sends_the_same_intent() {
    // R01:首次发送前退出后，重启只能传播已保存的同一条意图。
    let alice = device("alice");
    let harness = Harness::build(&alice, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);
    harness
        .coordinator
        .submit_removal(&device("bob"), 1000)
        .await
        .unwrap();
    let intent_id = harness
        .repository
        .load_state()
        .await
        .unwrap()
        .unwrap()
        .intents[0]
        .intent_id;

    harness.restart().reconcile(1001).await.unwrap();
    let state = harness.repository.load_state().await.unwrap().unwrap();
    assert_eq!(state.intents.len(), 1);
    assert_eq!(state.intents[0].intent_id, intent_id);
    assert!(harness
        .exchange
        .sent
        .lock()
        .unwrap()
        .iter()
        .any(|(_, message)| {
            matches!(message, uc_core::membership::RemovalExchangeMessage::Intent(intent)
            if intent.intent_id == intent_id)
        }));
}

#[tokio::test]
async fn empty_removal_query_reports_the_current_member_count() {
    let alice = device("alice");
    let harness = Harness::build(&alice, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);

    let summary = harness.coordinator.query(1000).await.unwrap();

    assert_eq!(summary.phase, RemovalPhase::Applied);
    assert_eq!(summary.intent_count, 0);
    assert_eq!(summary.effective_member_count, 3);
    assert_eq!(summary.convergence_digest, None);
}

#[tokio::test]
async fn repeated_removal_of_the_same_target_is_idempotent() {
    // B04:连续重复移除同一目标只产生一个不可变意图。
    let alice = device("alice");
    let harness = Harness::build(&alice, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);
    let first = harness
        .coordinator
        .submit_removal(&device("bob"), 1000)
        .await
        .unwrap();
    let second = harness
        .coordinator
        .submit_removal(&device("bob"), 1001)
        .await
        .unwrap();
    assert_eq!(first.intent_count, 1);
    assert_eq!(second.intent_count, 1);
    assert_eq!(first.convergence_digest, second.convergence_digest);
}

#[tokio::test]
async fn normal_exchange_never_sends_to_a_locally_removed_member() {
    // B07/C12:本机保存移除后，普通成员通道不得再联系目标；目标只能使用
    // 受限迟交入口提交历史意图。
    let alice = device("alice");
    let bob = device("bob");
    let harness = Harness::build(&alice, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);
    harness
        .coordinator
        .submit_removal(&bob, 1000)
        .await
        .unwrap();

    harness.coordinator.reconcile(1001).await.unwrap();

    assert!(harness
        .exchange
        .sent
        .lock()
        .unwrap()
        .iter()
        .all(|(recipient, _)| recipient != &bob));
}

#[tokio::test]
async fn normal_exchange_rejects_a_member_already_removed_by_this_device() {
    // C12:已被移除的成员不能继续进入普通成员通道；即使它携带的历史意图
    // 本身看似合法，也必须改走受限迟交入口。
    let alice = device("alice");
    let bob = device("bob");
    let harness = Harness::build(&alice, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);
    harness
        .coordinator
        .submit_removal(&bob, 1000)
        .await
        .unwrap();
    let historical_intent = signed_intent(uc_core::membership::RemovalIntentContent {
        space_lineage: "space-a".to_owned(),
        view_epoch: 1,
        view_members: sorted_view(vec![
            instance("alice", 1),
            instance("bob", 2),
            instance("charlie", 3),
        ]),
        initiator: instance("bob", 2),
        target: instance("charlie", 3),
    });

    let error = harness
        .coordinator
        .ingest_exchange(
            &bob,
            uc_core::membership::RemovalExchangeMessage::Intent(Box::new(historical_intent)),
            1001,
        )
        .await
        .unwrap_err();

    assert!(matches!(error, RemovalCoordinatorError::OwnInstanceRemoved));
}

#[tokio::test]
async fn normal_exchange_rejects_an_unknown_device_before_offering_a_key_package() {
    // 普通入口只服务当前有效成员。未知设备即使猜中摘要，也不能得到恢复所需资料。
    let alice = device("alice");
    let harness = Harness::build(&alice, vec![("alice", 1), ("bob", 2)]);
    harness
        .coordinator
        .submit_removal(&device("bob"), 1000)
        .await
        .unwrap();
    let state = harness.repository.load_state().await.unwrap().unwrap();

    let error = harness
        .coordinator
        .ingest_exchange(
            &device("mallory"),
            uc_core::membership::RemovalExchangeMessage::KeyPackageRequest {
                convergence_digest: state.convergence().convergence_digest(),
            },
            1001,
        )
        .await
        .unwrap_err();

    assert!(matches!(error, RemovalCoordinatorError::NotAMember));
}

#[tokio::test]
async fn a_removed_member_relays_saved_history_only_through_the_late_submission_channel() {
    // L01/L03: B 在看到 A→B 后，仍能迟交此前保存的 B→C；普通成员通道
    // 不得再用于这次转发，迟交通道只承载历史意图和有界接收结果。
    let alice = device("alice");
    let bob = device("bob");
    let charlie = device("charlie");
    let members = vec![("alice", 1), ("bob", 2), ("charlie", 3)];
    let a = Harness::build(&alice, members.clone());
    let b = Harness::build(&bob, members);

    b.coordinator.submit_removal(&charlie, 1000).await.unwrap();
    a.coordinator.submit_removal(&bob, 1001).await.unwrap();
    let removal_of_bob = a
        .repository
        .load_state()
        .await
        .unwrap()
        .unwrap()
        .intents
        .into_iter()
        .next()
        .unwrap();
    b.coordinator
        .handle_late_submission(
            RemovalLateSubmission::Intent(Box::new(removal_of_bob)),
            1002,
        )
        .await
        .unwrap();

    b.coordinator.reconcile(1003).await.unwrap();

    assert!(b.exchange.sent.lock().unwrap().is_empty());
    let late = b.late_exchange.sent.lock().unwrap().clone();
    assert_eq!(late.len(), 4);
    assert!(late
        .iter()
        .all(|(_, submission)| { matches!(submission, RemovalLateSubmission::Intent(_)) }));
    assert!(late.iter().all(|(recipient, _)| recipient != &bob));
}

#[tokio::test]
async fn cannot_remove_the_local_member_instance() {
    // V03:A 试图移除自己的成员实例,创建前拒绝,无磁盘和网络动作。
    let alice = device("alice");
    let harness = Harness::build(&alice, vec![("alice", 1), ("bob", 2)]);
    let error = harness
        .coordinator
        .submit_removal(&device("alice"), 1000)
        .await
        .unwrap_err();
    assert!(matches!(error, RemovalCoordinatorError::SelfTarget));
    assert!(harness.exchange.sent.lock().unwrap().is_empty());
    let state = harness.repository.load_state().await.unwrap();
    assert!(state.is_none() || state.unwrap().intents.is_empty());
}

#[tokio::test]
async fn unknown_target_is_rejected() {
    // V02:目标不在基准视图,拒绝且状态不变。
    let alice = device("alice");
    let harness = Harness::build(&alice, vec![("alice", 1), ("bob", 2)]);
    let error = harness
        .coordinator
        .submit_removal(&device("mallory"), 1000)
        .await
        .unwrap_err();
    assert!(matches!(error, RemovalCoordinatorError::UnknownTarget));
}

#[tokio::test]
async fn intent_from_another_space_is_rejected_before_it_is_saved() {
    // V05:另一空间沿革的意图不能进入当前空间的已知集合。
    let alice = device("alice");
    let harness = Harness::build(&alice, vec![("alice", 1), ("bob", 2)]);
    let intent = signed_intent(uc_core::membership::RemovalIntentContent {
        space_lineage: "space-b".to_owned(),
        view_epoch: 1,
        view_members: sorted_view(vec![instance("alice", 1), instance("bob", 2)]),
        initiator: instance("alice", 1),
        target: instance("bob", 2),
    });

    let error = harness
        .coordinator
        .ingest_exchange(
            &alice,
            uc_core::membership::RemovalExchangeMessage::Intent(Box::new(intent)),
            1000,
        )
        .await
        .unwrap_err();

    assert!(matches!(error, RemovalCoordinatorError::SpaceMismatch));
    assert!(harness.repository.load_state().await.unwrap().is_none());
}

#[tokio::test]
async fn observed_own_removal_rejects_new_intent_creation() {
    // L07:本机已经观察到自己被移除,再通过正常入口移除 C → 本机立即拒绝。
    let bob = device("bob");
    let harness = Harness::build(&bob, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);
    // 模拟 B 已被移除:构造一条移除 B 的意图并让 B 验收。
    let initiator = instance("alice", 1);
    let target = instance("bob", 2);
    let view_members = sorted_view(vec![initiator, target, instance("charlie", 3)]);
    let content = uc_core::membership::RemovalIntentContent {
        space_lineage: "space-a".to_owned(),
        view_epoch: 1,
        view_members,
        initiator,
        target,
    };
    let intent = signed_intent(content);
    harness
        .coordinator
        .ingest_exchange(
            &device("alice"),
            uc_core::membership::RemovalExchangeMessage::Intent(Box::new(intent)),
            1000,
        )
        .await
        .unwrap();
    let error = harness
        .coordinator
        .submit_removal(&device("charlie"), 1001)
        .await
        .unwrap_err();
    assert!(matches!(error, RemovalCoordinatorError::OwnInstanceRemoved));
}

#[tokio::test]
async fn removed_member_cannot_send_complete_over_normal_exchange() {
    // C12/P02:已被移除的成员不能借普通入口发送完成通知。
    let alice = device("alice");
    let harness = Harness::build(&alice, vec![("alice", 1), ("bob", 2)]);
    harness
        .coordinator
        .submit_removal(&device("bob"), 1000)
        .await
        .unwrap();
    let mut state = harness.repository.load_state().await.unwrap().unwrap();
    let digest = state.convergence().convergence_digest();
    state.applied_digest = Some(digest);
    state.phase = RemovalPhase::Converging;
    harness.repository.save_state(&state).await.unwrap();

    let error = harness
        .coordinator
        .ingest_exchange(
            &device("bob"),
            uc_core::membership::RemovalExchangeMessage::Complete {
                convergence_digest: digest,
                receipts: Vec::new(),
            },
            1001,
        )
        .await
        .unwrap_err();

    assert!(matches!(error, RemovalCoordinatorError::OwnInstanceRemoved));
    let state = harness.repository.load_state().await.unwrap().unwrap();
    assert_eq!(state.phase, RemovalPhase::Converging);
}

#[tokio::test]
async fn executor_completion_without_all_retained_member_confirmations_is_not_complete() {
    // P02:即使消息来自本轮确定执行者，也不能用一条网络通知代替所有保留成员
    // 实际应用同一安全状态的确认。
    let alice = device("alice");
    let bob = device("bob");
    let charlie = device("charlie");
    let alice_instance = instance("alice", 1);
    let charlie_instance = instance("charlie", 3);
    let executor = [alice_instance, charlie_instance]
        .into_iter()
        .min()
        .unwrap();
    let (receiver, executor_device) = if executor == alice_instance {
        (charlie, alice)
    } else {
        (alice, charlie)
    };
    let harness = Harness::build(&receiver, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);
    harness
        .coordinator
        .submit_removal(&bob, 1000)
        .await
        .unwrap();
    let mut state = harness.repository.load_state().await.unwrap().unwrap();
    let digest = state.convergence().convergence_digest();
    assert_eq!(state.convergence().executor(), Some(executor));
    state.applied_digest = Some(digest);
    state.phase = RemovalPhase::Converging;
    harness.repository.save_state(&state).await.unwrap();

    harness
        .coordinator
        .ingest_exchange(
            &executor_device,
            uc_core::membership::RemovalExchangeMessage::Complete {
                convergence_digest: digest,
                receipts: Vec::new(),
            },
            1001,
        )
        .await
        .unwrap();

    let state = harness.repository.load_state().await.unwrap().unwrap();
    assert_eq!(state.phase, RemovalPhase::Converging);
}

#[tokio::test]
async fn received_complete_state_is_published_without_waiting_for_the_retry_timer() {
    // P01/P04:入站完成状态已经落盘后，订阅者必须立刻得到新完整状态；不能
    // 等待下一次固定重试，也不能要求产品端猜测状态是否已变化。
    let alice = device("alice");
    let bob = device("bob");
    let charlie = device("charlie");
    let alice_instance = instance("alice", 1);
    let charlie_instance = instance("charlie", 3);
    let executor = [alice_instance, charlie_instance]
        .into_iter()
        .min()
        .unwrap();
    let (receiver, executor_device) = if executor == alice_instance {
        (charlie, alice)
    } else {
        (alice, charlie)
    };
    let harness = Harness::build(&receiver, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);
    let (_presence_sender, presence_events) = tokio::sync::broadcast::channel(1);
    let (state_events, mut changes) = tokio::sync::broadcast::channel(8);
    let runtime = super::MemberRemovalRuntime::start(
        Arc::clone(&harness.coordinator),
        presence_events,
        state_events,
    );
    let _ = tokio::time::timeout(Duration::from_secs(1), changes.recv())
        .await
        .unwrap()
        .unwrap();
    harness
        .coordinator
        .submit_removal(&bob, 1000)
        .await
        .unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(1), changes.recv())
        .await
        .unwrap()
        .unwrap();
    let mut state = harness.repository.load_state().await.unwrap().unwrap();
    let digest = state.convergence().convergence_digest();
    state.applied_digest = Some(digest);
    state.phase = RemovalPhase::Converging;
    harness.repository.save_state(&state).await.unwrap();
    let receipts = state
        .effective_members()
        .into_iter()
        .map(|member| RemovalCompletionReceipt {
            member,
            convergence_digest: digest,
            signature: vec![1],
        })
        .collect();

    harness
        .coordinator
        .ingest_exchange(
            &executor_device,
            uc_core::membership::RemovalExchangeMessage::Complete {
                convergence_digest: digest,
                receipts,
            },
            1001,
        )
        .await
        .unwrap();

    let change = tokio::time::timeout(Duration::from_millis(250), changes.recv())
        .await
        .expect("completion state should be published immediately")
        .unwrap();
    assert_eq!(
        change.phase,
        super::super::facade::MemberRemovalPhaseView::Complete
    );
    assert_eq!(
        change.effective_member_count, 2,
        "a completed round must report every retained member, even when a recovered peer now has a new instance"
    );
    runtime.shutdown().await;
}

#[tokio::test]
async fn restart_after_notification_interruption_publishes_the_saved_snapshot() {
    // R12:通知前退出不丢失事实；新进程只需查询或订阅当前完整状态。
    let alice = device("alice");
    let bob = device("bob");
    let harness = Harness::build(&alice, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);
    harness
        .coordinator
        .submit_removal(&bob, 1000)
        .await
        .unwrap();
    let (_presence_sender, presence_events) = tokio::sync::broadcast::channel(1);
    let (state_events, mut changes) = tokio::sync::broadcast::channel(8);
    let runtime =
        super::MemberRemovalRuntime::start(harness.restart(), presence_events, state_events);

    let change = tokio::time::timeout(Duration::from_secs(1), changes.recv())
        .await
        .expect("restart should publish the saved member-removal snapshot")
        .unwrap();
    assert_eq!(
        change.phase,
        super::super::facade::MemberRemovalPhaseView::Converging
    );
    assert_eq!(change.intent_count, 1);
    runtime.shutdown().await;
}

#[tokio::test]
async fn chained_offline_removals_converge_on_the_first_author() {
    // O01:A 移除 B、B 移除 C(互不知道),两意图都有效,最终只保留 A。
    let alice = device("alice");
    let bob = device("bob");
    let harness_a = Harness::build(&alice, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);
    let harness_b = Harness::build(&bob, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);

    // A 离线移除 B;B 离线移除 C。
    harness_a
        .coordinator
        .submit_removal(&device("bob"), 1000)
        .await
        .unwrap();
    harness_b
        .coordinator
        .submit_removal(&device("charlie"), 1001)
        .await
        .unwrap();

    // 各自触发传播。
    harness_a.coordinator.reconcile(1002).await.unwrap();
    harness_b.coordinator.reconcile(1003).await.unwrap();

    // A 的意图发给 B,B 的意图发给 A(先乱序,后重连)。
    deliver_to(&harness_a.exchange, &harness_b.coordinator, 1004).await;
    deliver_to(&harness_b.exchange, &harness_a.coordinator, 1005).await;

    let state_a = harness_a.repository.load_state().await.unwrap().unwrap();
    let state_b = harness_b.repository.load_state().await.unwrap().unwrap();
    assert_eq!(state_a.intents.len(), 2);
    assert_eq!(state_b.intents.len(), 2);
    let effective_a = state_a.effective_members();
    assert_eq!(effective_a, BTreeSet::from([instance("alice", 1)]));
    assert_eq!(
        state_a.convergence().convergence_digest(),
        state_b.convergence().convergence_digest()
    );
}

#[tokio::test]
async fn late_intent_from_a_removed_author_is_accepted_but_does_not_restore_it() {
    // L01:B 已被 A 移除,随后 B 提交此前创建的 B→C 意图。
    let alice = device("alice");
    let harness = Harness::build(&alice, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);
    harness
        .coordinator
        .submit_removal(&device("bob"), 1000)
        .await
        .unwrap();

    // B 在不知道自己被移除时创建 B→C。
    let initiator = instance("bob", 2);
    let target = instance("charlie", 3);
    let view_members = sorted_view(vec![instance("alice", 1), initiator, target]);
    let content = uc_core::membership::RemovalIntentContent {
        space_lineage: "space-a".to_owned(),
        view_epoch: 1,
        view_members,
        initiator,
        target,
    };
    let intent = signed_intent(content);

    // 通过受限迟交入口提交(已移除设备不能走普通成员通道)。
    let acceptance = harness
        .coordinator
        .handle_late_submission(RemovalLateSubmission::Intent(Box::new(intent)), 1001)
        .await
        .unwrap();
    match acceptance {
        RemovalLateAcceptance::Accepted { .. } | RemovalLateAcceptance::AlreadyKnown { .. } => {}
        RemovalLateAcceptance::Rejected { reason } => panic!("late intent rejected: {reason:?}"),
    }
    let state = harness.repository.load_state().await.unwrap().unwrap();
    assert_eq!(
        state.effective_members(),
        BTreeSet::from([instance("alice", 1)])
    );
}

#[tokio::test]
async fn saved_causal_history_still_accepts_a_late_intent_after_restart() {
    // L01/R01: 已锚定的历史检查点和意图同次保存，重启后仍可验证该历史。
    let alice = device("alice");
    let harness = Harness::build(&alice, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);
    harness
        .coordinator
        .submit_removal(&device("bob"), 1000)
        .await
        .unwrap();
    let content = RemovalIntentContent {
        space_lineage: "space-a".to_owned(),
        view_epoch: 1,
        view_members: sorted_view(vec![
            instance("alice", 1),
            instance("bob", 2),
            instance("charlie", 3),
        ]),
        initiator: instance("bob", 2),
        target: instance("charlie", 3),
    };

    let acceptance = harness
        .restart()
        .handle_late_submission(
            RemovalLateSubmission::Intent(Box::new(signed_intent(content))),
            1001,
        )
        .await
        .unwrap();
    assert!(matches!(acceptance, RemovalLateAcceptance::Accepted { .. }));
    let state = harness.repository.load_state().await.unwrap().unwrap();
    assert_eq!(state.causal_history.len(), 1);
    assert_eq!(state.intents.len(), 2);
}

#[tokio::test]
async fn late_intent_submission_is_idempotent() {
    // L04:已移除 B 反复迟交同一意图,幂等。
    let alice = device("alice");
    let harness = Harness::build(&alice, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);
    harness
        .coordinator
        .submit_removal(&device("bob"), 1000)
        .await
        .unwrap();

    let content = uc_core::membership::RemovalIntentContent {
        space_lineage: "space-a".to_owned(),
        view_epoch: 1,
        view_members: sorted_view(vec![
            instance("alice", 1),
            instance("bob", 2),
            instance("charlie", 3),
        ]),
        initiator: instance("bob", 2),
        target: instance("charlie", 3),
    };
    let intent = signed_intent(content);

    let first = harness
        .coordinator
        .handle_late_submission(
            RemovalLateSubmission::Intent(Box::new(intent.clone())),
            1001,
        )
        .await
        .unwrap();
    let second = harness
        .coordinator
        .handle_late_submission(RemovalLateSubmission::Intent(Box::new(intent)), 1002)
        .await
        .unwrap();
    assert!(matches!(first, RemovalLateAcceptance::Accepted { .. }));
    assert!(matches!(second, RemovalLateAcceptance::AlreadyKnown { .. }));
    let state = harness.repository.load_state().await.unwrap().unwrap();
    assert_eq!(state.intents.len(), 2);
}

#[tokio::test]
async fn late_intent_with_missing_causal_history_is_rejected() {
    // L05:签名正确但缺少基准历史的意图 → 拒绝,不猜测。
    let alice = device("alice");
    let harness = Harness::build(&alice, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);
    harness
        .coordinator
        .submit_removal(&device("bob"), 1000)
        .await
        .unwrap();
    let content = uc_core::membership::RemovalIntentContent {
        space_lineage: "space-a".to_owned(),
        view_epoch: 0,
        view_members: sorted_view(vec![
            instance("alice", 1),
            instance("bob", 2),
            instance("charlie", 3),
        ]),
        initiator: instance("bob", 2),
        target: instance("charlie", 3),
    };
    // 验签端口明确接受该意图；它和本机当前视图的证明不同，因此缺的是本机
    // 可验证的基准历史，不是签名或编码错误。
    let intent = signed_intent(content);
    let acceptance = harness
        .coordinator
        .handle_late_submission(RemovalLateSubmission::Intent(Box::new(intent)), 1001)
        .await
        .unwrap();
    assert!(matches!(
        acceptance,
        RemovalLateAcceptance::Rejected {
            reason: RemovalLateRejectionReason::MissingCausalHistory
        }
    ));
    // 当前成员和密钥不变，且停止自动推进而不是猜测历史关系。
    let state = harness.repository.load_state().await.unwrap().unwrap();
    assert_eq!(state.intents.len(), 1);
    assert_eq!(state.phase, RemovalPhase::RecoveryRequired);
}

#[tokio::test]
async fn malformed_proof_is_rejected_without_forcing_recovery() {
    let alice = device("alice");
    let harness = Harness::build(&alice, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);
    harness
        .coordinator
        .submit_removal(&device("bob"), 1000)
        .await
        .unwrap();
    *harness.verifier.failure.lock().unwrap() =
        Some(uc_core::membership::RemovalIntentVerificationError::InvalidProof);
    let intent = signed_intent(uc_core::membership::RemovalIntentContent {
        space_lineage: "space-a".to_owned(),
        view_epoch: 1,
        view_members: sorted_view(vec![
            instance("alice", 1),
            instance("bob", 2),
            instance("charlie", 3),
        ]),
        initiator: instance("bob", 2),
        target: instance("charlie", 3),
    });

    let acceptance = harness
        .coordinator
        .handle_late_submission(RemovalLateSubmission::Intent(Box::new(intent)), 1001)
        .await
        .unwrap();
    assert!(matches!(
        acceptance,
        RemovalLateAcceptance::Rejected {
            reason: RemovalLateRejectionReason::Invalid
        }
    ));
    let state = harness.repository.load_state().await.unwrap().unwrap();
    assert_eq!(state.phase, RemovalPhase::Applied);
    assert_eq!(state.intents.len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn late_submission_failure_logs_a_category_without_repository_details() {
    // P06:持久化失败中的原文不能通过成员移除日志泄露。
    let alice = device("alice");
    let harness = Harness::build(&alice, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);
    harness
        .coordinator
        .submit_removal(&device("bob"), 1000)
        .await
        .unwrap();
    harness
        .repository
        .fail_next_new_intent_save(RemovalIntentRepositoryError::Repository(
            "removal-log-sensitive-marker".to_owned(),
        ));
    let writer = CapturedWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(writer.clone())
        .with_ansi(false)
        .finish();
    let dispatch = tracing::Dispatch::new(subscriber);
    let _guard = tracing::dispatcher::set_default(&dispatch);

    let acceptance = harness
        .coordinator
        .handle_late_submission(
            RemovalLateSubmission::Intent(Box::new(signed_intent(RemovalIntentContent {
                space_lineage: "space-a".to_owned(),
                view_epoch: 1,
                view_members: sorted_view(vec![
                    instance("alice", 1),
                    instance("bob", 2),
                    instance("charlie", 3),
                ]),
                initiator: instance("bob", 2),
                target: instance("charlie", 3),
            }))),
            1001,
        )
        .await
        .unwrap();

    assert!(matches!(
        acceptance,
        RemovalLateAcceptance::Rejected {
            reason: RemovalLateRejectionReason::Invalid
        }
    ));
    assert!(
        !writer.dump().contains("removal-log-sensitive-marker"),
        "repository details leaked through member-removal logs"
    );
}

#[tokio::test]
async fn completed_convergence_restarts_on_late_intent() {
    // L08/P04:一个旧摘要已 Complete,随后收到从未复制过的合法意图 → 重新进入收敛。
    let alice = device("alice");
    let harness = Harness::build(&alice, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);
    harness
        .coordinator
        .submit_removal(&device("bob"), 1000)
        .await
        .unwrap();

    // 模拟执行者完成:B 是有效成员,先送恢复资料再送 Complete。
    let mut state = harness.repository.load_state().await.unwrap().unwrap();
    state.phase = RemovalPhase::Complete;
    state.applied_digest = Some(state.convergence().convergence_digest());
    harness.repository.save_state(&state).await.unwrap();
    assert_eq!(
        harness.coordinator.query(1001).await.unwrap().phase,
        RemovalPhase::Complete
    );

    // 迟到的 B→C 意图重新打开完成状态。
    let content = uc_core::membership::RemovalIntentContent {
        space_lineage: "space-a".to_owned(),
        view_epoch: 1,
        view_members: sorted_view(vec![
            instance("alice", 1),
            instance("bob", 2),
            instance("charlie", 3),
        ]),
        initiator: instance("bob", 2),
        target: instance("charlie", 3),
    };
    let intent = signed_intent(content);
    harness
        .coordinator
        .handle_late_submission(RemovalLateSubmission::Intent(Box::new(intent)), 1002)
        .await
        .unwrap();
    let summary = harness.coordinator.query(1003).await.unwrap();
    assert_eq!(summary.phase, RemovalPhase::Converging);
    assert_eq!(summary.effective_member_count, 1);
}

#[tokio::test]
async fn empty_effective_members_enters_recovery_required() {
    // C02/R09:A 移除 B、B 移除 C、C 移除 A → 有效成员为空,进入 RecoveryRequired。
    let alice = device("alice");
    let harness = Harness::build(&alice, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);
    harness
        .coordinator
        .submit_removal(&device("bob"), 1000)
        .await
        .unwrap();

    // B 移除 C。
    let content = uc_core::membership::RemovalIntentContent {
        space_lineage: "space-a".to_owned(),
        view_epoch: 1,
        view_members: sorted_view(vec![
            instance("alice", 1),
            instance("bob", 2),
            instance("charlie", 3),
        ]),
        initiator: instance("bob", 2),
        target: instance("charlie", 3),
    };
    harness
        .coordinator
        .handle_late_submission(
            RemovalLateSubmission::Intent(Box::new(signed_intent(content))),
            1001,
        )
        .await
        .unwrap();

    // C 移除 A。
    let content = uc_core::membership::RemovalIntentContent {
        space_lineage: "space-a".to_owned(),
        view_epoch: 1,
        view_members: sorted_view(vec![
            instance("alice", 1),
            instance("bob", 2),
            instance("charlie", 3),
        ]),
        initiator: instance("charlie", 3),
        target: instance("alice", 1),
    };
    harness
        .coordinator
        .handle_late_submission(
            RemovalLateSubmission::Intent(Box::new(signed_intent(content))),
            1002,
        )
        .await
        .unwrap();

    harness.coordinator.reconcile(1003).await.unwrap();
    let summary = harness.coordinator.query(1004).await.unwrap();
    assert_eq!(summary.phase, RemovalPhase::RecoveryRequired);
}

#[tokio::test]
async fn mutual_removals_converge_to_the_remaining_member() {
    // C01:A 移除 B、B 移除 A(同一因果视图的并发意图),C 未被移除 → 最终只保留 C。
    let alice = device("alice");
    let harness = Harness::build(&alice, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);
    harness
        .coordinator
        .submit_removal(&device("bob"), 1000)
        .await
        .unwrap();

    // B 移除 A:同一视图上的并发意图,必须与 A→B 合并。
    let content = uc_core::membership::RemovalIntentContent {
        space_lineage: "space-a".to_owned(),
        view_epoch: 1,
        view_members: sorted_view(vec![
            instance("alice", 1),
            instance("bob", 2),
            instance("charlie", 3),
        ]),
        initiator: instance("bob", 2),
        target: instance("alice", 1),
    };
    harness
        .coordinator
        .handle_late_submission(
            RemovalLateSubmission::Intent(Box::new(signed_intent(content))),
            1001,
        )
        .await
        .unwrap();

    let summary = harness.coordinator.query(1002).await.unwrap();
    assert_eq!(summary.phase, RemovalPhase::Converging);
    assert_eq!(summary.effective_member_count, 1);
    assert_eq!(summary.intent_count, 2);
}

#[tokio::test]
async fn two_device_mutual_removal_enters_recovery_required() {
    // C03:两设备 A、B 完全离线时互相移除,没有第三方保留 → RecoveryRequired,
    // 不自动选择幸存者。
    let alice = device("alice");
    let harness = Harness::build(&alice, vec![("alice", 1), ("bob", 2)]);
    harness
        .coordinator
        .submit_removal(&device("bob"), 1000)
        .await
        .unwrap();

    // B 移除 A:有效成员并集为空。
    let content = uc_core::membership::RemovalIntentContent {
        space_lineage: "space-a".to_owned(),
        view_epoch: 1,
        view_members: sorted_view(vec![instance("alice", 1), instance("bob", 2)]),
        initiator: instance("bob", 2),
        target: instance("alice", 1),
    };
    harness
        .coordinator
        .handle_late_submission(
            RemovalLateSubmission::Intent(Box::new(signed_intent(content))),
            1001,
        )
        .await
        .unwrap();

    harness.coordinator.reconcile(1002).await.unwrap();
    let summary = harness.coordinator.query(1003).await.unwrap();
    assert_eq!(summary.phase, RemovalPhase::RecoveryRequired);
    assert_eq!(summary.effective_member_count, 0);
}

#[tokio::test]
async fn executor_collects_key_packages_and_distributes_recovery_material() {
    // R03/R04:执行者收集 key package 后生成恢复资料并分发,等待全部确认。
    let alice = device("alice");
    let bob = device("bob");
    let charlie = device("charlie");
    let harness_a = Harness::build(&alice, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);
    let harness_b = Harness::build(&bob, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);
    let harness_c = Harness::build(&charlie, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);

    // A 移除 B。有效成员 = {A, C},执行者 = min(A, C) 按实例 ID。
    harness_a
        .coordinator
        .submit_removal(&device("bob"), 1000)
        .await
        .unwrap();
    harness_a.coordinator.reconcile(1001).await.unwrap();
    // 传播到 B、C(连同 key package 请求一起同步交换)。
    round_trip(&harness_a, &harness_c, 1002).await;
    round_trip(&harness_a, &harness_b, 1002).await;

    // 确定执行者:有效集合 {alice, charlie} 中实例 ID 较小者。
    let state_a = harness_a.repository.load_state().await.unwrap().unwrap();
    let executor = state_a.convergence().executor().unwrap();
    let a_is_executor = harness_a.recovery.own_instance().await.unwrap() == Some(executor);
    let (executor_harness, member_harness) = if a_is_executor {
        (&harness_a, &harness_c)
    } else {
        (&harness_c, &harness_a)
    };

    // 执行者开始恢复:请求 key package。
    executor_harness.coordinator.reconcile(1003).await.unwrap();
    // 请求与回应在一次同步交换中完成。
    round_trip(executor_harness, member_harness, 1004).await;
    round_trip(executor_harness, member_harness, 1005).await;
    // 执行者再次推进:生成并分发恢复资料。
    executor_harness.coordinator.reconcile(1006).await.unwrap();
    // 成员应用恢复资料并确认(同步交换带回确认)。
    round_trip(executor_harness, member_harness, 1007).await;
    let member_state = member_harness
        .repository
        .load_state()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(member_state.phase, RemovalPhase::Converging);
    assert!(member_state.applied_digest.is_some());
    assert!(!member_harness.recovery.applied.lock().unwrap().is_empty());

    // 执行者收集齐确认后完成并广播 Complete。
    executor_harness.coordinator.reconcile(1009).await.unwrap();
    round_trip(executor_harness, member_harness, 1010).await;
    let member_state = member_harness
        .repository
        .load_state()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(member_state.phase, RemovalPhase::Complete);
}

#[tokio::test]
async fn executor_persists_its_recovery_checkpoint_before_local_application() {
    // R03:恢复资料必须先保存，进程在本机应用前退出时才能从同一资料继续。
    let alice = device("alice");
    let bob = device("bob");
    let charlie = device("charlie");
    let harness_a = Harness::build(&alice, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);
    let harness_b = Harness::build(&bob, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);
    let harness_c = Harness::build(&charlie, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);

    harness_a
        .coordinator
        .submit_removal(&bob, 1000)
        .await
        .unwrap();
    harness_a.coordinator.reconcile(1001).await.unwrap();
    round_trip(&harness_a, &harness_c, 1002).await;
    round_trip(&harness_a, &harness_b, 1002).await;

    let state_a = harness_a.repository.load_state().await.unwrap().unwrap();
    let executor = state_a.convergence().executor().unwrap();
    let a_is_executor = harness_a.recovery.own_instance().await.unwrap() == Some(executor);
    let (executor_harness, member_harness) = if a_is_executor {
        (&harness_a, &harness_c)
    } else {
        (&harness_c, &harness_a)
    };

    executor_harness.coordinator.reconcile(1003).await.unwrap();
    round_trip(executor_harness, member_harness, 1004).await;
    round_trip(executor_harness, member_harness, 1005).await;
    executor_harness.coordinator.reconcile(1006).await.unwrap();

    let state = executor_harness
        .repository
        .load_state()
        .await
        .unwrap()
        .unwrap();
    let recovery = state.recovery.unwrap();
    assert!(recovery.material.is_some());
    assert!(recovery.local_checkpoint.is_some());
}

#[tokio::test]
async fn executor_restart_reuses_the_persisted_checkpoint_after_local_install_fails() {
    // R03:本机安装失败后，重启只能继续已保存的资料，不能生成第二份恢复状态。
    let alice = device("alice");
    let bob = device("bob");
    let charlie = device("charlie");
    let harness_a = Harness::build(&alice, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);
    let harness_b = Harness::build(&bob, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);
    let harness_c = Harness::build(&charlie, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);

    harness_a
        .coordinator
        .submit_removal(&bob, 1000)
        .await
        .unwrap();
    harness_a.coordinator.reconcile(1001).await.unwrap();
    round_trip(&harness_a, &harness_c, 1002).await;
    round_trip(&harness_a, &harness_b, 1002).await;

    let state_a = harness_a.repository.load_state().await.unwrap().unwrap();
    let executor = state_a.convergence().executor().unwrap();
    let a_is_executor = harness_a.recovery.own_instance().await.unwrap() == Some(executor);
    let (executor_harness, member_harness) = if a_is_executor {
        (&harness_a, &harness_c)
    } else {
        (&harness_c, &harness_a)
    };

    executor_harness.coordinator.reconcile(1003).await.unwrap();
    round_trip(executor_harness, member_harness, 1004).await;
    round_trip(executor_harness, member_harness, 1005).await;
    *executor_harness.recovery.install_failure.lock().unwrap() =
        Some(uc_core::membership::RemovalRecoveryError::Unavailable);
    assert!(executor_harness.coordinator.reconcile(1006).await.is_err());
    assert_eq!(
        executor_harness
            .recovery
            .prepared_checkpoints
            .lock()
            .unwrap()
            .len(),
        1
    );
    let state = executor_harness
        .repository
        .load_state()
        .await
        .unwrap()
        .unwrap();
    assert!(state.recovery.unwrap().local_checkpoint.is_some());

    *executor_harness.recovery.install_failure.lock().unwrap() = None;
    executor_harness.restart().reconcile(1007).await.unwrap();
    assert_eq!(
        executor_harness
            .recovery
            .prepared_checkpoints
            .lock()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        executor_harness
            .recovery
            .installed_checkpoints
            .lock()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn recovery_material_with_wrong_target_set_is_rejected() {
    // R08:收到摘要匹配但目标集合不同的恢复资料 → 拒绝并保持当前状态。
    let alice = device("alice");
    let harness = Harness::build(&alice, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);
    harness
        .coordinator
        .submit_removal(&device("bob"), 1000)
        .await
        .unwrap();
    let state = harness.repository.load_state().await.unwrap().unwrap();
    let digest = state.convergence().convergence_digest();

    let wrong = uc_core::membership::RemovalRecoveryMaterial {
        convergence_digest: digest,
        effective_members: vec![instance("alice", 1), instance("bob", 2)],
        epoch: 2,
        commit: vec![1],
        welcome: None,
        encrypted_key_catalog: vec![2],
    };
    let response = harness
        .coordinator
        .ingest_exchange(
            &device("charlie"),
            uc_core::membership::RemovalExchangeMessage::RecoveryMaterial(wrong),
            1001,
        )
        .await
        .unwrap();
    assert!(matches!(
        response,
        uc_core::membership::RemovalExchangeMessage::IntentAck(_)
    ));
    let state = harness.repository.load_state().await.unwrap().unwrap();
    assert!(state.applied_digest.is_none());
    assert!(harness.recovery.applied.lock().unwrap().is_empty());
}

#[tokio::test]
async fn recovery_material_from_a_non_executor_is_rejected() {
    // R04:只有当前摘要确定的执行者能分发恢复资料，其他保留成员不能创建并行状态。
    let alice = device("alice");
    let harness = Harness::build(&alice, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);
    harness
        .coordinator
        .submit_removal(&device("bob"), 1000)
        .await
        .unwrap();
    let state = harness.repository.load_state().await.unwrap().unwrap();
    let digest = state.convergence().convergence_digest();
    let effective_members = state.effective_members().into_iter().collect::<Vec<_>>();
    let executor = state.convergence().executor().unwrap();
    let non_executor = effective_members
        .iter()
        .copied()
        .find(|member| *member != executor)
        .unwrap();
    let source = state.member_devices.get(&non_executor).unwrap().clone();

    let response = harness
        .coordinator
        .ingest_exchange(
            &source,
            uc_core::membership::RemovalExchangeMessage::RecoveryMaterial(
                uc_core::membership::RemovalRecoveryMaterial {
                    convergence_digest: digest,
                    effective_members,
                    epoch: 2,
                    commit: vec![1],
                    welcome: None,
                    encrypted_key_catalog: vec![2],
                },
            ),
            1001,
        )
        .await
        .unwrap();

    assert!(matches!(
        response,
        uc_core::membership::RemovalExchangeMessage::IntentAck(_)
    ));
    assert!(harness.recovery.applied.lock().unwrap().is_empty());
    let state = harness.repository.load_state().await.unwrap().unwrap();
    assert!(state.applied_digest.is_none());
}

#[tokio::test]
async fn executor_restarts_with_fresh_digest_when_new_intent_arrives() {
    // R06:执行者恢复时发现更新的合法意图 → 不发布过时目标,转向新摘要。
    let alice = device("alice");
    let charlie = device("charlie");
    let harness_a = Harness::build(&alice, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);
    let harness_c = Harness::build(&charlie, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);

    harness_a
        .coordinator
        .submit_removal(&device("bob"), 1000)
        .await
        .unwrap();
    deliver_to(&harness_a.exchange, &harness_c.coordinator, 1001).await;

    // C 在恢复进行中提交自己的新意图(C 移除自己?不——C 移除… 只能移除其他人)。
    // C 创建 C→D?D 不存在。改为:B 的迟到意图(通过受限入口给 A)。
    let content = uc_core::membership::RemovalIntentContent {
        space_lineage: "space-a".to_owned(),
        view_epoch: 1,
        view_members: sorted_view(vec![
            instance("alice", 1),
            instance("bob", 2),
            instance("charlie", 3),
        ]),
        initiator: instance("bob", 2),
        target: instance("charlie", 3),
    };
    harness_a
        .coordinator
        .handle_late_submission(
            RemovalLateSubmission::Intent(Box::new(signed_intent(content))),
            1002,
        )
        .await
        .unwrap();

    // 执行者推进:新摘要要求恢复 {A}(B、C 都被移除),唯一有效成员立即完成。
    harness_a.coordinator.reconcile(1003).await.unwrap();
    let state_a = harness_a.repository.load_state().await.unwrap().unwrap();
    assert_eq!(state_a.phase, RemovalPhase::Complete);
    assert_eq!(
        state_a.applied_digest,
        Some(state_a.convergence().convergence_digest())
    );
    assert!(state_a.recovery.is_some());
}

#[tokio::test]
async fn completed_executor_retries_a_completion_notification_after_transport_failure() {
    // 资料确认齐全后，完成通知丢失不能让其他保留成员永久停在 Converging。
    let alice = device("alice");
    let charlie = device("charlie");
    let alice_instance = instance("alice", 1);
    let charlie_instance = instance("charlie", 3);
    let executor = [alice_instance, charlie_instance]
        .into_iter()
        .min()
        .unwrap();
    let executor_device = if executor == alice_instance {
        alice
    } else {
        charlie
    };
    let harness = Harness::build(
        &executor_device,
        vec![("alice", 1), ("bob", 2), ("charlie", 3)],
    );
    harness
        .coordinator
        .submit_removal(&device("bob"), 1000)
        .await
        .unwrap();
    let mut state = harness.repository.load_state().await.unwrap().unwrap();
    let digest = state.convergence().convergence_digest();
    let effective = state.effective_members();
    state.phase = RemovalPhase::Complete;
    state.applied_digest = Some(digest);
    state.completed_member_count = Some(effective.len());
    state.recovery = Some(RemovalRecoveryPersisted {
        convergence_digest: digest,
        effective_members: effective.iter().copied().collect(),
        key_packages: BTreeMap::new(),
        material: None,
        local_checkpoint: None,
        delivery_acks: effective
            .iter()
            .map(|member| {
                (
                    *member,
                    RemovalCompletionReceipt {
                        member: *member,
                        convergence_digest: digest,
                        signature: vec![1],
                    },
                )
            })
            .collect(),
        completion_deliveries: BTreeMap::new(),
    });
    harness.repository.save_state(&state).await.unwrap();

    harness.exchange.fail_next_complete();
    // 通知通道在同一推进周期内也保持不可达:完成通知与移除通知都失败时,
    // 不记录任何投递进度,重启后一起重试。
    harness
        .notice_exchange
        .fail_next
        .lock()
        .unwrap()
        .clone_from(&true);
    harness.coordinator.reconcile(1001).await.unwrap();
    let after_failure = harness.repository.load_state().await.unwrap().unwrap();
    assert!(after_failure
        .recovery
        .as_ref()
        .unwrap()
        .completion_deliveries
        .is_empty());
    assert!(after_failure.notified_removals.is_empty());

    harness.restart().reconcile(1002).await.unwrap();
    let after_retry = harness.repository.load_state().await.unwrap().unwrap();
    assert_eq!(
        after_retry
            .recovery
            .as_ref()
            .unwrap()
            .completion_deliveries
            .len(),
        1
    );
}

#[tokio::test]
async fn retained_member_rejects_out_of_order_recovery_material() {
    // R07:保留成员先收到较新的安全资料(与本地摘要不符) → 拒绝越级应用,
    // 补齐后按序应用,不能跳过中间状态。
    let alice = device("alice");
    let harness = Harness::build(&alice, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);
    harness
        .coordinator
        .submit_removal(&device("bob"), 1000)
        .await
        .unwrap();
    let state = harness.repository.load_state().await.unwrap().unwrap();
    let digest = state.convergence().convergence_digest();

    // 构造一个"更新"摘要的恢复资料(目标集合不同),模拟越级资料。
    let wrong_material = uc_core::membership::RemovalRecoveryMaterial {
        convergence_digest: [0u8; 32],
        effective_members: sorted_view(vec![instance("alice", 1), instance("charlie", 3)]),
        epoch: 5,
        commit: b"fake-commit".to_vec(),
        welcome: Some(b"fake-welcome".to_vec()),
        encrypted_key_catalog: b"fake-catalog".to_vec(),
    };

    harness
        .coordinator
        .ingest_exchange(
            &device("charlie"),
            uc_core::membership::RemovalExchangeMessage::RecoveryMaterial(wrong_material),
            1001,
        )
        .await
        .unwrap();

    let state = harness.repository.load_state().await.unwrap().unwrap();
    assert!(state.applied_digest.is_none());
    assert!(harness.recovery.applied.lock().unwrap().is_empty());
    assert_eq!(state.phase, RemovalPhase::Applied);

    // 正确的恢复资料按序应用后,摘要与成员集合都匹配。
    let correct_material = uc_core::membership::RemovalRecoveryMaterial {
        convergence_digest: digest,
        effective_members: sorted_view(vec![instance("alice", 1), instance("charlie", 3)]),
        epoch: 4,
        commit: b"fake-commit".to_vec(),
        welcome: Some(b"fake-welcome".to_vec()),
        encrypted_key_catalog: b"fake-catalog".to_vec(),
    };
    harness
        .coordinator
        .ingest_exchange(
            &device("charlie"),
            uc_core::membership::RemovalExchangeMessage::RecoveryMaterial(correct_material),
            1002,
        )
        .await
        .unwrap();
    let state = harness.repository.load_state().await.unwrap().unwrap();
    assert_eq!(state.applied_digest, Some(digest));
    assert_eq!(harness.recovery.applied.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn removed_device_receives_a_notice_and_marks_itself_removed() {
    // N01:B 离线期间被 A 移除;A 的后台推进向 B 投递移除通知,B 验收后
    // QueryMemberRemoval 返回 removed: true,且 A 不重复投递。
    let alice = device("alice");
    let alice_harness = Harness::build(&alice, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);
    alice_harness
        .coordinator
        .submit_removal(&device("bob"), 1000)
        .await
        .unwrap();
    alice_harness.coordinator.reconcile(1001).await.unwrap();

    let state_before = alice_harness
        .repository
        .load_state()
        .await
        .unwrap()
        .unwrap();
    eprintln!(
        "N01 intents={} targets={:?} phase={:?}",
        state_before.intents.len(),
        state_before
            .intents
            .iter()
            .map(|i| format!("{:?}", i.content.target))
            .collect::<Vec<_>>(),
        state_before.phase
    );

    let sent = alice_harness.notice_exchange.sent.lock().unwrap().clone();
    eprintln!("N01 sent notices: {:?}", sent.len());
    eprintln!(
        "N01 state: notified={:?} locally_removed_devices={:?} member_devices={:?}",
        alice_harness
            .repository
            .load_state()
            .await
            .unwrap()
            .map(|s| s.notified_removals),
        alice_harness
            .repository
            .load_state()
            .await
            .unwrap()
            .map(|s| s.locally_removed_devices),
        alice_harness
            .repository
            .load_state()
            .await
            .unwrap()
            .map(|s| s.member_devices),
    );
    assert_eq!(sent.len(), 1);
    let (recipient, notice) = &sent[0];
    assert_eq!(recipient, &device("bob"));
    assert_eq!(notice.target_instance, instance("bob", 2));
    assert_eq!(notice.issuer_instance, instance("alice", 1));

    // B 上线,收到 A 的通知并验收。
    let bob = device("bob");
    let bob_harness = Harness::build(&bob, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);
    let acceptance = bob_harness
        .coordinator
        .handle_notice(notice.clone(), 1002)
        .await
        .unwrap();
    assert!(matches!(
        acceptance,
        uc_core::membership::RemovalNoticeAcceptance::Accepted { .. }
    ));
    let state = bob_harness.repository.load_state().await.unwrap().unwrap();
    assert_eq!(state.self_removed, Some(notice.intent_id));
    let summary = bob_harness.coordinator.query(1003).await.unwrap();
    assert!(summary.removed);
    assert_eq!(summary.phase, RemovalPhase::Applied);

    // A 的记录进度避免重复投递。
    alice_harness.coordinator.reconcile(1004).await.unwrap();
    assert_eq!(alice_harness.notice_exchange.sent.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn another_member_can_forward_the_notice_after_the_initiator_is_gone() {
    // N02:发起者 A 永久离线后,已验收意图的 C 也能补发通知。
    let alice = device("alice");
    let alice_harness = Harness::build(&alice, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);
    alice_harness
        .coordinator
        .submit_removal(&device("bob"), 1000)
        .await
        .unwrap();

    // C 验收 A 的意图后自行推进并补发通知。
    let charlie = device("charlie");
    let charlie_harness = Harness::build(&charlie, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);
    let intent = alice_harness
        .repository
        .load_state()
        .await
        .unwrap()
        .unwrap()
        .intents
        .into_iter()
        .next()
        .unwrap();
    charlie_harness
        .coordinator
        .ingest_exchange(
            &alice,
            uc_core::membership::RemovalExchangeMessage::Intent(Box::new(intent)),
            1001,
        )
        .await
        .unwrap();
    charlie_harness.coordinator.reconcile(1002).await.unwrap();

    let sent = charlie_harness.notice_exchange.sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    let (recipient, notice) = &sent[0];
    assert_eq!(recipient, &device("bob"));
    assert_eq!(notice.issuer_instance, instance("charlie", 3));

    let bob = device("bob");
    let bob_harness = Harness::build(&bob, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);
    let acceptance = bob_harness
        .coordinator
        .handle_notice(notice.clone(), 1003)
        .await
        .unwrap();
    assert!(matches!(
        acceptance,
        uc_core::membership::RemovalNoticeAcceptance::Accepted { .. }
    ));
    assert!(bob_harness.coordinator.query(1004).await.unwrap().removed);
}

#[tokio::test]
async fn online_removal_marks_self_removed_via_the_intent() {
    // B 在线时验收移除自己的意图,与移除通知写同一标记,查询立即为 removed。
    let bob = device("bob");
    let bob_harness = Harness::build(&bob, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);
    let initiator = instance("alice", 1);
    let target = instance("bob", 2);
    let content = uc_core::membership::RemovalIntentContent {
        space_lineage: "space-a".to_owned(),
        view_epoch: 1,
        view_members: sorted_view(vec![initiator, target, instance("charlie", 3)]),
        initiator,
        target,
    };
    let intent = signed_intent(content);
    bob_harness
        .coordinator
        .ingest_exchange(
            &device("alice"),
            uc_core::membership::RemovalExchangeMessage::Intent(Box::new(intent.clone())),
            1000,
        )
        .await
        .unwrap();
    let state = bob_harness.repository.load_state().await.unwrap().unwrap();
    assert_eq!(state.self_removed, Some(intent.intent_id));
    assert!(bob_harness.coordinator.query(1001).await.unwrap().removed);
}

#[tokio::test]
async fn removed_device_rejects_creating_a_new_intent_after_the_notice() {
    // N06/L07:收到移除通知后,本机经正常入口移除他人被立即拒绝。
    let alice = device("alice");
    let alice_harness = Harness::build(&alice, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);
    alice_harness
        .coordinator
        .submit_removal(&device("bob"), 1000)
        .await
        .unwrap();
    alice_harness.coordinator.reconcile(1001).await.unwrap();
    let (_, notice) = alice_harness
        .notice_exchange
        .sent
        .lock()
        .unwrap()
        .first()
        .cloned()
        .unwrap();

    let bob = device("bob");
    let bob_harness = Harness::build(&bob, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);
    bob_harness
        .coordinator
        .handle_notice(notice, 1002)
        .await
        .unwrap();
    let error = bob_harness
        .coordinator
        .submit_removal(&device("charlie"), 1003)
        .await
        .unwrap_err();
    assert!(matches!(error, RemovalCoordinatorError::OwnInstanceRemoved));
}

#[tokio::test]
async fn notice_from_another_space_is_rejected_without_state_change() {
    // N04:空间指纹不符的通知被拒绝,状态不变。
    let alice = device("alice");
    let alice_harness = Harness::build(&alice, vec![("alice", 1), ("bob", 2)]);
    alice_harness
        .coordinator
        .submit_removal(&device("bob"), 1000)
        .await
        .unwrap();
    alice_harness.coordinator.reconcile(1001).await.unwrap();
    let (_, notice) = alice_harness
        .notice_exchange
        .sent
        .lock()
        .unwrap()
        .first()
        .cloned()
        .unwrap();

    let bob = device("bob");
    let bob_harness = Harness::build(&bob, vec![("alice", 1), ("bob", 2)]);
    bob_harness
        .coordinator
        .handle_notice(
            uc_core::membership::RemovalNotice {
                space_lineage_fingerprint: [9; 32],
                ..notice.clone()
            },
            1002,
        )
        .await
        .unwrap();
    let state = bob_harness.repository.load_state().await.unwrap();
    assert!(state.is_none() || state.unwrap().self_removed.is_none());
}

#[tokio::test]
async fn notice_from_an_unknown_issuer_is_rejected() {
    // N04:签发者不在本机保存的视图资料中 → 拒绝且状态不变。
    let alice = device("alice");
    let alice_harness = Harness::build(&alice, vec![("alice", 1), ("bob", 2)]);
    alice_harness
        .coordinator
        .submit_removal(&device("bob"), 1000)
        .await
        .unwrap();
    alice_harness.coordinator.reconcile(1001).await.unwrap();
    let (_, notice) = alice_harness
        .notice_exchange
        .sent
        .lock()
        .unwrap()
        .first()
        .cloned()
        .unwrap();

    let bob = device("bob");
    let bob_harness = Harness::build(&bob, vec![("alice", 1), ("bob", 2)]);
    let acceptance = bob_harness
        .coordinator
        .handle_notice(
            uc_core::membership::RemovalNotice {
                issuer_instance: instance("mallory", 5),
                ..notice
            },
            1002,
        )
        .await
        .unwrap();
    assert!(matches!(
        acceptance,
        uc_core::membership::RemovalNoticeAcceptance::Rejected {
            reason: uc_core::membership::RemovalNoticeRejectionReason::Invalid
        }
    ));
    let state = bob_harness.repository.load_state().await.unwrap();
    assert!(state.is_none() || state.unwrap().self_removed.is_none());
}

#[tokio::test]
async fn notice_with_a_bad_signature_is_rejected() {
    // N04:签名验证失败 → 拒绝且状态不变。
    let alice = device("alice");
    let alice_harness = Harness::build(&alice, vec![("alice", 1), ("bob", 2)]);
    alice_harness
        .coordinator
        .submit_removal(&device("bob"), 1000)
        .await
        .unwrap();
    alice_harness.coordinator.reconcile(1001).await.unwrap();
    let (_, notice) = alice_harness
        .notice_exchange
        .sent
        .lock()
        .unwrap()
        .first()
        .cloned()
        .unwrap();

    let bob = device("bob");
    let bob_harness = Harness::build(&bob, vec![("alice", 1), ("bob", 2)]);
    *bob_harness.notice_verifier.failure.lock().unwrap() =
        Some(uc_core::membership::RemovalNoticeVerificationError::BadSignature);
    let acceptance = bob_harness
        .coordinator
        .handle_notice(notice, 1002)
        .await
        .unwrap();
    assert!(matches!(
        acceptance,
        uc_core::membership::RemovalNoticeAcceptance::Rejected {
            reason: uc_core::membership::RemovalNoticeRejectionReason::Invalid
        }
    ));
    let state = bob_harness.repository.load_state().await.unwrap();
    assert!(state.is_none() || state.unwrap().self_removed.is_none());
}

#[tokio::test]
async fn duplicate_notice_is_idempotent() {
    // N05:重复通知幂等,第二次返回“已知”。
    let alice = device("alice");
    let alice_harness = Harness::build(&alice, vec![("alice", 1), ("bob", 2)]);
    alice_harness
        .coordinator
        .submit_removal(&device("bob"), 1000)
        .await
        .unwrap();
    alice_harness.coordinator.reconcile(1001).await.unwrap();
    let (_, notice) = alice_harness
        .notice_exchange
        .sent
        .lock()
        .unwrap()
        .first()
        .cloned()
        .unwrap();

    let bob = device("bob");
    let bob_harness = Harness::build(&bob, vec![("alice", 1), ("bob", 2)]);
    let first = bob_harness
        .coordinator
        .handle_notice(notice.clone(), 1002)
        .await
        .unwrap();
    assert!(matches!(
        first,
        uc_core::membership::RemovalNoticeAcceptance::Accepted { .. }
    ));
    let second = bob_harness
        .coordinator
        .handle_notice(notice, 1003)
        .await
        .unwrap();
    assert!(matches!(
        second,
        uc_core::membership::RemovalNoticeAcceptance::AlreadyKnown { .. }
    ));
}

#[tokio::test]
async fn notice_failure_does_not_block_convergence() {
    // N07:通知不可达不阻塞收敛与完成判定;失败只保留待处理,后续重试。
    let alice = device("alice");
    let alice_harness = Harness::build(&alice, vec![("alice", 1), ("bob", 2), ("charlie", 3)]);
    alice_harness
        .coordinator
        .submit_removal(&device("bob"), 1000)
        .await
        .unwrap();
    // 目标持续离线:本机推进不受影响,通知进度保持为空。
    *alice_harness.notice_exchange.offline.lock().unwrap() = true;
    alice_harness.coordinator.reconcile(1001).await.unwrap();
    let sent = alice_harness.notice_exchange.sent.lock().unwrap().clone();
    assert!(sent.is_empty());
    let state = alice_harness
        .repository
        .load_state()
        .await
        .unwrap()
        .unwrap();
    assert!(state.notified_removals.is_empty());
    assert_eq!(state.phase, RemovalPhase::Converging);

    // 目标上线:同一后台推进补发并记录进度。
    *alice_harness.notice_exchange.offline.lock().unwrap() = false;
    alice_harness.coordinator.reconcile(1002).await.unwrap();
    assert_eq!(alice_harness.notice_exchange.sent.lock().unwrap().len(), 1);
    let state = alice_harness
        .repository
        .load_state()
        .await
        .unwrap()
        .unwrap();
    assert!(state
        .notified_removals
        .contains(&state.intents[0].intent_id));
}

#[tokio::test]
async fn readmission_clears_a_stale_self_removed_marker() {
    // N08:重新加入产生新实例后,旧标记被清除,新实例不受影响。
    let alice = device("alice");
    let alice_harness = Harness::build(&alice, vec![("alice", 1), ("bob", 2)]);
    alice_harness
        .coordinator
        .submit_removal(&device("bob"), 1000)
        .await
        .unwrap();
    alice_harness.coordinator.reconcile(1001).await.unwrap();
    let (_, notice) = alice_harness
        .notice_exchange
        .sent
        .lock()
        .unwrap()
        .first()
        .cloned()
        .unwrap();

    let bob = device("bob");
    let bob_harness = Harness::build(&bob, vec![("alice", 1), ("bob", 2)]);
    bob_harness
        .coordinator
        .handle_notice(notice, 1002)
        .await
        .unwrap();
    assert!(bob_harness.coordinator.query(1003).await.unwrap().removed);

    // B 重新加入:当前视图产生新成员实例(新签名密钥)。
    *bob_harness.recovery.members.lock().unwrap() = vec![
        (device("alice"), instance("alice", 1)),
        (device("bob"), instance("bob", 9)),
    ];
    let summary = bob_harness.coordinator.query(1004).await.unwrap();
    assert!(!summary.removed);
    let state = bob_harness.repository.load_state().await.unwrap().unwrap();
    assert!(state.self_removed.is_none());
}

#[tokio::test]
async fn stale_notice_does_not_relock_a_readmitted_instance() {
    // N08:重新加入后重放的旧通知被拒绝,不会再次锁定新实例。
    let alice = device("alice");
    let alice_harness = Harness::build(&alice, vec![("alice", 1), ("bob", 2)]);
    alice_harness
        .coordinator
        .submit_removal(&device("bob"), 1000)
        .await
        .unwrap();
    alice_harness.coordinator.reconcile(1001).await.unwrap();
    let (_, notice) = alice_harness
        .notice_exchange
        .sent
        .lock()
        .unwrap()
        .first()
        .cloned()
        .unwrap();

    let bob = device("bob");
    let bob_harness = Harness::build(&bob, vec![("alice", 1), ("bob", 2)]);
    bob_harness
        .coordinator
        .handle_notice(notice.clone(), 1002)
        .await
        .unwrap();
    *bob_harness.recovery.members.lock().unwrap() = vec![
        (device("alice"), instance("alice", 1)),
        (device("bob"), instance("bob", 9)),
    ];
    bob_harness.coordinator.query(1004).await.unwrap();

    let acceptance = bob_harness
        .coordinator
        .handle_notice(notice, 1005)
        .await
        .unwrap();
    assert!(matches!(
        acceptance,
        uc_core::membership::RemovalNoticeAcceptance::Rejected {
            reason: uc_core::membership::RemovalNoticeRejectionReason::Invalid
        }
    ));
    let state = bob_harness.repository.load_state().await.unwrap().unwrap();
    assert!(state.self_removed.is_none());
}
