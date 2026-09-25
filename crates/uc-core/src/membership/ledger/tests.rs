use sha2::{Digest, Sha256};

use crate::ids::DeviceId;
use crate::membership::{
    AdmissionActivationReceipt, AdmissionChangeFacts, HistoricalMembershipSignatureError,
    HistoricalMembershipSignatureVerifier, MembershipAdmissionV2, MembershipCredential,
    MembershipEventId, MembershipEventV2, MembershipOperationV2, RemovalDecision,
    VersionedMembershipHistory, ED25519_SIGNATURE_ALGORITHM_V1, MEMBERSHIP_EVENT_FORMAT_V2,
};
use crate::security::IdentityFingerprint;

use super::{
    LedgerDeliveryKind, LedgerDeliveryResult, LedgerEffect, LedgerFollowUp, LedgerInput,
    LedgerMemberStatus, LedgerOutcome, LedgerTransitionError, LedgerUpdateProblem,
    LedgerUpdateView, LedgerWork, MemberEffectPhase, MembershipLedger, PeerEvidence, PeerLink,
    PeerLinkSnapshot, PeerPauseReason, PeerRelation, PeerRelationView, PeerSyncResult,
    PeerSyncView, SecurityDeliveryStatus, DEPARTURE_WINDOW_MS,
};

const LINEAGE: &str = "space-membership-lineage";
const NOW: i64 = 1_000_000;

struct TestVerifier;

impl TestVerifier {
    fn sign(&self, credential: &MembershipCredential, payload: &[u8]) -> Vec<u8> {
        let mut hasher = Sha256::new();
        hasher.update(b"space-membership-test-signature\0");
        hasher.update(&credential.public_key);
        hasher.update(payload);
        hasher.finalize().to_vec()
    }
}

impl HistoricalMembershipSignatureVerifier for TestVerifier {
    fn verify(
        &self,
        signature_algorithm_version: u16,
        public_key: &[u8],
        payload: &[u8],
        signature: &[u8],
    ) -> Result<bool, HistoricalMembershipSignatureError> {
        let credential =
            MembershipCredential::new(signature_algorithm_version, public_key.to_vec());
        Ok(self.sign(&credential, payload) == signature)
    }
}

fn admission(device: &str, byte: u8) -> MembershipAdmissionV2 {
    let credential = MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![byte; 32]);
    let device_id = DeviceId::new(device);
    MembershipAdmissionV2 {
        facts: AdmissionChangeFacts {
            member_instance: credential.member_instance_id(&device_id),
            device_id,
            device_name: device.to_owned(),
            identity_fingerprint: IdentityFingerprint::from_display_string("ABCD-EFGH-IJKL-MNOP")
                .unwrap(),
            transport_public_key: vec![1],
            transport_address_blob: vec![2],
            identity_signature: vec![3],
        },
        membership_credential: credential,
        resume_public_key_digest: [7; 32],
        security_commitment_id: [8; 32],
    }
}

fn signed_event(
    history: &VersionedMembershipHistory,
    author: &MembershipAdmissionV2,
    operation: MembershipOperationV2,
    marker: u8,
) -> MembershipEventV2 {
    let parent = history.current_head();
    let resulting_members_digest = history
        .expected_resulting_members_digest(parent, &operation)
        .unwrap();
    let mut event = MembershipEventV2::new(
        MEMBERSHIP_EVENT_FORMAT_V2,
        LINEAGE.to_owned(),
        parent,
        parent
            .map(|parent| history.depth(parent).unwrap() + 1)
            .unwrap_or(0),
        [marker; 16],
        author.facts.member_instance,
        author.membership_credential.credential_id,
        author.membership_credential.signature_algorithm_version,
        operation,
        resulting_members_digest,
        [marker.wrapping_add(1); 32],
        vec![marker],
        Some([marker.wrapping_add(2); 32]),
        Vec::new(),
    );
    event.signature = TestVerifier.sign(&author.membership_credential, &event.signing_payload());
    event
}

fn activate(
    history: &mut VersionedMembershipHistory,
    event: &MembershipEventV2,
    admitted: &MembershipAdmissionV2,
) {
    let mut receipt = AdmissionActivationReceipt::new(
        1,
        [event.operation_id[0]; 32],
        event.event_id(),
        event.resulting_members_digest,
        admitted.security_commitment_id,
        admitted.facts.member_instance,
        Vec::new(),
    );
    receipt.signature =
        TestVerifier.sign(&admitted.membership_credential, &receipt.signing_payload());
    history
        .verify_and_record_activation_receipt(receipt, &TestVerifier)
        .unwrap();
}

/// 由设备 A 创建、依次加入并激活其余设备的共同历史。
struct Group {
    history: VersionedMembershipHistory,
    members: Vec<MembershipAdmissionV2>,
}

impl Group {
    fn new(devices: &[&str]) -> Self {
        let members: Vec<MembershipAdmissionV2> = devices
            .iter()
            .enumerate()
            .map(|(index, device)| admission(device, 0x40 + index as u8))
            .collect();
        let mut history = VersionedMembershipHistory::new(LINEAGE.to_owned());
        for (index, member) in members.iter().enumerate() {
            let event = signed_event(
                &history,
                &members[0],
                MembershipOperationV2::AddDevice {
                    admission: member.clone(),
                },
                index as u8 + 1,
            );
            history
                .verify_and_receive_event(event.clone(), &TestVerifier)
                .unwrap();
            if index > 0 {
                activate(&mut history, &event, member);
            }
        }
        Self { history, members }
    }

    fn member(&self, device: &str) -> &MembershipAdmissionV2 {
        self.members
            .iter()
            .find(|member| member.facts.device_id.as_str() == device)
            .unwrap()
    }

    fn start(&self, local: &str) -> MembershipLedger {
        let local = self.member(local);
        MembershipLedger::start(
            self.history.clone(),
            local.facts.device_id,
            local.facts.member_instance,
            10,
        )
        .unwrap()
    }

    /// `author` 在 `history` 上签名移除 `target`，返回新历史与移除事件。
    fn removal(
        &self,
        history: &VersionedMembershipHistory,
        author: &str,
        target: &str,
        marker: u8,
    ) -> (VersionedMembershipHistory, MembershipEventV2) {
        let author = self.member(author);
        let mut event = history
            .create_unsigned_local_removal_event(
                author.facts.member_instance,
                &author.membership_credential,
                self.member(target).facts.member_instance,
                [marker; 16],
                [marker; 32],
            )
            .unwrap();
        event.signature =
            TestVerifier.sign(&author.membership_credential, &event.signing_payload());
        let mut next = history.clone();
        next.verify_and_receive_event(event.clone(), &TestVerifier)
            .unwrap();
        (next, event)
    }

    /// `local` 收到 `remote` 历史中的移除，返回待决定的新历史。
    fn received(
        &self,
        local_history: &VersionedMembershipHistory,
        remote: &VersionedMembershipHistory,
        local: &str,
    ) -> VersionedMembershipHistory {
        let mut next = local_history.clone();
        next.merge_remote_history(
            remote,
            self.member(local).facts.member_instance,
            &TestVerifier,
        )
        .unwrap();
        next
    }

