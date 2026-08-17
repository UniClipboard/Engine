use uc_core::ids::DeviceId;
use uc_core::membership::{
    AdmissionChangeFacts, MemberInstanceId, MembershipDecision, MembershipEvent, MembershipEventId,
    MembershipOperation, MembershipReconciliation, MembershipReconciliationOutcome,
    RemovalDecision,
};

const LINEAGE: &str = "space-lineage";

fn member(byte: u8) -> MemberInstanceId {
    MemberInstanceId::from_bytes([byte; 32])
}

fn add_operation(member: MemberInstanceId) -> MembershipOperation {
    MembershipOperation::AddDevice {
        admission: AdmissionChangeFacts {
            member_instance: member,
            device_id: DeviceId::new(format!("device-{:02x}", member.as_bytes()[0])),
            device_name: "device".to_owned(),
            identity_fingerprint: uc_core::security::IdentityFingerprint::from_display_string(
                "ABCD-EFGH-IJKL-MNOP",
            )
            .unwrap(),
            transport_public_key: vec![1],
            transport_address_blob: vec![2],
            identity_signature: vec![3],
        },
    }
}

fn event(
    parent: Option<MembershipEventId>,
    parent_depth: u64,
    author: MemberInstanceId,
    operation: MembershipOperation,
    operation_byte: u8,
) -> MembershipEvent {
    MembershipEvent::new(
        LINEAGE.to_owned(),
        parent,
        parent_depth,
        [operation_byte; 16],
        author,
        operation,
        [operation_byte; 32],
        [operation_byte.saturating_add(1); 32],
        Vec::new(),
        None,
        vec![operation_byte],
    )
}

// 流程：B 收到尚未亲自确认的移除；已知历史前进，但已应用历史停在移除之前等待用户。
#[test]
fn unseen_removal_waits_for_the_local_user_before_advancing_the_applied_head() {
    let a = member(1);
    let b = member(2);
    let mut reconciliation = MembershipReconciliation::new(LINEAGE.to_owned(), b);

    let genesis = event(None, 0, a, add_operation(a), 1);
    assert_eq!(
        reconciliation.receive_verified(genesis.clone()),
        Ok(MembershipReconciliationOutcome::UpdatesApplied)
    );

    let addition = event(Some(genesis.event_id()), 1, a, add_operation(b), 2);
    assert_eq!(
        reconciliation.receive_verified(addition.clone()),
        Ok(MembershipReconciliationOutcome::UpdatesApplied)
    );

    let removal = event(
        Some(addition.event_id()),
        2,
        a,
        MembershipOperation::RemoveDevice { member: b },
        3,
    );
    assert_eq!(
        reconciliation.receive_verified(removal.clone()),
        Ok(MembershipReconciliationOutcome::RemovalDecisionRequired {
            removal_event_id: removal.event_id(),
        })
    );
    assert_eq!(reconciliation.known_head(), Some(removal.event_id()));
    assert_eq!(reconciliation.applied_head(), Some(addition.event_id()));
    assert_eq!(
        reconciliation.pending_removal_decision(),
        Some(removal.event_id())
    );
    assert_eq!(reconciliation.effective_members(), [a, b].into());

    assert_eq!(
        reconciliation.record_decision(MembershipDecision::new(
            LINEAGE.to_owned(),
            removal.event_id(),
            b,
            RemovalDecision::Accept,
            Some(addition.event_id()),
            [3; 32],
            [4; 16],
            vec![4],
        )),
        Ok(MembershipReconciliationOutcome::RemovalAccepted {
            removal_event_id: removal.event_id(),
        })
    );
    assert_eq!(reconciliation.applied_head(), Some(removal.event_id()));
    assert_eq!(reconciliation.effective_members(), [a].into());
}

