use crate::ids::SpaceId;
use crate::membership::{
    AdmissionAttemptContractError, AdmissionAttemptContractV2, AdmissionChannelPeerId,
    AdmissionMemberBindingV2, InvitationId, MemberInstanceId, MembershipEventId, SpaceAdmissionId,
    SPACE_ADMISSION_ATTEMPT_DURATION_MS,
};

use super::super::attempt::AdmissionAttemptTimeline;

fn attempt(admission: u8) -> AdmissionAttemptContractV2 {
    AdmissionAttemptContractV2::start(
        SpaceAdmissionId::from_bytes([admission; 32]).expect("non-zero admission"),
        InvitationId::from_bytes([0x22; 32]).expect("non-zero invitation"),
        AdmissionChannelPeerId::from_bytes([0x33; 32]).expect("non-zero joiner"),
        AdmissionChannelPeerId::from_bytes([0x44; 32]).expect("non-zero sponsor"),
        1_789_268_400_000,
    )
    .expect("valid attempt")
}

#[test]
fn attempt_has_one_five_minute_budget() {
    let attempt = attempt(0x11);
    let encoded = attempt.canonical_bytes();

    assert_eq!(
        attempt.expires_at_ms() - attempt.started_at_ms(),
        SPACE_ADMISSION_ATTEMPT_DURATION_MS
    );
    assert_eq!(SPACE_ADMISSION_ATTEMPT_DURATION_MS, 300_000);
    assert_eq!(
        AdmissionAttemptContractV2::decode_canonical(&encoded).expect("valid encoding"),
        attempt
    );
    let mut trailing = encoded;
    trailing.push(0);
    assert_eq!(
        AdmissionAttemptContractV2::decode_canonical(&trailing),
        Err(AdmissionAttemptContractError::InvalidEncoding)
    );
}

#[test]
fn attempt_rejects_a_renewed_or_shortened_deadline() {
    let common = (
        SpaceAdmissionId::from_bytes([0x11; 32]).expect("non-zero admission"),
        InvitationId::from_bytes([0x22; 32]).expect("non-zero invitation"),
        AdmissionChannelPeerId::from_bytes([0x33; 32]).expect("non-zero joiner"),
        AdmissionChannelPeerId::from_bytes([0x44; 32]).expect("non-zero sponsor"),
    );

    for deadline in [1_789_268_699_999, 1_789_268_700_001] {
        assert_eq!(
            AdmissionAttemptContractV2::new(
                common.0,
                common.1,
                common.2,
                common.3,
                1_789_268_400_000,
                deadline,
            ),
            Err(AdmissionAttemptContractError::InvalidDeadline)
        );
    }
}

#[test]
fn attempt_rejects_invalid_time_and_peer_boundaries() {
    let admission = SpaceAdmissionId::from_bytes([0x11; 32]).expect("non-zero admission");
    let invitation = InvitationId::from_bytes([0x22; 32]).expect("non-zero invitation");
    let peer = AdmissionChannelPeerId::from_bytes([0x33; 32]).expect("non-zero peer");
    let sponsor = AdmissionChannelPeerId::from_bytes([0x44; 32]).expect("non-zero sponsor");

    assert_eq!(
        AdmissionAttemptContractV2::start(admission, invitation, peer, sponsor, -1),
        Err(AdmissionAttemptContractError::InvalidStartTime)
    );
    assert_eq!(
        AdmissionAttemptContractV2::start(admission, invitation, peer, sponsor, i64::MAX),
        Err(AdmissionAttemptContractError::DeadlineOverflow)
    );
    assert_eq!(
        AdmissionAttemptContractV2::start(admission, invitation, peer, peer, 0),
        Err(AdmissionAttemptContractError::IdenticalPeers)
    );
}