    fn decided(
        &self,
        history: &VersionedMembershipHistory,
        removal: MembershipEventId,
        local: &str,
        decision: RemovalDecision,
    ) -> VersionedMembershipHistory {
        let local = self.member(local);
        let mut signed = history
            .create_unsigned_local_removal_decision(
                removal,
                local.facts.member_instance,
                &local.membership_credential,
                decision,
                [9; 16],
            )
            .unwrap();
        signed.signature =
            TestVerifier.sign(&local.membership_credential, &signed.signing_payload());
        let mut next = history.clone();
        next.apply_signed_local_removal_decision(
            signed,
            local.facts.member_instance,
            &TestVerifier,
        )
        .unwrap();
        next
    }
}

fn device(name: &str) -> DeviceId {
    DeviceId::new(name)
}

fn apply(
    membership: MembershipLedger,
    input: LedgerInput,
    now_ms: i64,
) -> (MembershipLedger, LedgerOutcome, Vec<LedgerEffect>) {
    membership.apply(input, now_ms).unwrap().into_parts()
}

fn view_of(membership: &MembershipLedger) -> super::LedgerView {
    membership
        .present(SecurityDeliveryStatus::Completed)
        .unwrap()
}

fn device_view<'a>(view: &'a super::LedgerView, name: &str) -> Option<&'a super::LedgerDeviceView> {
    view.devices
        .iter()
        .find(|device| device.device_id.as_str() == name)
}

/// 以全部成功的结果执行所有已到期的阻塞待办，直到没有可执行项。
fn run_due_work(mut membership: MembershipLedger, now_ms: i64) -> MembershipLedger {
    for _ in 0..64 {
        let due: Vec<_> = membership
            .outstanding_work(now_ms)
            .unwrap()
            .into_iter()
            .filter(|work| work.due_at_ms <= now_ms)
            .collect();
        let Some(next) = due.into_iter().next() else {
            return membership;
        };
        let input = success_input(&membership, next.work);
        let (replacement, outcome, _) = apply(membership, input, now_ms);
        assert_ne!(outcome, LedgerOutcome::Stale);
        membership = replacement;
    }
    panic!("due work did not settle");
}

fn success_input(membership: &MembershipLedger, work: LedgerWork) -> LedgerInput {
    match work {
        LedgerWork::AdvanceEffect(effect) => LedgerInput::EffectStepFinished {
            event_id: effect.event_id(),
            from: effect.phase(),
        },
        LedgerWork::DeliverRemovalNotice { peer, .. } => LedgerInput::DeliveryFinished {
            peer,
            delivery: LedgerDeliveryKind::RemovalNotice,
            result: LedgerDeliveryResult::Delivered,
        },
        LedgerWork::EndDeparture { peer } => LedgerInput::DepartureWindowElapsed { peer },
        LedgerWork::DeliverDecision { peer, .. } => LedgerInput::DeliveryFinished {
            peer,
            delivery: LedgerDeliveryKind::Decision,
            result: LedgerDeliveryResult::Delivered,
        },
        LedgerWork::SynchronizeHistory { peer } => LedgerInput::HistorySyncFinished {
            peer,
            synced_position: membership.history().current_position().unwrap(),
            result: PeerSyncResult::Confirmed,
        },
    }
}

#[test]
fn joined_member_starts_consistent_but_awaits_confirmation_of_its_position() {
    let group = Group::new(&["device-a", "device-b"]);
    let membership = group.start("device-b");

    assert!(matches!(
        membership.peer(&device("device-a")),
        Some(PeerLink::Member(link)) if link.relation() == PeerRelation::Consistent
    ));
    let view = view_of(&membership);
    assert_eq!(view.local_status, LedgerMemberStatus::Active);
    assert_eq!(
        device_view(&view, "device-a").unwrap().relation,
        PeerRelationView::ConfirmationPending
    );
    assert_eq!(view.device_update, LedgerUpdateView::Updating);

    let settled = run_due_work(membership, NOW);
    assert_eq!(view_of(&settled).device_update, LedgerUpdateView::Completed);
    assert_eq!(
        view_of(&settled).scope.usable_peer_device_ids,
        vec![device("device-a")]
    );
}

#[test]
fn start_rejects_a_local_member_outside_the_history() {
    let group = Group::new(&["device-a"]);
    let stranger = admission("device-z", 0x7a);

    let result = MembershipLedger::start(
        group.history.clone(),
        stranger.facts.device_id,
        stranger.facts.member_instance,
        1,
    );

    assert_eq!(result.unwrap_err(), LedgerTransitionError::InputMismatch);
}

// R1：移除通知送达后，被移除设备从移除方的设备列表中消失，设备更新完成。
#[test]
fn delivered_removal_notice_ends_the_departure() {
    let group = Group::new(&["device-a", "device-b"]);
    let membership = run_due_work(group.start("device-a"), NOW);
    let (history, _) = group.removal(membership.history(), "device-a", "device-b", 0x31);

    let (removed, outcome, effects) = apply(
        membership,
        LedgerInput::LocalRemovalSigned {
            history,
            retained_device_ids: Vec::new(),
        },
        NOW,
    );

    assert_eq!(outcome, LedgerOutcome::Applied);
    assert_eq!(removed.revision(), 12);
    assert_eq!(
        effects,
        vec![
            LedgerEffect::AfterCommit(LedgerFollowUp::PublishDeviceTrustChange),
            LedgerEffect::AfterCommit(LedgerFollowUp::WakeWorker),
        ]
    );
    let view = view_of(&removed);
    let departing = device_view(&view, "device-b").unwrap();
    assert_eq!(departing.status, LedgerMemberStatus::PendingActivation);
    assert_eq!(
        departing.relation,
        PeerRelationView::AwaitingRemovalAcknowledgement
    );
    assert_eq!(
        departing.sync,
        PeerSyncView::Paused(PeerPauseReason::LocalMemberInactive)
    );

    let effects_done = run_effects_only(removed);
    let view = view_of(&effects_done);
    assert_eq!(view.device_update, LedgerUpdateView::Completed);
    assert!(effects_done
        .outstanding_work(NOW)
        .unwrap()
        .iter()
        .all(|work| !work.blocks_device_update));

    let (delivered, outcome, _) = apply(
        effects_done,
        LedgerInput::DeliveryFinished {
            peer: device("device-b"),
            delivery: LedgerDeliveryKind::RemovalNotice,
            result: LedgerDeliveryResult::Delivered,
        },
        NOW,
    );
    assert_eq!(outcome, LedgerOutcome::Applied);
    assert!(delivered.peer(&device("device-b")).is_none());
    assert!(device_view(&view_of(&delivered), "device-b").is_none());
    assert!(delivered.outstanding_work(NOW).unwrap().is_empty());
}

fn run_effects_only(mut membership: MembershipLedger) -> MembershipLedger {
    loop {
        let Some(effect) = membership.unfinished_effects().next().cloned() else {
            return membership;
        };
        membership = apply(
            membership,
            LedgerInput::EffectStepFinished {
                event_id: effect.event_id(),
                from: effect.phase(),
            },
            NOW,
        )
        .0;
    }
}

