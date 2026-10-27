use super::source::ProgressSourceCatalog;
use uc_core::file_transfer::{ProgressIdentity, ProgressItem, ProgressItemKind};
use uc_core::ids::{DeviceId, EntryId};
fn id(value: u16) -> ProgressIdentity {
    let mut bytes = [0; 16];
    bytes[14..].copy_from_slice(&value.to_be_bytes());
    ProgressIdentity::new(bytes)
}
fn items() -> Vec<ProgressItem> {
    vec![ProgressItem {
        kind: ProgressItemKind::File,
        index: 0,
    }]
}

#[test]
fn repeated_handoff_is_idempotent_and_new_attempt_replaces_without_uuid_ordering() {
    let mut owner = ProgressSourceCatalog::default();
    let peer = DeviceId::new("source");
    let local = EntryId::from("local");
    owner.accept(&local, peer, id(1), id(9), items());
    let first = owner.current(&peer, id(1)).unwrap();
    owner.accept(&local, peer, id(1), id(9), items());
    assert_eq!(
        owner.current(&peer, id(1)).unwrap().descriptor.revision,
        first.descriptor.revision
    );
    owner.accept(&local, peer, id(1), id(2), items());
    let second = owner.current(&peer, id(1)).unwrap();
    assert_eq!(second.descriptor.producer, first.descriptor.producer);
    assert!(second.descriptor.revision > first.descriptor.revision);
    assert_eq!(second.descriptor.attempt, id(2));
    assert!(owner.current(&DeviceId::new("provider"), id(1)).is_none());
}
#[test]
fn peer_and_global_capacity_evict_oldest_metadata_without_affecting_other_peers_early() {
    let mut owner = ProgressSourceCatalog::default();
    let a = DeviceId::new("a");
    let b = DeviceId::new("b");
    owner.accept(&EntryId::from("other"), b, id(0), id(1), items());
    for index in 1..=65 {
        owner.accept(
            &EntryId::from(format!("entry-{index}")),
            a,
            id(index),
            id(1),
            items(),
        );
    }
    assert!(owner.current(&a, id(1)).is_none());
    assert!(owner.current(&a, id(2)).is_some());
    assert!(owner.current(&b, id(0)).is_some());
    for group in 0..4 {
        for index in 0..64 {
            let peer = DeviceId::new(format!("peer-{group}"));
            owner.accept(
                &EntryId::from(format!("group-{group}-{index}")),
                peer,
                id(index),
                id(1),
                items(),
            );
        }
    }
    assert!(owner.current(&b, id(0)).is_none());
    assert!(owner.current(&a, id(65)).is_none());
    for group in 0..4 {
        for index in 0..64 {
            assert!(owner
                .current(&DeviceId::new(format!("peer-{group}")), id(index))
                .is_some());
        }
    }
}
#[test]
fn clearing_preserves_revision_but_session_retirement_creates_new_producer() {
    let mut owner = ProgressSourceCatalog::default();
    let peer = DeviceId::new("peer");
    let entry = EntryId::from("local");
    owner.accept(&entry, peer, id(1), id(2), items());
    let first = owner.current(&peer, id(1)).unwrap();
    owner.clear();
    assert!(owner.current(&peer, id(1)).is_none());
    owner.accept(&entry, peer, id(1), id(3), items());
    let next = owner.current(&peer, id(1)).unwrap();
    assert_eq!(next.descriptor.producer, first.descriptor.producer);
    assert!(next.descriptor.revision > first.descriptor.revision);
    owner.retire();
    owner.accept(&entry, peer, id(1), id(4), items());
    assert_ne!(
        owner.current(&peer, id(1)).unwrap().descriptor.producer,
        first.descriptor.producer
    );
}

#[test]
fn terminal_intent_claim_is_once_across_duplicate_handoff_and_new_attempt_is_separate() {
    use super::ProgressClaimRefusal;
    let mut owner = ProgressSourceCatalog::default();
    let local = EntryId::from("entry");
    let peer = DeviceId::new("source");
    owner.accept(&local, peer, id(1), id(2), items());
    let before = owner.current(&peer, id(1)).unwrap();
    assert!(owner
        .claim_terminal(&peer, before.descriptor, items()[0])
        .is_ok());
    assert!(before.notification_closed.is_empty());
    assert!(owner
        .current(&peer, id(1))
        .unwrap()
        .notification_closed
        .contains(&items()[0]));
    owner.accept(&local, peer, id(1), id(2), items());
    assert!(matches!(
        owner.claim_terminal(&peer, before.descriptor, items()[0]),
        Err(ProgressClaimRefusal::AlreadyObserved)
    ));
    owner.accept(&local, peer, id(1), id(3), items());
    assert!(owner
        .current(&peer, id(1))
        .unwrap()
        .notification_closed
        .is_empty());
    assert!(matches!(
        owner.claim_terminal(&peer, before.descriptor, items()[0]),
        Err(ProgressClaimRefusal::Superseded)
    ));
    assert!(owner
        .claim_terminal(
            &peer,
            owner.current(&peer, id(1)).unwrap().descriptor,
            items()[0]
        )
        .is_ok());
}