// 流程：C 收到由 B 转发的 A 移除 B 事件；产品事实必须仍把 A 识别为原始发起设备，并精确返回 B。
#[test]
fn pending_removal_facts_resolve_the_signed_author_and_exact_target_from_prior_history() {
    let a = member(1);
    let b = member(2);
    let c = member(3);
    let mut reconciliation = MembershipReconciliation::new(LINEAGE.to_owned(), c);
    let genesis = event(None, 0, a, add_operation(a), 1);
    let b_addition = event(Some(genesis.event_id()), 1, a, add_operation(b), 2);
    let c_addition = event(Some(b_addition.event_id()), 2, b, add_operation(c), 3);
    let removal = event(
        Some(c_addition.event_id()),
        3,
        a,
        MembershipOperation::RemoveDevice { member: b },
        4,
    );

    assert!(reconciliation.receive_verified(genesis).is_ok());
    assert!(reconciliation.receive_verified(b_addition).is_ok());
    assert!(reconciliation.receive_verified(c_addition).is_ok());
    assert!(reconciliation.receive_verified(removal.clone()).is_ok());

    let facts = reconciliation
        .pending_removal_facts()
        .expect("pending removal exposes verified product facts");
    assert_eq!(facts.removal_event_id, removal.event_id());
    assert_eq!(facts.proposed_by_device_id, DeviceId::new("device-01"));
    assert_eq!(facts.target_device_ids, vec![DeviceId::new("device-02")]);
    assert!(!facts.includes_member(c));
    assert!(facts.includes_member(b));
}

// 流程：本机已经拒绝一项移除后再次收到相同产品提交；加密历史仍能回答原决定，不需要第二份队列。
#[test]
fn completed_local_removal_decision_remains_queryable_from_membership_history() {
    let a = member(1);
    let b = member(2);
    let mut reconciliation = MembershipReconciliation::new(LINEAGE.to_owned(), b);
    let genesis = event(None, 0, a, add_operation(a), 1);
    let addition = event(Some(genesis.event_id()), 1, a, add_operation(b), 2);
    let removal = event(
        Some(addition.event_id()),
        2,
        a,
        MembershipOperation::RemoveDevice { member: b },
        3,
    );
    assert!(reconciliation.receive_verified(genesis).is_ok());
    assert!(reconciliation.receive_verified(addition.clone()).is_ok());
    assert!(reconciliation.receive_verified(removal.clone()).is_ok());
    assert!(reconciliation
        .record_decision(MembershipDecision::new(
            LINEAGE.to_owned(),
            removal.event_id(),
            b,
            RemovalDecision::Reject,
            Some(addition.event_id()),
            [2; 32],
            [4; 16],
            vec![4],
        ))
        .is_ok());

    assert_eq!(
        reconciliation.local_removal_decision(removal.event_id()),
        Some(RemovalDecision::Reject)
    );
}

// 流程：A 已经应用对 B 的移除；B 不再是有效成员，但产品仍需从加密历史显示其可信名称和已移除状态。
#[test]
fn admitted_device_facts_remain_available_after_the_device_is_removed() {
    let a = member(1);
    let b = member(2);
    let mut reconciliation = MembershipReconciliation::new(LINEAGE.to_owned(), a);
    let genesis = event(None, 0, a, add_operation(a), 1);
    let addition = event(Some(genesis.event_id()), 1, a, add_operation(b), 2);
    let removal = event(
        Some(addition.event_id()),
        2,
        a,
        MembershipOperation::RemoveDevice { member: b },
        3,
    );
    assert!(reconciliation.receive_verified(genesis).is_ok());
    assert!(reconciliation.receive_verified(addition).is_ok());
    assert!(reconciliation.receive_verified(removal).is_ok());

    let devices = reconciliation.admitted_device_facts();
    assert_eq!(devices.len(), 2);
    assert_eq!(devices[1].device_id, DeviceId::new("device-02"));
    assert_eq!(devices[1].device_name, "device");
    assert!(!reconciliation.is_device_effective(&DeviceId::new("device-02")));
}