// 被移除设备离线时，移除方在窗口到期后结束通知责任。
#[test]
fn undelivered_departure_ends_exactly_at_the_window_boundary() {
    let group = Group::new(&["device-a", "device-b"]);
    let membership = run_due_work(group.start("device-a"), NOW);
    let (history, _) = group.removal(membership.history(), "device-a", "device-b", 0x32);
    let removed = run_effects_only(
        apply(
            membership,
            LedgerInput::LocalRemovalSigned {
                history,
                retained_device_ids: Vec::new(),
            },
            NOW,
        )
        .0,
    );
    let expires_at = NOW + DEPARTURE_WINDOW_MS;
    let deferred = apply(
        removed,
        LedgerInput::DeliveryFinished {
            peer: device("device-b"),
            delivery: LedgerDeliveryKind::RemovalNotice,
            result: LedgerDeliveryResult::Deferred,
        },
        NOW,
    );
    assert_eq!(deferred.1, LedgerOutcome::Unchanged);

    let early = apply(
        deferred.0,
        LedgerInput::DepartureWindowElapsed {
            peer: device("device-b"),
        },
        expires_at - 1,
    );
    assert_eq!(early.1, LedgerOutcome::Unchanged);
    let notice_at_boundary = early.0.outstanding_work(expires_at).unwrap();
    assert_eq!(
        notice_at_boundary
            .iter()
            .map(|work| &work.work)
            .collect::<Vec<_>>(),
        vec![&LedgerWork::EndDeparture {
            peer: device("device-b")
        }]
    );

    let (ended, outcome, _) = apply(
        early.0,
        LedgerInput::DepartureWindowElapsed {
            peer: device("device-b"),
        },
        expires_at,
    );
    assert_eq!(outcome, LedgerOutcome::Applied);
    assert!(ended.peer(&device("device-b")).is_none());
}

/// 返回 B 视角：已收到 A 对 B 的移除，等待本机决定。
fn removed_device_awaiting_decision(group: &Group) -> (MembershipLedger, MembershipEventId) {
    let local = run_due_work(group.start("device-b"), NOW);
    let (remote, removal) = group.removal(&group.history, "device-a", "device-b", 0x33);
    let received = group.received(local.history(), &remote, "device-b");
    let (awaiting, outcome, _) = apply(
        local,
        LedgerInput::PeerEvidenceReconciled {
            source: device("device-a"),
            history: Some(received),
            evidence: PeerEvidence::Confirmed,
        },
        NOW,
    );
    assert_eq!(outcome, LedgerOutcome::Applied);
    (awaiting, removal.event_id())
}

#[test]
fn received_removal_waits_for_the_local_decision() {
    let group = Group::new(&["device-a", "device-b"]);
    let (awaiting, _) = removed_device_awaiting_decision(&group);

    let view = view_of(&awaiting);
    assert_eq!(view.local_status, LedgerMemberStatus::Active);
    assert_eq!(
        device_view(&view, "device-a").unwrap().relation,
        PeerRelationView::PendingLocalDecision
    );
    assert_eq!(
        view.device_update,
        LedgerUpdateView::NeedsAttention(LedgerUpdateProblem::DeviceRelationshipConflict)
    );
}

// R2：被移除方接受后进入已移除终态，不排队任何决定投递，也没有后续待办。
#[test]
fn accepted_local_removal_is_a_terminal_state_without_outstanding_work() {
    let group = Group::new(&["device-a", "device-b"]);
    let (awaiting, removal) = removed_device_awaiting_decision(&group);
    let history = group.decided(
        awaiting.history(),
        removal,
        "device-b",
        RemovalDecision::Accept,
    );

    let (accepted, outcome, _) = apply(
        awaiting,
        LedgerInput::LocalDecisionSigned {
            history,
            removal_event_id: removal,
        },
        NOW,
    );
    assert_eq!(outcome, LedgerOutcome::Applied);
    assert!(matches!(
        accepted.peer(&device("device-a")),
        Some(PeerLink::Member(link)) if link.outgoing_decision().is_none()
    ));
    assert_eq!(
        accepted.local_status(),
        LedgerMemberStatus::PendingActivation
    );

    let settled = run_due_work(accepted, NOW);
    let view = view_of(&settled);
    assert_eq!(view.local_status, LedgerMemberStatus::Removed);
    assert_eq!(
        device_view(&view, "device-b").unwrap().sync,
        PeerSyncView::Paused(PeerPauseReason::LocalMemberInactive)
    );
    assert_eq!(view.device_update, LedgerUpdateView::Completed);
    assert!(settled.outstanding_work(NOW).unwrap().is_empty());
    assert!(settled
        .outstanding_work(NOW + DEPARTURE_WINDOW_MS * 10)
        .unwrap()
        .is_empty());
}

#[test]
fn inbound_admission_follows_the_local_status_and_the_peer_relation() {
    let group = Group::new(&["device-a", "device-b"]);
    let sponsor = run_due_work(group.start("device-a"), NOW);
    assert!(sponsor.admits_inbound_peer(&device("device-b")));
    assert!(!sponsor.admits_inbound_peer(&device("device-c")));

    // 被移除的对端进入离开窗口后立即拒绝，不等通知送达。
    let (history, _) = group.removal(sponsor.history(), "device-a", "device-b", 0x35);
    let (removed, _, _) = apply(
        sponsor,
        LedgerInput::LocalRemovalSigned {
            history,
            retained_device_ids: Vec::new(),
        },
        NOW,
    );
    assert!(matches!(
        removed.peer(&device("device-b")),
        Some(PeerLink::Departing(_))
    ));
    assert!(!removed.admits_inbound_peer(&device("device-b")));

    // 等待本机决定移除时仍放行发起方，接受后本机已移除即全部拒绝。
    let (awaiting, removal) = removed_device_awaiting_decision(&group);
    assert!(awaiting.admits_inbound_peer(&device("device-a")));
    let history = group.decided(
        awaiting.history(),
        removal,
        "device-b",
        RemovalDecision::Accept,
    );
    let (accepted, _, _) = apply(
        awaiting,
        LedgerInput::LocalDecisionSigned {
            history,
            removal_event_id: removal,
        },
        NOW,
    );
    let settled = run_due_work(accepted, NOW);
    assert_eq!(settled.local_status(), LedgerMemberStatus::Removed);
    assert!(!settled.admits_inbound_peer(&device("device-a")));
}

