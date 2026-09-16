use super::*;

#[test]
fn continuation_mac_binds_direction_identity_nonce_and_digest() {
    let credential = credential();
    let admission = admission_id();
    let sender = peer(0x33);
    let receiver = peer(0x34);
    let nonce = [0x35; 32];
    let digest = [0x36; 32];
    let mac = calculate_mac(
        &credential,
        b"request",
        admission,
        sender,
        receiver,
        &nonce,
        &digest,
        None,
    )
    .expect("valid MAC inputs");

    assert!(verify_mac(
        &credential,
        b"request",
        admission,
        sender,
        receiver,
        &nonce,
        &digest,
        None,
        &mac,
    )
    .is_ok());
    assert!(verify_mac(
        &credential,
        b"reply",
        admission,
        sender,
        receiver,
        &nonce,
        &digest,
        None,
        &mac,
    )
    .is_err());
    assert!(verify_mac(
        &credential,
        b"request",
        admission,
        receiver,
        sender,
        &nonce,
        &digest,
        None,
        &mac,
    )
    .is_err());
    let mut changed_nonce = nonce;
    changed_nonce[0] ^= 1;
    assert!(verify_mac(
        &credential,
        b"request",
        admission,
        sender,
        receiver,
        &changed_nonce,
        &digest,
        None,
        &mac,
    )
    .is_err());
    let mut changed_digest = digest;
    changed_digest[0] ^= 1;
    assert!(verify_mac(
        &credential,
        b"request",
        admission,
        sender,
        receiver,
        &nonce,
        &changed_digest,
        None,
        &mac,
    )
    .is_err());

    let trace_context = WireTraceContext {
        traceparent: "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".to_owned(),
    };
    assert!(verify_mac(
        &credential,
        b"request",
        admission,
        sender,
        receiver,
        &nonce,
        &digest,
        Some(&trace_context),
        &mac,
    )
    .is_err());
}