// 流程：A 移除 B；B 接受后向 A 回传，A 保存 B 的决定但不改变 A 已经应用的分支。
#[test]
fn removal_author_records_a_verified_acceptance_from_the_removed_member() {
    let a = member(1);
    let b = member(2);
    let mut reconciliation = MembershipReconciliation::new(LINEAGE.to_owned(), a);
    let genesis = event(None, 0, a, add_operation(a), 1);
    let addition = event(Some(genesis.event_id()), 1, a, add_operation(b), 2);
    let removal = event(
        Some(addition.event_id()),
        2,
        a,
        MembershipOperation::RemoveDevice { member: b },
        3,
    );

    assert!(reconciliation.receive_verified(genesis).is_ok());
    assert!(reconciliation.receive_verified(addition.clone()).is_ok());
    assert_eq!(
        reconciliation.receive_verified(removal.clone()),
        Ok(MembershipReconciliationOutcome::UpdatesApplied)
    );

    let decision = MembershipDecision::new(
        LINEAGE.to_owned(),
        removal.event_id(),
        b,
        RemovalDecision::Accept,
        Some(addition.event_id()),
        [3; 32],
        [4; 16],
        vec![4],
    );
    assert_eq!(
        reconciliation.record_peer_decision(decision.clone()),
        Ok(MembershipReconciliationOutcome::RemovalAccepted {
            removal_event_id: removal.event_id(),
        })
    );
    assert_eq!(
        reconciliation.decision_for(removal.event_id(), b),
        Some(&decision)
    );
}

// 流程：A 发起对 B 的移除；A 不能再伪装为对端，为同一移除补交一份决定。
#[test]
fn removal_author_cannot_submit_a_redundant_peer_decision() {
    let a = member(1);
    let b = member(2);
    let c = member(3);
    let mut reconciliation = MembershipReconciliation::new(LINEAGE.to_owned(), c);
    let genesis = event(None, 0, a, add_operation(a), 1);
    let b_addition = event(Some(genesis.event_id()), 1, a, add_operation(b), 2);
    let c_addition = event(Some(b_addition.event_id()), 2, a, add_operation(c), 3);
    let removal = event(
        Some(c_addition.event_id()),
        3,
        a,
        MembershipOperation::RemoveDevice { member: b },
        4,
    );

    assert!(reconciliation.receive_verified(genesis).is_ok());
    assert!(reconciliation.receive_verified(b_addition).is_ok());
    assert!(reconciliation.receive_verified(c_addition.clone()).is_ok());
    assert!(reconciliation.receive_verified(removal.clone()).is_ok());

    assert_eq!(
        reconciliation.record_peer_decision(MembershipDecision::new(
            LINEAGE.to_owned(),
            removal.event_id(),
            a,
            RemovalDecision::Accept,
            Some(c_addition.event_id()),
            removal.resulting_members_digest,
            [5; 16],
            vec![5],
        )),
        Err(uc_core::membership::MembershipHistoryError::DecisionFromAnotherMember)
    );
}

// 流程：B 收到移除及后续历史，随后又重复收到同一移除；原待决定项保持唯一，后续事件不能越过它。
#[test]
fn later_history_and_duplicate_delivery_keep_the_original_removal_pending() {
    let a = member(1);
    let b = member(2);
    let c = member(3);
    let mut reconciliation = MembershipReconciliation::new(LINEAGE.to_owned(), b);

    let genesis = event(None, 0, a, add_operation(a), 1);
    let addition = event(Some(genesis.event_id()), 1, a, add_operation(b), 2);
    let removal = event(
        Some(addition.event_id()),
        2,
        a,
        MembershipOperation::RemoveDevice { member: b },
        3,
    );
    let later_addition = event(Some(removal.event_id()), 3, a, add_operation(c), 4);

    assert!(reconciliation.receive_verified(genesis).is_ok());
    assert!(reconciliation.receive_verified(addition.clone()).is_ok());
    assert_eq!(
        reconciliation.receive_verified(removal.clone()),
        Ok(MembershipReconciliationOutcome::RemovalDecisionRequired {
            removal_event_id: removal.event_id(),
        })
    );
    assert_eq!(
        reconciliation.receive_verified(later_addition),
        Ok(MembershipReconciliationOutcome::RemovalDecisionRequired {
            removal_event_id: removal.event_id(),
        })
    );
    assert_eq!(
        reconciliation.receive_verified(removal.clone()),
        Ok(MembershipReconciliationOutcome::RemovalDecisionRequired {
            removal_event_id: removal.event_id(),
        })
    );
    assert_eq!(reconciliation.applied_head(), Some(addition.event_id()));
}