#[test]
fn read_model_keeps_a_departing_device_until_its_departure_ends() {
    let group = Group::new(&["device-a", "device-b", "device-c"]);
    let membership = run_due_work(group.start("device-a"), NOW);
    let devices = |model: &super::LedgerReadModel| -> Vec<DeviceId> {
        model.members.iter().map(|facts| facts.device_id).collect()
    };

    let model = membership.read_model().unwrap();
    assert_eq!(
        devices(&model),
        vec![device("device-a"), device("device-b"), device("device-c")]
    );
    assert_eq!(
        model.trusted_device_ids.into_iter().collect::<Vec<_>>(),
        vec![device("device-b"), device("device-c")]
    );

    let (history, _) = group.removal(membership.history(), "device-a", "device-b", 0x36);
    let (removed, _, _) = apply(
        membership,
        LedgerInput::LocalRemovalSigned {
            history,
            retained_device_ids: vec![device("device-c")],
        },
        NOW,
    );
    let model = removed.read_model().unwrap();
    assert!(devices(&model).contains(&device("device-b")));
    assert_eq!(
        model.trusted_device_ids.into_iter().collect::<Vec<_>>(),
        vec![device("device-c")]
    );

    let settled = run_due_work(removed, NOW);
    assert!(settled.peer(&device("device-b")).is_none());
    assert_eq!(
        devices(&settled.read_model().unwrap()),
        vec![device("device-a"), device("device-c")]
    );
}

// R3：被移除方拒绝后把发起方标为分叉，稳定停在需要处理且没有无法完成的投递。
#[test]
fn rejected_local_removal_diverges_without_an_undeliverable_decision() {
    let group = Group::new(&["device-a", "device-b"]);
    let (awaiting, removal) = removed_device_awaiting_decision(&group);
    let history = group.decided(
        awaiting.history(),
        removal,
        "device-b",
        RemovalDecision::Reject,
    );

    let (rejected, _, _) = apply(
        awaiting,
        LedgerInput::LocalDecisionSigned {
            history,
            removal_event_id: removal,
        },
        NOW,
    );

    assert!(matches!(
        rejected.peer(&device("device-a")),
        Some(PeerLink::Member(link))
            if link.relation() == PeerRelation::Diverged && link.outgoing_decision().is_none()
    ));
    let view = view_of(&rejected);
    assert_eq!(view.local_status, LedgerMemberStatus::Active);
    assert_eq!(
        view.device_update,
        LedgerUpdateView::NeedsAttention(LedgerUpdateProblem::DeviceRelationshipConflict)
    );
    assert!(rejected.outstanding_work(NOW).unwrap().is_empty());
}

// 第三台设备接受移除时，把决定投递给仍认可自己的发起方。
#[test]
fn third_member_delivers_its_acceptance_to_the_proposer() {
    let group = Group::new(&["device-a", "device-b", "device-c"]);
    let local = run_due_work(group.start("device-c"), NOW);
    let (remote, removal) = group.removal(&group.history, "device-a", "device-b", 0x34);
    let received = group.received(local.history(), &remote, "device-c");
    let awaiting = apply(
        local,
        LedgerInput::PeerEvidenceReconciled {
            source: device("device-a"),
            history: Some(received),
            evidence: PeerEvidence::Confirmed,
        },
        NOW,
    )
    .0;
    let history = group.decided(
        awaiting.history(),
        removal.event_id(),
        "device-c",
        RemovalDecision::Accept,
    );

    let accepted = apply(
        awaiting,
        LedgerInput::LocalDecisionSigned {
            history,
            removal_event_id: removal.event_id(),
        },
        NOW,
    )
    .0;

    assert!(accepted.peer(&device("device-b")).is_none());
    let work = accepted.outstanding_work(NOW).unwrap();
    assert!(work.iter().any(|work| matches!(
        &work.work,
        LedgerWork::DeliverDecision { peer, .. } if peer == &device("device-a")
    ) && work.blocks_device_update));
    let settled = run_due_work(accepted, NOW);
    assert!(matches!(
        settled.peer(&device("device-a")),
        Some(PeerLink::Member(link)) if link.outgoing_decision().is_none()
    ));
    assert_eq!(view_of(&settled).device_update, LedgerUpdateView::Completed);
}

// ADR-020：本机仍有待决定的移除时，对端的同步确认不能越过它恢复一致。
#[test]
fn a_sync_confirmation_keeps_the_pending_local_decision() {
    let group = Group::new(&["device-a", "device-b", "device-c"]);
    let local = run_due_work(group.start("device-c"), NOW);
    let (remote, _) = group.removal(&group.history, "device-a", "device-b", 0x35);
    let received = group.received(local.history(), &remote, "device-c");
    let awaiting = apply(
        local,
        LedgerInput::PeerEvidenceReconciled {
            source: device("device-a"),
            history: Some(received),
            evidence: PeerEvidence::Confirmed,
        },
        NOW,
    )
    .0;
    let position = awaiting.history().current_position().unwrap();

    let (confirmed, _, _) = apply(
        awaiting,
        LedgerInput::HistorySyncFinished {
            peer: device("device-a"),
            synced_position: position,
            result: PeerSyncResult::Confirmed,
        },
        NOW,
    );

    assert!(matches!(
        confirmed.peer(&device("device-a")),
        Some(PeerLink::Member(link)) if link.relation() == PeerRelation::AwaitingLocalDecision
    ));
    assert_eq!(
        device_view(&view_of(&confirmed), "device-a").unwrap().sync,
        PeerSyncView::Paused(PeerPauseReason::PendingLocalDecision)
    );
}

// 发起方看到第三台设备停在本机移除之前时，明确显示为等待对方确认并暂停普通内容，不重复同步；
// 对方送来接受决定后恢复一致。
#[test]
fn a_third_member_that_has_not_decided_is_shown_as_awaiting_confirmation() {
    let group = Group::new(&["device-a", "device-b", "device-c"]);
    let membership = run_due_work(group.start("device-a"), NOW);
    let (history, removal) = group.removal(membership.history(), "device-a", "device-b", 0x36);
    let removed = run_effects_only(
        apply(
            membership,
            LedgerInput::LocalRemovalSigned {
                history,
                retained_device_ids: Vec::new(),
            },
            NOW,
        )
        .0,
    );
    let position = removed.history().current_position().unwrap();

    let (awaiting, outcome, _) = apply(
        removed,
        LedgerInput::HistorySyncFinished {
            peer: device("device-c"),
            synced_position: position,
            result: PeerSyncResult::AwaitingPeerDecision,
        },
        NOW,
    );

    assert_eq!(outcome, LedgerOutcome::Applied);
    let view = view_of(&awaiting);
    let peer = device_view(&view, "device-c").unwrap();
    assert_eq!(peer.relation, PeerRelationView::ConfirmationPending);
    assert_eq!(
        peer.sync,
        PeerSyncView::Paused(PeerPauseReason::RelationshipUnconfirmed)
    );
    assert!(!awaiting
        .outstanding_work(NOW)
        .unwrap()
        .iter()
        .any(|work| matches!(
            &work.work,
            LedgerWork::SynchronizeHistory { peer } if peer == &device("device-c")
        )));

    let decided = group.decided(
        &group.received(&group.history, awaiting.history(), "device-c"),
        removal.event_id(),
        "device-c",
        RemovalDecision::Accept,
    );
    let mut merged = awaiting.history().clone();
    merged
        .merge_remote_history(
            &decided,
            group.member("device-a").facts.member_instance,
            &TestVerifier,
        )
        .unwrap();
    let (consistent, _, _) = apply(
        awaiting,
        LedgerInput::PeerEvidenceReconciled {
            source: device("device-c"),
            history: Some(merged),
            evidence: PeerEvidence::Consistent,
        },
        NOW,
    );
    assert!(matches!(
        consistent.peer(&device("device-c")),
        Some(PeerLink::Member(link)) if link.relation() == PeerRelation::Consistent
    ));
}

