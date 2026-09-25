use uc_core::ids::DeviceId;
use uc_core::membership::{
    LedgerInput, MembershipLedger, PeerAdmissionError, PeerAdmissionPort, PeerRelation,
    VersionedMembershipHistory,
};

use super::{admits_peer, known_peer_identities, PeerAccess, PeerIdentityDirectoryPort};
use crate::space::membership::testing::{
    with_peer_relation, AcceptingVerifier, EstablishedSpace, OwnerFixture,
};
use crate::space::membership::{
    ledger_error, MembershipLedgerError, MembershipRecord, MembershipView, SpaceMembershipRecord,
};

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

#[test]
fn a_member_in_a_pending_removal_decision_stays_connected() {
    let space = EstablishedSpace::new(&["device-a", "device-b"]);
    for relation in [
        PeerRelation::AwaitingLocalDecision,
        PeerRelation::AwaitingPeerDecision,
    ] {
        let record = with_peer_relation(
            space.record("device-a", 1),
            &device("device-b"),
            relation,
            None,
        );

        assert!(admits_peer(&view(record), &device("device-b")));
    }
}

/// 在 `history` 上追加本机 `device-a` 移除 `device-b` 的签名事件。
fn with_removal_of_b(
    space: &EstablishedSpace,
    history: &VersionedMembershipHistory,
) -> VersionedMembershipHistory {
    let mut history = history.clone();
    let mut removal = history
        .create_unsigned_local_removal_event(
            space.member("device-a"),
            space.credential("device-a"),
            space.member("device-b"),
            [0x41; 16],
            [0x42; 32],
        )
        .unwrap();
    removal.signature = vec![0x43];
    history
        .verify_and_receive_event(removal, &AcceptingVerifier)
        .unwrap();
    history
}

fn identity_devices(view: &MembershipView) -> Vec<DeviceId> {
    let mut devices: Vec<_> = known_peer_identities(view)
        .unwrap()
        .into_iter()
        .map(|identity| identity.device_id)
        .collect();
    devices.sort();
    devices
}

#[test]
fn identities_cover_the_local_device_and_every_current_member() {
    let space = EstablishedSpace::new(&["device-a", "device-b", "device-c"]);
    let view = view(space.record("device-a", 1));

    assert_eq!(
        identity_devices(&view),
        vec![device("device-a"), device("device-b"), device("device-c")]
    );
    let identities = known_peer_identities(&view).unwrap();
    let b = identities
        .iter()
        .find(|identity| identity.device_id == device("device-b"))
        .unwrap();
    assert_eq!(
        b.identity_fingerprint,
        space.facts("device-b").identity_fingerprint
    );
    assert!(identity_devices(&self::view(MembershipRecord::NoSpace { revision: 2 })).is_empty());
}

#[test]
fn a_device_leaves_the_identities_only_after_its_departure_ends() {
    let space = EstablishedSpace::new(&["device-a", "device-b"]);
    let MembershipRecord::Space(record) = space.record("device-a", 1) else {
        unreachable!("started record has a space");
    };
    let ledger = MembershipLedger::restore(record.ledger).unwrap();
    let history = with_removal_of_b(&space, ledger.history());
    let (mut ledger, _, _) = ledger
        .apply(
            LedgerInput::LocalRemovalSigned {
                history,
                retained_device_ids: Vec::new(),
            },
            1_000,
        )
        .unwrap()
        .into_parts();
    loop {
        let Some(effect) = ledger.unfinished_effects().next().cloned() else {
            break;
        };
        ledger = ledger
            .apply(
                LedgerInput::EffectStepFinished {
                    event_id: effect.event_id(),
                    from: effect.phase(),
                },
                1_000,
            )
            .unwrap()
            .into_parts()
            .0;
    }
    let departing = view(space_record(&ledger));
    assert!(!admits_peer(&departing, &device("device-b")));
    assert_eq!(
        identity_devices(&departing),
        vec![device("device-a"), device("device-b")]
    );

    let (ledger, _, _) = ledger
        .apply(
            LedgerInput::DepartureWindowElapsed {
                peer: device("device-b"),
            },
            1_000 + 300_000,
        )
        .unwrap()
        .into_parts();

    assert_eq!(
        identity_devices(&view(space_record(&ledger))),
        vec![device("device-a")]
    );
}

fn space_record(ledger: &MembershipLedger) -> MembershipRecord {
    MembershipRecord::Space(Box::new(SpaceMembershipRecord {
        ledger: ledger.snapshot(),
        history_exchange: Default::default(),
        branch_recovery: Default::default(),
    }))
}

#[tokio::test]
async fn unbound_access_refuses_every_peer() {
    let access = PeerAccess::unbound();

    assert!(matches!(
        access.is_admitted(&device("device-b")).await,
        Err(PeerAdmissionError::Unavailable)
    ));
    assert!(matches!(
        access.known_peer_identities().await,
        Err(MembershipLedgerError::Unavailable { .. })
    ));
}

/// 移除一经负责人提交，入站判定立即拒绝被移除设备，而它的身份仍可识别；判定不依赖成员读模型的更新。
#[tokio::test]
async fn a_committed_removal_is_enforced_before_any_read_model_update() {
    let space = EstablishedSpace::new(&["device-a", "device-b"]);
    let fixture = OwnerFixture::new(space.record("device-a", 1));
    let access = PeerAccess::unbound();
    access.bind(fixture.owner.clone());
    assert!(access.is_admitted(&device("device-b")).await.unwrap());

    fixture
        .owner
        .commit(|draft| {
            let history = with_removal_of_b(&space, draft.require_space()?.history());
            draft
                .apply(LedgerInput::LocalRemovalSigned {
                    history,
                    retained_device_ids: Vec::new(),
                })
                .map_err(ledger_error)
        })
        .await
        .unwrap();

    assert!(!access.is_admitted(&device("device-b")).await.unwrap());
    assert!(access
        .known_peer_identities()
        .await
        .unwrap()
        .iter()
        .any(|identity| identity.device_id == device("device-b")));
}
