//! 删除按本机所有权退休；原协议源可以不同，旧意图与删除期间意图都不能复活。
use super::{item, key, Clock};
use crate::transfer::progress::{ProgressProjectionAdmission, ProgressProjectionRegistry};
use std::sync::{atomic::AtomicU64, Arc};
use uc_core::ids::EntryId;

#[test]
fn deletion_retires_original_and_waiting_intents_without_erasing_another_local_owner() {
    let (registry, _worker) = ProgressProjectionRegistry::new(Arc::new(Clock(AtomicU64::new(0))));
    let local = EntryId::from("local-history-owner");
    let original = registry.begin_intent(local.clone());
    let waiting = registry.begin_intent(local.clone());
    let unrelated = registry.begin_intent(EntryId::from("other-local-owner"));
    original.accept(key(1, 8), vec![item(0)]).unwrap();
    unrelated.accept(key(2, 9), vec![item(0)]).unwrap();
    let first = registry.deleting_entry(&local);
    let second = registry.deleting_entry(&local);
    assert!(registry.snapshot(key(1, 8)).is_none());
    assert!(registry.snapshot(key(2, 9)).is_some());
    let during = registry.begin_intent(local.clone());
    for intent in [&original, &waiting, &during] {
        assert_eq!(
            intent.accept(key(1, 8), vec![item(0)]).unwrap(),
            ProgressProjectionAdmission::Closed
        );
    }
    drop(first);
    let still_deleting = registry.begin_intent(local.clone());
    assert_eq!(
        still_deleting.accept(key(1, 8), vec![item(0)]).unwrap(),
        ProgressProjectionAdmission::Closed
    );
    drop(second);
    let fresh = registry.begin_intent(local);
    assert_eq!(
        fresh.accept(key(1, 8), vec![item(0)]).unwrap(),
        ProgressProjectionAdmission::Retained
    );
    for intent in [&original, &waiting, &during, &still_deleting] {
        assert_eq!(
            intent.accept(key(1, 8), vec![item(0)]).unwrap(),
            ProgressProjectionAdmission::Closed
        );
    }
    registry.clear();
    assert_eq!(
        fresh.accept(key(1, 8), vec![item(0)]).unwrap(),
        ProgressProjectionAdmission::Closed
    );
    assert_eq!(
        unrelated.accept(key(2, 9), vec![item(0)]).unwrap(),
        ProgressProjectionAdmission::Closed
    );
}

#[test]
fn peer_retirement_fences_waiting_intent_without_fencing_the_other_target() {
    let (registry, _worker) = ProgressProjectionRegistry::new(Arc::new(Clock(AtomicU64::new(0))));
    let local = EntryId::from("local-history-owner");
    let pending = registry.begin_intent(local.clone());
    pending.accept(key(1, 8), vec![item(0)]).unwrap();
    pending.accept(key(2, 8), vec![item(0)]).unwrap();
    registry.remove_peer(key(1, 8).peer);
    assert!(registry.snapshot(key(1, 8)).is_none());
    assert!(registry.snapshot(key(2, 8)).is_some());
    assert_eq!(
        pending.accept(key(1, 8), vec![item(0)]).unwrap(),
        ProgressProjectionAdmission::Closed
    );
    assert_eq!(
        pending.accept(key(2, 8), vec![item(0)]).unwrap(),
        ProgressProjectionAdmission::Unchanged
    );
    let fresh = registry.begin_intent(local);
    assert_eq!(
        fresh.accept(key(1, 8), vec![item(0)]).unwrap(),
        ProgressProjectionAdmission::Retained
    );
    assert_eq!(
        pending.accept(key(1, 8), vec![item(0)]).unwrap(),
        ProgressProjectionAdmission::Closed
    );
}

#[test]
fn indefinitely_waiting_intent_has_bounded_retirement_memory_and_does_not_erase_other_slots() {
    use uc_core::file_transfer::PROJECTION_GLOBAL_CAPACITY;
    let (registry, _worker) = ProgressProjectionRegistry::new(Arc::new(Clock(AtomicU64::new(0))));
    let owner = EntryId::from("long-running-action");
    let pending = registry.begin_intent(owner.clone());
    pending.accept(key(1000, 8), vec![item(0)]).unwrap();
    for peer in 0..=PROJECTION_GLOBAL_CAPACITY as u16 {
        registry.remove_peer(key(peer, 8).peer);
    }
    assert!(registry.snapshot(key(1000, 8)).is_some());
    assert_eq!(
        pending.accept(key(1001, 8), vec![item(0)]).unwrap(),
        ProgressProjectionAdmission::Closed
    );
    let fresh = registry.begin_intent(owner);
    assert_eq!(
        fresh.accept(key(1001, 8), vec![item(0)]).unwrap(),
        ProgressProjectionAdmission::Retained
    );
}