// 本机历史中没有待该对端决定的本机移除时，不采信"对端停在祖先位置"，按暂时失败重试。
#[test]
fn an_ancestor_confirmation_without_an_undecided_local_removal_is_retried() {
    let group = Group::new(&["device-a", "device-b"]);
    let membership = group.start("device-a");
    let position = membership.history().current_position().unwrap();

    let (deferred, _, _) = apply(
        membership,
        LedgerInput::HistorySyncFinished {
            peer: device("device-b"),
            synced_position: position,
            result: PeerSyncResult::AwaitingPeerDecision,
        },
        NOW,
    );

    assert!(matches!(
        deferred.peer(&device("device-b")),
        Some(PeerLink::Member(link))
            if link.relation() == PeerRelation::Consistent
                && link.sync().last_outcome() == super::PeerSyncOutcome::Deferred
    ));
}

#[test]
fn deferred_history_sync_reports_the_next_retry_and_recovers() {
    let group = Group::new(&["device-a", "device-b"]);
    let membership = group.start("device-a");
    let position = membership.history().current_position().unwrap();

    let (deferred, _, _) = apply(
        membership,
        LedgerInput::HistorySyncFinished {
            peer: device("device-b"),
            synced_position: position.clone(),
            result: PeerSyncResult::Deferred,
        },
        NOW,
    );
    assert_eq!(
        view_of(&deferred).device_update,
        LedgerUpdateView::RetryableFailure {
            next_retry_at_ms: NOW + 1_000
        }
    );
    let work = deferred.outstanding_work(NOW).unwrap();
    assert_eq!(work.len(), 1);
    assert_eq!(work[0].due_at_ms, NOW + 1_000);

    let (confirmed, _, _) = apply(
        deferred,
        LedgerInput::HistorySyncFinished {
            peer: device("device-b"),
            synced_position: position,
            result: PeerSyncResult::Confirmed,
        },
        NOW + 1_000,
    );
    assert_eq!(
        view_of(&confirmed).device_update,
        LedgerUpdateView::Completed
    );
    assert!(confirmed.outstanding_work(NOW).unwrap().is_empty());
}

#[test]
fn a_new_attempt_replaces_a_stable_rejection_until_its_result_arrives() {
    let group = Group::new(&["device-a", "device-b"]);
    let membership = group.start("device-a");
    let position = membership.history().current_position().unwrap();
    let (rejected, _, _) = apply(
        membership,
        LedgerInput::HistorySyncFinished {
            peer: device("device-b"),
            synced_position: position.clone(),
            result: PeerSyncResult::Rejected,
        },
        NOW,
    );
    assert_eq!(
        view_of(&rejected).device_update,
        LedgerUpdateView::NeedsAttention(LedgerUpdateProblem::DeviceStateRejected)
    );

    let (retrying, outcome, _) = apply(
        rejected,
        LedgerInput::HistorySyncSelected {
            peers: vec![device("device-b")],
        },
        NOW,
    );

    // 重新尝试期间不再显示上一轮的拒绝，本轮结论到达前为更新中。
    assert_eq!(outcome, LedgerOutcome::Applied);
    assert_eq!(view_of(&retrying).device_update, LedgerUpdateView::Updating);
    let (confirmed, _, _) = apply(
        retrying,
        LedgerInput::HistorySyncFinished {
            peer: device("device-b"),
            synced_position: position,
            result: PeerSyncResult::Confirmed,
        },
        NOW,
    );
    assert_eq!(
        view_of(&confirmed).device_update,
        LedgerUpdateView::Completed
    );
}

#[test]
fn history_sync_result_for_an_older_position_is_stale() {
    let group = Group::new(&["device-a", "device-b", "device-c"]);
    let membership = group.start("device-a");
    let old_position = membership.history().current_position().unwrap();
    let (history, _) = group.removal(membership.history(), "device-a", "device-c", 0x35);
    let removed = apply(
        membership,
        LedgerInput::LocalRemovalSigned {
            history,
            retained_device_ids: vec![device("device-b")],
        },
        NOW,
    )
    .0;

    let (unchanged, outcome, effects) = apply(
        removed.clone(),
        LedgerInput::HistorySyncFinished {
            peer: device("device-b"),
            synced_position: old_position,
            result: PeerSyncResult::Confirmed,
        },
        NOW,
    );

    assert_eq!(outcome, LedgerOutcome::Stale);
    assert!(effects.is_empty());
    assert_eq!(unchanged, removed);
}

#[test]
fn admission_invalidates_earlier_confirmations() {
    let mut group = Group::new(&["device-a", "device-b"]);
    let membership = run_due_work(group.start("device-a"), NOW);
    let joiner = admission("device-c", 0x5c);
    let event = signed_event(
        membership.history(),
        group.member("device-a"),
        MembershipOperationV2::AddDevice {
            admission: joiner.clone(),
        },
        0x36,
    );
    let mut history = membership.history().clone();
    history
        .verify_and_receive_event(event.clone(), &TestVerifier)
        .unwrap();
    activate(&mut history, &event, &joiner);
    group.members.push(joiner);

    let (admitted, outcome, _) = apply(
        membership.clone(),
        LedgerInput::AdmissionCommitted {
            history: history.clone(),
        },
        NOW,
    );
    assert_eq!(outcome, LedgerOutcome::Applied);
    let view = view_of(&admitted);
    assert_eq!(
        device_view(&view, "device-b").unwrap().relation,
        PeerRelationView::ConfirmationPending
    );
    assert_eq!(
        device_view(&view, "device-c").unwrap().relation,
        PeerRelationView::ConfirmationPending
    );

    let (_, repeated, effects) = apply(admitted, LedgerInput::AdmissionCommitted { history }, NOW);
    assert_eq!(repeated, LedgerOutcome::Unchanged);
    assert!(effects.is_empty());
}

#[test]
fn a_device_that_rejoins_is_presented_by_its_current_member_instance() {
    let mut group = Group::new(&["device-a", "device-b"]);
    let membership = run_due_work(group.start("device-a"), NOW);
    let (history, _) = group.removal(membership.history(), "device-a", "device-b", 0x31);
    let (removed, _, _) = apply(
        membership,
        LedgerInput::LocalRemovalSigned {
            history,
            retained_device_ids: Vec::new(),
        },
        NOW,
    );
    let (departed, _, _) = apply(
        run_effects_only(removed),
        LedgerInput::DeliveryFinished {
            peer: device("device-b"),
            delivery: LedgerDeliveryKind::RemovalNotice,
            result: LedgerDeliveryResult::Delivered,
        },
        NOW,
    );
    let previous = group.member("device-b").facts.member_instance;
    let rejoined = admission("device-b", 0x6b);
    let event = signed_event(
        departed.history(),
        group.member("device-a"),
        MembershipOperationV2::AddDevice {
            admission: rejoined.clone(),
        },
        0x37,
    );
    let mut history = departed.history().clone();
    history
        .verify_and_receive_event(event.clone(), &TestVerifier)
        .unwrap();
    activate(&mut history, &event, &rejoined);
    group.members.push(rejoined.clone());

    let (admitted, outcome, _) = apply(departed, LedgerInput::AdmissionCommitted { history }, NOW);

    assert_eq!(outcome, LedgerOutcome::Applied);
    assert_ne!(previous, rejoined.facts.member_instance);
    let view = view_of(&admitted);
    let presented = device_view(&view, "device-b").unwrap();
    assert_eq!(presented.status, LedgerMemberStatus::Active);
    assert_eq!(presented.member, Some(rejoined.facts.member_instance));
}