#[test]
fn attempt_digest_binds_every_authenticated_field() {
    let original = attempt(0x11);
    let different_attempt = attempt(0x12);
    let different_invitation = AdmissionAttemptContractV2::start(
        original.admission_id(),
        InvitationId::from_bytes([0x23; 32]).expect("non-zero invitation"),
        original.joiner_peer_id(),
        original.sponsor_peer_id(),
        original.started_at_ms(),
    )
    .expect("valid attempt");
    let different_start = AdmissionAttemptContractV2::start(
        original.admission_id(),
        original.invitation_id(),
        original.joiner_peer_id(),
        original.sponsor_peer_id(),
        original.started_at_ms() + 1,
    )
    .expect("valid attempt");
    let different_joiner = AdmissionAttemptContractV2::start(
        original.admission_id(),
        original.invitation_id(),
        AdmissionChannelPeerId::from_bytes([0x55; 32]).expect("non-zero joiner"),
        original.sponsor_peer_id(),
        original.started_at_ms(),
    )
    .expect("valid attempt");
    let different_sponsor = AdmissionAttemptContractV2::start(
        original.admission_id(),
        original.invitation_id(),
        original.joiner_peer_id(),
        AdmissionChannelPeerId::from_bytes([0x55; 32]).expect("non-zero sponsor"),
        original.started_at_ms(),
    )
    .expect("valid attempt");

    assert_ne!(original.digest(), different_attempt.digest());
    assert_ne!(original.digest(), different_invitation.digest());
    assert_ne!(original.digest(), different_start.digest());
    assert_ne!(original.digest(), different_joiner.digest());
    assert_ne!(original.digest(), different_sponsor.digest());
    assert_eq!(
        hex::encode(original.digest()),
        "77c8c49a972eab5eef57fbfb84f1679527ef8a6caeddcf122d37af47f4cba3e6"
    );
}

#[test]
fn member_binding_cannot_be_replayed_for_a_new_attempt() {
    let old = attempt(0x11);
    let new = attempt(0x12);
    let member = MemberInstanceId::from_bytes([0x55; 32]);
    let event = MembershipEventId::from_hex(&"66".repeat(32)).expect("valid event");
    let old_binding =
        AdmissionMemberBindingV2::new(old.digest(), SpaceId::from_str("space-a"), member, event)
            .expect("valid binding");
    let new_binding =
        AdmissionMemberBindingV2::new(new.digest(), SpaceId::from_str("space-a"), member, event)
            .expect("valid binding");

    assert_ne!(old_binding.digest(), new_binding.digest());
    assert_eq!(
        AdmissionMemberBindingV2::decode_canonical(&old_binding.canonical_bytes())
            .expect("valid member binding"),
        old_binding
    );

    let other_space =
        AdmissionMemberBindingV2::new(old.digest(), SpaceId::from_str("space-b"), member, event)
            .expect("valid binding");
    let other_member = AdmissionMemberBindingV2::new(
        old.digest(),
        SpaceId::from_str("space-a"),
        MemberInstanceId::from_bytes([0x77; 32]),
        event,
    )
    .expect("valid binding");
    let other_event = AdmissionMemberBindingV2::new(
        old.digest(),
        SpaceId::from_str("space-a"),
        member,
        MembershipEventId::from_hex(&"88".repeat(32)).expect("valid event"),
    )
    .expect("valid binding");
    assert_ne!(old_binding.digest(), other_space.digest());
    assert_ne!(old_binding.digest(), other_member.digest());
    assert_ne!(old_binding.digest(), other_event.digest());
}

#[test]
fn attempt_contract_debug_redacts_all_identifiers_and_times() {
    let debug = format!("{:?}", attempt(0x11));

    assert_eq!(debug, "AdmissionAttemptContractV2([REDACTED])");
    assert!(!debug.contains("1789268400000"));
}

#[test]
fn local_attempt_timeline_expires_at_the_single_five_minute_boundary() {
    let timeline = AdmissionAttemptTimeline::start(1_000).expect("valid local attempt timeline");

    assert_eq!(timeline.started_at_ms(), 1_000);
    assert_eq!(timeline.expires_at_ms(), 301_000);
    assert!(!timeline.is_expired(300_999));
    assert!(timeline.is_expired(301_000));
}

#[test]
fn local_attempt_timeline_rejects_invalid_or_renewed_boundaries() {
    assert_eq!(
        AdmissionAttemptTimeline::start(-1),
        Err(AdmissionAttemptContractError::InvalidStartTime)
    );
    assert_eq!(
        AdmissionAttemptTimeline::start(i64::MAX),
        Err(AdmissionAttemptContractError::DeadlineOverflow)
    );
    assert_eq!(
        AdmissionAttemptTimeline::new(1_000, 301_001),
        Err(AdmissionAttemptContractError::InvalidDeadline)
    );
}