// 流程：B 接受 A 的移除，但决定携带的结果摘要与原事件不一致；该决定必须被拒绝。
#[test]
fn accepting_a_removal_requires_the_received_result_digest() {
    let a = member(1);
    let b = member(2);
    let mut reconciliation = MembershipReconciliation::new(LINEAGE.to_owned(), b);

    let genesis = event(None, 0, a, add_operation(a), 1);
    let addition = event(Some(genesis.event_id()), 1, a, add_operation(b), 2);
    let removal = event(
        Some(addition.event_id()),
        2,
        a,
        MembershipOperation::RemoveDevice { member: b },
        3,
    );

    assert!(reconciliation.receive_verified(genesis).is_ok());
    assert!(reconciliation.receive_verified(addition.clone()).is_ok());
    assert!(reconciliation.receive_verified(removal.clone()).is_ok());

    let result = reconciliation.record_decision(MembershipDecision::new(
        LINEAGE.to_owned(),
        removal.event_id(),
        b,
        RemovalDecision::Accept,
        Some(addition.event_id()),
        [99; 32],
        [4; 16],
        vec![4],
    ));

    assert!(result.is_err());
    assert_eq!(reconciliation.applied_head(), Some(addition.event_id()));
}

// 流程：同一个操作标识被用于创建第二项成员变化；历史拒绝这次重放。
#[test]
fn reused_operation_id_cannot_create_a_second_history_event() {
    let a = member(1);
    let b = member(2);
    let c = member(3);
    let mut reconciliation = MembershipReconciliation::new(LINEAGE.to_owned(), a);

    let genesis = event(None, 0, a, add_operation(a), 1);
    let first = event(Some(genesis.event_id()), 1, a, add_operation(b), 2);
    let replayed_operation = event(Some(first.event_id()), 2, a, add_operation(c), 2);

    assert!(reconciliation.receive_verified(genesis).is_ok());
    assert!(reconciliation.receive_verified(first).is_ok());
    assert!(reconciliation.receive_verified(replayed_operation).is_err());
    assert_eq!(reconciliation.effective_members(), [a, b].into());
}

// 流程：双方提交共享父事件的不同后继；本机保存可验证事实，并只把双方关系判为分叉。
#[test]
fn incomparable_verified_history_is_preserved_and_marks_only_the_peer_relationship_diverged() {
    let a = member(1);
    let b = member(2);
    let c = member(3);
    let mut reconciliation = MembershipReconciliation::new(LINEAGE.to_owned(), a);

    let genesis = event(None, 0, a, add_operation(a), 1);
    let local_successor = event(Some(genesis.event_id()), 1, a, add_operation(b), 2);
    let peer_successor = event(Some(genesis.event_id()), 1, a, add_operation(c), 3);

    assert!(reconciliation.receive_verified(genesis).is_ok());
    assert!(reconciliation
        .receive_verified(local_successor.clone())
        .is_ok());
    assert_eq!(
        reconciliation.receive_verified(peer_successor.clone()),
        Ok(MembershipReconciliationOutcome::Diverged)
    );
    assert_eq!(
        reconciliation.known_head(),
        Some(local_successor.event_id())
    );
    assert_eq!(reconciliation.effective_members(), [a, b].into());
    assert_eq!(
        reconciliation.receive_verified(peer_successor),
        Ok(MembershipReconciliationOutcome::Diverged)
    );
}

// 流程：B 拒绝 A 的移除，但决定携带的保留分支摘要不等于本机当前结果；该决定必须被拒绝。
#[test]
fn rejecting_a_removal_requires_the_current_local_result_digest() {
    let a = member(1);
    let b = member(2);
    let mut reconciliation = MembershipReconciliation::new(LINEAGE.to_owned(), b);

    let genesis = event(None, 0, a, add_operation(a), 1);
    let addition = event(Some(genesis.event_id()), 1, a, add_operation(b), 2);
    let removal = event(
        Some(addition.event_id()),
        2,
        a,
        MembershipOperation::RemoveDevice { member: b },
        3,
    );

    assert!(reconciliation.receive_verified(genesis).is_ok());
    assert!(reconciliation.receive_verified(addition.clone()).is_ok());
    assert!(reconciliation.receive_verified(removal.clone()).is_ok());

    let result = reconciliation.record_decision(MembershipDecision::new(
        LINEAGE.to_owned(),
        removal.event_id(),
        b,
        RemovalDecision::Reject,
        Some(addition.event_id()),
        [99; 32],
        [4; 16],
        vec![4],
    ));

    assert!(result.is_err());
    assert_eq!(reconciliation.known_head(), Some(removal.event_id()));
    assert_eq!(reconciliation.applied_head(), Some(addition.event_id()));
    assert_eq!(reconciliation.effective_members(), [a, b].into());
}