// 设备未被移除就重新加入：新实例接替旧实例后，加入方以新实例在目标历史上建立成员记录。
#[test]
fn a_device_that_rejoins_without_removal_starts_with_its_new_instance() {
    let group = Group::new(&["device-a", "device-b"]);
    let rejoined = admission("device-b", 0x6c);
    let event = signed_event(
        &group.history,
        group.member("device-a"),
        MembershipOperationV2::AddDevice {
            admission: rejoined.clone(),
        },
        0x38,
    );
    let mut history = group.history.clone();
    history
        .verify_and_receive_event(event.clone(), &TestVerifier)
        .unwrap();
    activate(&mut history, &event, &rejoined);

    let membership = MembershipLedger::start(
        history,
        rejoined.facts.device_id,
        rejoined.facts.member_instance,
        10,
    )
    .expect("the rejoined device starts from its current instance");

    assert!(membership.peer(&device("device-b")).is_none());
    assert!(matches!(
        membership.peer(&device("device-a")),
        Some(PeerLink::Member(_))
    ));
    let view = view_of(&membership);
    let local = device_view(&view, "device-b").unwrap();
    assert!(local.is_local);
    assert_eq!(local.member, Some(rejoined.facts.member_instance));
}

#[test]
fn history_from_another_lineage_is_rejected() {
    let group = Group::new(&["device-a", "device-b"]);
    let membership = group.start("device-a");
    let other = VersionedMembershipHistory::new("another-lineage".to_owned());

    let error = membership
        .apply(LedgerInput::AdmissionCommitted { history: other }, NOW)
        .unwrap_err();

    assert_eq!(error, LedgerTransitionError::LineageMismatch);
}

#[test]
fn snapshot_round_trips_and_rejects_broken_invariants() {
    let group = Group::new(&["device-a", "device-b", "device-c"]);
    let membership = run_due_work(group.start("device-a"), NOW);
    let (history, _) = group.removal(membership.history(), "device-a", "device-b", 0x37);
    let removed = apply(
        membership,
        LedgerInput::LocalRemovalSigned {
            history,
            retained_device_ids: vec![device("device-c")],
        },
        NOW,
    )
    .0;

    assert_eq!(
        MembershipLedger::restore(removed.snapshot()).unwrap(),
        removed
    );

    let mut missing_member = removed.snapshot();
    missing_member.peers.remove(&device("device-c"));
    assert_eq!(
        MembershipLedger::restore(missing_member).unwrap_err(),
        LedgerTransitionError::InvalidSnapshot
    );

    let mut member_departing = removed.snapshot();
    let departing = member_departing.peers[&device("device-b")].clone();
    member_departing.peers.insert(device("device-c"), departing);
    assert_eq!(
        MembershipLedger::restore(member_departing).unwrap_err(),
        LedgerTransitionError::InvalidSnapshot
    );

    let mut stray = removed.snapshot();
    let PeerLinkSnapshot::Member(member) = stray.peers[&device("device-c")].clone() else {
        panic!("device-c is a member");
    };
    stray
        .peers
        .insert(device("device-z"), PeerLinkSnapshot::Member(member));
    assert_eq!(
        MembershipLedger::restore(stray).unwrap_err(),
        LedgerTransitionError::InvalidSnapshot
    );
}

#[test]
fn normalized_restore_repairs_legacy_shapes_that_plain_restore_rejects() {
    let group = Group::new(&["device-a", "device-b", "device-c"]);
    let membership = run_due_work(group.start("device-a"), NOW);
    let (history, _) = group.removal(membership.history(), "device-a", "device-b", 0x38);
    let removed = apply(
        membership,
        LedgerInput::LocalRemovalSigned {
            history,
            retained_device_ids: vec![device("device-c")],
        },
        NOW,
    )
    .0;
    let departing = removed.snapshot().peers[&device("device-b")].clone();
    let PeerLinkSnapshot::Member(member) = removed.snapshot().peers[&device("device-c")].clone()
    else {
        panic!("device-c is a member");
    };

    let mut legacy = removed.snapshot();
    // 有效成员只剩离开记录、已不在历史中的设备仍是成员、本机出现在对端表中。
    legacy.peers.insert(device("device-c"), departing.clone());
    legacy
        .peers
        .insert(device("device-z"), PeerLinkSnapshot::Member(member.clone()));
    legacy
        .peers
        .insert(device("device-a"), PeerLinkSnapshot::Member(member));
    assert_eq!(
        MembershipLedger::restore(legacy.clone()).unwrap_err(),
        LedgerTransitionError::InvalidSnapshot
    );

    let normalized = MembershipLedger::restore_normalized(legacy).unwrap();
    assert_eq!(normalized.revision(), removed.revision());
    assert_eq!(
        normalized.peer(&device("device-b")),
        removed.peer(&device("device-b"))
    );
    assert!(matches!(
        normalized.peer(&device("device-c")),
        Some(PeerLink::Member(link)) if link.relation() == PeerRelation::Unconfirmed
    ));
    assert!(normalized.peer(&device("device-z")).is_none());
    assert!(normalized.peer(&device("device-a")).is_none());
    assert_eq!(
        MembershipLedger::restore(normalized.snapshot()).unwrap(),
        normalized
    );
}

/// 可复现的伪随机序列，不依赖外部随机源。
struct Seeded(u64);

impl Seeded {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn pick(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }
}

/// 阻塞设备更新的待办非空，当且仅当成员部分为更新中或可重试失败；需要处理状态除外。
fn assert_work_matches_presentation(membership: &MembershipLedger, now_ms: i64, seed: u64) {
    let blocking = membership
        .outstanding_work(now_ms)
        .unwrap()
        .iter()
        .any(|work| work.blocks_device_update);
    match view_of(membership).device_update {
        LedgerUpdateView::NeedsAttention(_) => {}
        LedgerUpdateView::Updating | LedgerUpdateView::RetryableFailure { .. } => {
            assert!(
                blocking,
                "seed {seed}: update shown without outstanding work"
            )
        }
        LedgerUpdateView::Completed => {
            assert!(
                !blocking,
                "seed {seed}: outstanding work hidden behind completion"
            )
        }
    }
}

