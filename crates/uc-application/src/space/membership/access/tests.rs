use uc_core::ids::DeviceId;
use uc_core::membership::PeerRelation;

use super::admits_peer;
use crate::space::membership::testing::{with_peer_relation, EstablishedSpace};
use crate::space::membership::{MembershipRecord, MembershipView};

fn device(name: &str) -> DeviceId {
    DeviceId::new(name)
}

fn view(record: MembershipRecord) -> MembershipView {
    MembershipView::from_record(record).unwrap()
}

#[test]
fn a_consistent_current_member_is_admitted() {
    let space = EstablishedSpace::new(&["device-a", "device-b"]);

    assert!(admits_peer(
        &view(space.record("device-a", 1)),
        &device("device-b")
    ));
}

#[test]
fn unknown_devices_and_the_missing_space_are_refused() {
    let space = EstablishedSpace::new(&["device-a", "device-b"]);

    assert!(!admits_peer(
        &view(space.record("device-a", 1)),
        &device("device-z")
    ));
    assert!(!admits_peer(
        &view(MembershipRecord::NoSpace { revision: 3 }),
        &device("device-b")
    ));
}

#[test]
fn a_member_whose_history_diverged_is_refused() {
    let space = EstablishedSpace::new(&["device-a", "device-b"]);
    let record = with_peer_relation(
        space.record("device-a", 1),
        &device("device-b"),
        PeerRelation::Diverged,
        None,
    );

    assert!(!admits_peer(&view(record), &device("device-b")));
}
