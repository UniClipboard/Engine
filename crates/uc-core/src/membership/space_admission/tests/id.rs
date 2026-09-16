use super::*;

fn assert_stable_value_key<T: Copy + Ord>(first: T, same: T, other: T) {
    let copied = first;
    let mut values = BTreeSet::new();
    values.insert(first);
    values.insert(same);
    values.insert(other);
    assert_eq!(values.len(), 2);
    let _ = copied;
}

#[test]
fn space_admission_id_debug_output_is_redacted() {
    let id = SpaceAdmissionId::from_bytes([0xab; 32]).expect("non-zero admission id fixture");
    let output = format!("{id:?}");
    assert_eq!(output, "SpaceAdmissionId([REDACTED])");
    assert!(!output.contains("abab"));
}

#[test]
fn space_admission_id_returns_its_original_bytes() {
    let bytes = [0xcd; 32];
    let id = SpaceAdmissionId::from_bytes(bytes).expect("non-zero admission id fixture");
    assert_eq!(id.as_bytes(), &bytes);
}

#[test]
fn zero_space_admission_id_is_rejected() {
    let result = SpaceAdmissionId::from_bytes([0; 32]);
    assert!(result.is_none());
}

#[test]
fn space_admission_id_is_a_stable_value_key() {
    let first = SpaceAdmissionId::from_bytes([0x11; 32]).expect("non-zero admission id fixture");
    let copied = first;
    let same = SpaceAdmissionId::from_bytes([0x11; 32]).expect("non-zero admission id fixture");
    let other = SpaceAdmissionId::from_bytes([0x12; 32]).expect("non-zero admission id fixture");

    let mut ids = BTreeSet::new();
    ids.insert(first);
    ids.insert(same);
    ids.insert(other);

    assert_eq!(ids.len(), 2);
    assert_eq!(copied.as_bytes(), &[0x11; 32]);
}

#[test]
fn remaining_admission_ids_preserve_bytes_and_redact_debug_output() {
    let join = JoinId::from_bytes([0x21; 16]).expect("non-zero join id fixture");
    let message = AdmissionMessageId::from_bytes([0x22; 32]).expect("non-zero message id fixture");
    let invitation = InvitationId::from_bytes([0x23; 32]).expect("non-zero invitation id fixture");
    let peer =
        AdmissionChannelPeerId::from_bytes([0x24; 32]).expect("non-zero channel peer id fixture");

    assert_eq!(join.as_bytes(), &[0x21; 16]);
    assert_eq!(message.as_bytes(), &[0x22; 32]);
    assert_eq!(invitation.as_bytes(), &[0x23; 32]);
    assert_eq!(peer.as_bytes(), &[0x24; 32]);
    assert_eq!(format!("{join:?}"), "JoinId([REDACTED])");
    assert_eq!(format!("{message:?}"), "AdmissionMessageId([REDACTED])");
    assert_eq!(format!("{invitation:?}"), "InvitationId([REDACTED])");
    assert_eq!(format!("{peer:?}"), "AdmissionChannelPeerId([REDACTED])");
}

#[test]
fn remaining_admission_ids_reject_zero_and_are_stable_value_keys() {
    assert!(JoinId::from_bytes([0; 16]).is_none());
    assert!(AdmissionMessageId::from_bytes([0; 32]).is_none());
    assert!(InvitationId::from_bytes([0; 32]).is_none());
    assert!(AdmissionChannelPeerId::from_bytes([0; 32]).is_none());

    assert_stable_value_key(
        JoinId::from_bytes([0x31; 16]).expect("non-zero join id fixture"),
        JoinId::from_bytes([0x31; 16]).expect("non-zero join id fixture"),
        JoinId::from_bytes([0x32; 16]).expect("non-zero join id fixture"),
    );
    assert_stable_value_key(
        AdmissionMessageId::from_bytes([0x33; 32]).expect("non-zero message id fixture"),
        AdmissionMessageId::from_bytes([0x33; 32]).expect("non-zero message id fixture"),
        AdmissionMessageId::from_bytes([0x34; 32]).expect("non-zero message id fixture"),
    );
    assert_stable_value_key(
        InvitationId::from_bytes([0x35; 32]).expect("non-zero invitation id fixture"),
        InvitationId::from_bytes([0x35; 32]).expect("non-zero invitation id fixture"),
        InvitationId::from_bytes([0x36; 32]).expect("non-zero invitation id fixture"),
    );
    assert_stable_value_key(
        AdmissionChannelPeerId::from_bytes([0x37; 32]).expect("non-zero channel peer id fixture"),
        AdmissionChannelPeerId::from_bytes([0x37; 32]).expect("non-zero channel peer id fixture"),
        AdmissionChannelPeerId::from_bytes([0x38; 32]).expect("non-zero channel peer id fixture"),
    );
}