#[test]
fn seeded_interleavings_keep_invariants_and_converge() {
    for seed in 1..=24u64 {
        let group = Group::new(&["device-a", "device-b", "device-c", "device-d"]);
        let mut membership = group.start("device-a");
        let mut random = Seeded(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
        let mut now_ms = NOW;
        let mut marker = 0x60u8;
        for _ in 0..160 {
            let input = match random.pick(5) {
                0 => {
                    let peers: Vec<DeviceId> = membership
                        .peers()
                        .filter(|(_, link)| matches!(link, PeerLink::Member(_)))
                        .map(|(device, _)| *device)
                        .collect();
                    if peers.is_empty() {
                        continue;
                    }
                    let target = peers[random.pick(peers.len())];
                    marker = marker.wrapping_add(1);
                    let (history, _) =
                        group.removal(membership.history(), "device-a", target.as_str(), marker);
                    LedgerInput::LocalRemovalSigned {
                        history,
                        retained_device_ids: Vec::new(),
                    }
                }
                1 => {
                    now_ms += (random.pick(4) as i64) * 100_000;
                    continue;
                }
                2 => {
                    let peers: Vec<DeviceId> = membership
                        .peers()
                        .filter(|(_, link)| matches!(link, PeerLink::Member(_)))
                        .map(|(device, _)| *device)
                        .collect();
                    if peers.is_empty() {
                        continue;
                    }
                    let evidence = [
                        PeerEvidence::Confirmed,
                        PeerEvidence::Invalid,
                        PeerEvidence::NeedsEvidence,
                    ][random.pick(3)]
                    .clone();
                    LedgerInput::PeerEvidenceReconciled {
                        source: peers[random.pick(peers.len())],
                        history: None,
                        evidence,
                    }
                }
                _ => {
                    let due: Vec<_> = membership
                        .outstanding_work(now_ms)
                        .unwrap()
                        .into_iter()
                        .filter(|work| work.due_at_ms <= now_ms)
                        .collect();
                    if due.is_empty() {
                        continue;
                    }
                    let work = due[random.pick(due.len())].work.clone();
                    random_result(&membership, work, &mut random)
                }
            };
            membership = membership.apply(input, now_ms).unwrap().into_parts().0;
            membership.validate().unwrap();
            assert_work_matches_presentation(&membership, now_ms, seed);
        }

        // 收敛：之后所有投递与同步都成功，并跨过离开窗口。
        now_ms += DEPARTURE_WINDOW_MS;
        membership = run_due_work(membership, now_ms);
        assert!(
            membership
                .peers()
                .all(|(_, link)| matches!(link, PeerLink::Member(_))),
            "seed {seed}: departures must end"
        );
        assert!(
            !matches!(
                view_of(&membership).device_update,
                LedgerUpdateView::Updating | LedgerUpdateView::RetryableFailure { .. }
            ),
            "seed {seed}: device update must settle"
        );
        assert_work_matches_presentation(&membership, now_ms, seed);
    }
}

fn random_result(
    membership: &MembershipLedger,
    work: LedgerWork,
    random: &mut Seeded,
) -> LedgerInput {
    let delivery = [
        LedgerDeliveryResult::Delivered,
        LedgerDeliveryResult::Deferred,
        LedgerDeliveryResult::Rejected,
    ][random.pick(3)];
    match work {
        LedgerWork::DeliverRemovalNotice { peer, .. } => LedgerInput::DeliveryFinished {
            peer,
            delivery: LedgerDeliveryKind::RemovalNotice,
            result: delivery,
        },
        LedgerWork::DeliverDecision { peer, .. } => LedgerInput::DeliveryFinished {
            peer,
            delivery: LedgerDeliveryKind::Decision,
            result: delivery,
        },
        LedgerWork::SynchronizeHistory { peer } => LedgerInput::HistorySyncFinished {
            peer,
            synced_position: membership.history().current_position().unwrap(),
            result: [
                PeerSyncResult::Confirmed,
                PeerSyncResult::Deferred,
                PeerSyncResult::Deferred,
                PeerSyncResult::Invalid,
            ][random.pick(4)],
        },
        other => success_input(membership, other),
    }
}

#[test]
fn effect_steps_advance_in_order_and_ignore_stale_reports() {
    let group = Group::new(&["device-a", "device-b"]);
    let membership = run_due_work(group.start("device-a"), NOW);
    let (history, event) = group.removal(membership.history(), "device-a", "device-b", 0x38);
    let removed = apply(
        membership,
        LedgerInput::LocalRemovalSigned {
            history,
            retained_device_ids: Vec::new(),
        },
        NOW,
    )
    .0;

    let (_, stale, _) = apply(
        removed.clone(),
        LedgerInput::EffectStepFinished {
            event_id: event.event_id(),
            from: MemberEffectPhase::SecurityApplied,
        },
        NOW,
    );
    assert_eq!(stale, LedgerOutcome::Stale);

    let mut current = removed;
    for phase in [
        MemberEffectPhase::Prepared,
        MemberEffectPhase::MemberFactsApplied,
        MemberEffectPhase::SecurityApplied,
    ] {
        let (next, outcome, _) = apply(
            current,
            LedgerInput::EffectStepFinished {
                event_id: event.event_id(),
                from: phase,
            },
            NOW,
        );
        assert_eq!(outcome, LedgerOutcome::Applied);
        current = next;
    }
    assert_eq!(current.unfinished_effects().count(), 0);
}

#[test]
fn adopted_remote_admission_registers_an_effect_for_the_new_member() {
    let mut group = Group::new(&["device-a", "device-b", "device-c"]);
    let local = run_due_work(group.start("device-c"), NOW);
    let joiner = admission("device-d", 0x5d);
    let event = signed_event(
        &group.history,
        group.member("device-a"),
        MembershipOperationV2::AddDevice {
            admission: joiner.clone(),
        },
        0x39,
    );
    let mut remote = group.history.clone();
    remote
        .verify_and_receive_event(event.clone(), &TestVerifier)
        .unwrap();
    activate(&mut remote, &event, &joiner);
    group.members.push(joiner);
    let received = group.received(local.history(), &remote, "device-c");

    let (adopted, outcome, _) = apply(
        local,
        LedgerInput::PeerEvidenceReconciled {
            source: device("device-a"),
            history: Some(received),
            evidence: PeerEvidence::Confirmed,
        },
        NOW,
    );

    assert_eq!(outcome, LedgerOutcome::Applied);
    let effect = adopted.unfinished_effects().next().unwrap();
    assert_eq!(effect.event_id(), event.event_id());
    assert_eq!(effect.kind(), super::MemberEffectKind::AddDevice);
    assert_eq!(effect.affected_device_ids(), &[device("device-d")]);
    assert!(matches!(
        adopted.peer(&device("device-d")),
        Some(PeerLink::Member(link)) if link.relation() == PeerRelation::Unconfirmed
    ));
    assert_eq!(
        device_view(&view_of(&adopted), "device-d").unwrap().sync,
        PeerSyncView::Paused(PeerPauseReason::EffectPending)
    );
    let settled = run_due_work(adopted, NOW);
    assert_eq!(view_of(&settled).device_update, LedgerUpdateView::Completed);
}

#[test]
fn recovered_branch_rebuilds_peers_and_drops_old_branch_work() {
    let group = Group::new(&["device-a", "device-b", "device-c"]);
    let membership = run_due_work(group.start("device-a"), NOW);
    let (history, _) = group.removal(membership.history(), "device-a", "device-b", 0x3a);
    let removed = apply(
        membership,
        LedgerInput::LocalRemovalSigned {
            history,
            retained_device_ids: vec![device("device-c")],
        },
        NOW,
    )
    .0;

    let (recovered, outcome, _) = apply(
        removed,
        LedgerInput::BranchRecovered {
            history: group.history.clone(),
        },
        NOW,
    );

    assert_eq!(outcome, LedgerOutcome::Applied);
    assert_eq!(recovered.unfinished_effects().count(), 0);
    for peer in ["device-b", "device-c"] {
        assert!(matches!(
            recovered.peer(&device(peer)),
            Some(PeerLink::Member(link)) if link.relation() == PeerRelation::Consistent
        ));
    }
    recovered.validate().unwrap();
}

// 已被本机移除的设备送来的决定只贡献已验证历史，不恢复它的成员关系。
#[test]
fn evidence_from_a_departing_device_only_contributes_history() {
    let group = Group::new(&["device-a", "device-b", "device-c"]);
    let proposer = run_due_work(group.start("device-a"), NOW);
    let (removed_history, removal) =
        group.removal(proposer.history(), "device-a", "device-b", 0x51);
    let (proposer, _, _) = apply(
        proposer,
        LedgerInput::LocalRemovalSigned {
            history: removed_history.clone(),
            retained_device_ids: vec![device("device-c")],
        },
        NOW,
    );
    let target_history = group.received(&group.history, &removed_history, "device-b");
    let decided = group.decided(
        &target_history,
        removal.event_id(),
        "device-b",
        RemovalDecision::Accept,
    );
    let merged = group.received(proposer.history(), &decided, "device-a");
    assert_ne!(&merged, proposer.history());
    let revision = proposer.revision();

    let (adopted, outcome, _) = apply(
        proposer,
        LedgerInput::PeerEvidenceReconciled {
            source: device("device-b"),
            history: Some(merged.clone()),
            evidence: PeerEvidence::Consistent,
        },
        NOW,
    );

    assert_eq!(outcome, LedgerOutcome::Applied);
    assert_eq!(adopted.revision(), revision + 1);
    assert_eq!(adopted.history(), &merged);
    assert!(matches!(
        adopted.peer(&device("device-b")),
        Some(PeerLink::Departing(_))
    ));

    let (repeated, outcome, effects) = apply(
        adopted,
        LedgerInput::PeerEvidenceReconciled {
            source: device("device-b"),
            history: Some(merged),
            evidence: PeerEvidence::Consistent,
        },
        NOW,
    );
    assert_eq!(outcome, LedgerOutcome::Unchanged);
    assert!(effects.is_empty());
    assert_eq!(repeated.revision(), revision + 1);
}

// 一致但未确认位置的证据只修复关系，不伪造也不丢弃已认证的确认位置。
#[test]
fn consistent_evidence_keeps_the_confirmed_position() {
    let group = Group::new(&["device-a", "device-b"]);
    let membership = run_due_work(group.start("device-a"), NOW);
    let confirmed = membership.history().current_position().unwrap();
    let (invalid, outcome, _) = apply(
        membership,
        LedgerInput::PeerEvidenceReconciled {
            source: device("device-b"),
            history: None,
            evidence: PeerEvidence::Invalid,
        },
        NOW,
    );
    assert_eq!(outcome, LedgerOutcome::Applied);

    let (repaired, outcome, _) = apply(
        invalid,
        LedgerInput::PeerEvidenceReconciled {
            source: device("device-b"),
            history: None,
            evidence: PeerEvidence::Consistent,
        },
        NOW,
    );

    assert_eq!(outcome, LedgerOutcome::Applied);
    let Some(PeerLink::Member(link)) = repaired.peer(&device("device-b")) else {
        panic!("device-b stays a member");
    };
    assert_eq!(link.relation(), PeerRelation::Consistent);
    assert_eq!(link.confirmed_position(), Some(&confirmed));
    assert_eq!(
        view_of(&repaired).device_update,
        LedgerUpdateView::Completed
    );
}

#[test]
fn companion_data_changes_only_advance_the_revision() {
    let group = Group::new(&["device-a", "device-b"]);
    let membership = run_due_work(group.start("device-a"), NOW);
    let revision = membership.revision();

    let (quiet, outcome, effects) = apply(
        membership.clone(),
        LedgerInput::CompanionDataChanged {
            presentation_changed: false,
        },
        NOW,
    );
    assert_eq!(outcome, LedgerOutcome::Applied);
    assert!(effects.is_empty());
    assert_eq!(quiet.revision(), revision + 1);
    assert_eq!(quiet.snapshot().peers, membership.snapshot().peers);

    let (_, _, effects) = apply(
        membership,
        LedgerInput::CompanionDataChanged {
            presentation_changed: true,
        },
        NOW,
    );
    assert_eq!(
        effects,
        vec![LedgerEffect::AfterCommit(
            LedgerFollowUp::PublishDeviceTrustChange
        )]
    );
}

// 从激活基线开始的历史不保存基线之前的事件；采用对端追加的加入时回溯到基线即停止。
#[test]
fn adopting_history_on_top_of_an_activation_baseline_registers_the_new_member() {
    use crate::membership::MembershipActivationBaselineV2;

    let members = [admission("device-a", 0x41), admission("device-b", 0x42)];
    let baseline = VersionedMembershipHistory::from_activation_baseline(
        MembershipActivationBaselineV2::Established {
            lineage_id: LINEAGE.to_owned(),
            head_event_id: MembershipEventId::from_bytes([0x11; 32]),
            head_depth: 0,
            current_members: members
                .iter()
                .map(|member| (member.facts.clone(), member.membership_credential.clone()))
                .collect(),
        },
    )
    .unwrap();
    let membership = run_due_work(
        MembershipLedger::start(
            baseline.clone(),
            device("device-a"),
            members[0].facts.member_instance,
            10,
        )
        .unwrap(),
        NOW,
    );
    let joined = admission("device-c", 0x43);
    let mut remote = baseline;
    remote
        .verify_and_receive_event(
            signed_event(
                &remote,
                &members[1],
                MembershipOperationV2::AddDevice {
                    admission: joined.clone(),
                },
                0x61,
            ),
            &TestVerifier,
        )
        .unwrap();

    let (adopted, outcome, _) = apply(
        membership,
        LedgerInput::PeerEvidenceReconciled {
            source: device("device-b"),
            history: Some(remote),
            evidence: PeerEvidence::Confirmed,
        },
        NOW,
    );

    assert_eq!(outcome, LedgerOutcome::Applied);
    assert!(adopted
        .unfinished_effects()
        .any(|effect| effect.affected_device_ids() == [device("device-c")]));
    assert!(matches!(
        adopted.peer(&device("device-c")),
        Some(PeerLink::Member(_))
    ));
}
