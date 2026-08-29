use std::error::Error;
use uc_core::crypto::domain::Passphrase;
use uc_core::membership::{
    AdmissionChannelPeerId, InvitationId, SpaceAdmissionId, SpaceAdmissionProtocolVersion,
};
use uc_infra::security::{SpaceAdmissionAuth, SpaceAdmissionAuthContext, SpaceAdmissionAuthError};

fn authentication_context() -> SpaceAdmissionAuthContext {
    authentication_context_with_ids([0x11; 32], [0x22; 32], [0x33; 32], [0x44; 32])
}

fn authentication_context_with_ids(
    admission_id: [u8; 32],
    invitation_id: [u8; 32],
    joiner_peer_id: [u8; 32],
    sponsor_peer_id: [u8; 32],
) -> SpaceAdmissionAuthContext {
    SpaceAdmissionAuthContext::new(
        SpaceAdmissionProtocolVersion::V1,
        SpaceAdmissionId::from_bytes(admission_id).expect("non-zero admission id"),
        InvitationId::from_bytes(invitation_id).expect("non-zero invitation id"),
        AdmissionChannelPeerId::from_bytes(joiner_peer_id).expect("non-zero Joiner peer id"),
        AdmissionChannelPeerId::from_bytes(sponsor_peer_id).expect("non-zero Sponsor peer id"),
    )
}

#[test]
fn mismatched_admission_identity_context_cannot_authenticate_the_exchange() {
    assert_context_mismatch_is_rejected(authentication_context_with_ids(
        [0x55; 32], [0x22; 32], [0x33; 32], [0x44; 32],
    ));
    assert_context_mismatch_is_rejected(authentication_context_with_ids(
        [0x11; 32], [0x55; 32], [0x33; 32], [0x44; 32],
    ));
    assert_context_mismatch_is_rejected(authentication_context_with_ids(
        [0x11; 32], [0x22; 32], [0x55; 32], [0x44; 32],
    ));
    assert_context_mismatch_is_rejected(authentication_context_with_ids(
        [0x11; 32], [0x22; 32], [0x33; 32], [0x55; 32],
    ));
}

fn assert_context_mismatch_is_rejected(sponsor_context: SpaceAdmissionAuthContext) {
    let passphrase = Passphrase::new("correct horse battery staple");
    let joiner_context = authentication_context();
    let server_setup = SpaceAdmissionAuth::generate_server_setup();
    let registration = SpaceAdmissionAuth::register(&server_setup, &passphrase)
        .expect("the Space passphrase should produce a registration record");

    let (client_state, ke1) = SpaceAdmissionAuth::start_client(&passphrase, &joiner_context)
        .expect("the Joiner should create KE1");
    let (_server_state, ke2) =
        SpaceAdmissionAuth::start_server(&server_setup, &registration, &sponsor_context, ke1)
            .expect("the Sponsor should create KE2");
    let error = match client_state.finish(&joiner_context, ke2) {
        Ok(_) => panic!("different admission identity contexts must not authenticate"),
        Err(error) => error,
    };

    assert!(matches!(
        error,
        SpaceAdmissionAuthError::Authentication { .. }
    ));
    assert!(error.source().is_some());
}

#[test]
fn matching_passphrase_establishes_the_same_bound_continuation_credential() {
    let passphrase = Passphrase::new("correct horse battery staple");
    let context = authentication_context();
    let server_setup = SpaceAdmissionAuth::generate_server_setup();
    let registration = SpaceAdmissionAuth::register(&server_setup, &passphrase)
        .expect("the Space passphrase should produce a registration record");

    let (client_state, ke1) = SpaceAdmissionAuth::start_client(&passphrase, &context)
        .expect("the Joiner should create KE1");
    let (server_state, ke2) =
        SpaceAdmissionAuth::start_server(&server_setup, &registration, &context, ke1)
            .expect("the Sponsor should create KE2");
    let (client_credential, ke3) = client_state
        .finish(&context, ke2)
        .expect("the Joiner should authenticate the Sponsor");
    let server_credential = server_state
        .finish(&context, ke3)
        .expect("the Sponsor should authenticate the Joiner");

    assert!(client_credential == server_credential);
}

#[test]
fn authentication_failure_preserves_classification_and_source() {
    let registered_passphrase = Passphrase::new("correct horse battery staple");
    let attempted_passphrase = Passphrase::new("incorrect horse battery staple");
    let context = authentication_context();
    let server_setup = SpaceAdmissionAuth::generate_server_setup();
    let registration = SpaceAdmissionAuth::register(&server_setup, &registered_passphrase)
        .expect("the Space passphrase should produce a registration record");

    let (client_state, ke1) = SpaceAdmissionAuth::start_client(&attempted_passphrase, &context)
        .expect("the Joiner should create KE1 without revealing whether the passphrase matches");
    let (_server_state, ke2) =
        SpaceAdmissionAuth::start_server(&server_setup, &registration, &context, ke1).expect(
            "the Sponsor should create KE2 without revealing whether the passphrase matches",
        );
    let error = match client_state.finish(&context, ke2) {
        Ok(_) => panic!("an incorrect passphrase must not authenticate"),
        Err(error) => error,
    };

    assert!(matches!(
        error,
        SpaceAdmissionAuthError::Authentication { .. }
    ));
    assert!(error.source().is_some());
}