// 流程：B 拒绝 A 的远端移除后继续自己的分支，并能在原已应用历史上提交后续成员变化。
#[test]
fn rejecting_a_remote_removal_keeps_the_local_branch_open_for_later_changes() {
    let a = member(1);
    let b = member(2);
    let c = member(3);
    let mut reconciliation = MembershipReconciliation::new(LINEAGE.to_owned(), b);
    let genesis = event(None, 0, a, add_operation(a), 1);
    let addition = event(Some(genesis.event_id()), 1, a, add_operation(b), 2);
    let removal = event(
        Some(addition.event_id()),
        2,
        a,
        MembershipOperation::RemoveDevice { member: b },
        3,
    );

    assert!(reconciliation.receive_verified(genesis).is_ok());
    assert!(reconciliation.receive_verified(addition.clone()).is_ok());
    assert!(reconciliation.receive_verified(removal.clone()).is_ok());
    assert_eq!(
        reconciliation.record_decision(MembershipDecision::new(
            LINEAGE.to_owned(),
            removal.event_id(),
            b,
            RemovalDecision::Reject,
            Some(addition.event_id()),
            [2; 32],
            [4; 16],
            vec![4],
        )),
        Ok(MembershipReconciliationOutcome::RemovalRejected {
            removal_event_id: removal.event_id(),
        })
    );

    let (parent, depth) = reconciliation.next_event_position();
    let local_addition = event(Some(parent.unwrap()), depth, b, add_operation(c), 5);
    assert_eq!(
        reconciliation.receive_verified(local_addition),
        Ok(MembershipReconciliationOutcome::UpdatesApplied)
    );
    assert_eq!(reconciliation.effective_members(), [a, b, c].into());
}

// 流程：从已应用历史恢复当前成员集合；每个有效成员都必须能还原到原设备。
#[test]
fn applied_history_recovers_the_device_for_each_effective_member() {
    let a = member(1);
    let b = member(2);
    let mut reconciliation = MembershipReconciliation::new(LINEAGE.to_owned(), a);
    let genesis = event(None, 0, a, add_operation(a), 1);
    let addition = event(Some(genesis.event_id()), 1, a, add_operation(b), 2);

    assert!(reconciliation.receive_verified(genesis).is_ok());
    assert!(reconciliation.receive_verified(addition).is_ok());

    assert_eq!(
        reconciliation.device_for_member(&a),
        Some(DeviceId::new("device-01"))
    );
    assert_eq!(
        reconciliation.device_for_member(&b),
        Some(DeviceId::new("device-02"))
    );
}

// 流程：对端从指定父事件之后请求一页历史；返回必须连续且不重复父事件。
#[test]
fn history_page_starts_after_the_requested_parent_and_stays_continuous() {
    let a = member(1);
    let b = member(2);
    let c = member(3);
    let mut reconciliation = MembershipReconciliation::new(LINEAGE.to_owned(), a);
    let genesis = event(None, 0, a, add_operation(a), 1);
    let addition = event(Some(genesis.event_id()), 1, a, add_operation(b), 2);
    let later_addition = event(Some(addition.event_id()), 2, a, add_operation(c), 3);

    assert!(reconciliation.receive_verified(genesis.clone()).is_ok());
    assert!(reconciliation.receive_verified(addition.clone()).is_ok());
    assert!(reconciliation
        .receive_verified(later_addition.clone())
        .is_ok());

    assert_eq!(
        reconciliation.events_after(Some(genesis.event_id()), 2),
        vec![addition, later_addition]
    );
}

// 流程：成员事件标识输出为稳定外部格式后再读回；前后必须完全一致。
#[test]
fn membership_event_id_has_a_round_trippable_stable_external_form() {
    let a = member(1);
    let event = event(None, 0, a, add_operation(a), 1);
    let event_id = event.event_id();

    assert_eq!(
        MembershipEventId::from_hex(&event_id.to_hex()),
        Some(event_id)
    );
    assert_eq!(MembershipEventId::from_hex("not-an-event-id"), None);
}
