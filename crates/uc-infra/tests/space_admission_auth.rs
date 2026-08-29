use std::error::Error;
use uc_core::crypto::domain::Passphrase;
use uc_core::membership::{
    AdmissionChannelPeerId, InvitationId, SpaceAdmissionId, SpaceAdmissionProtocolVersion,
};
use uc_infra::security::{SpaceAdmissionAuth, SpaceAdmissionAuthContext, SpaceAdmissionAuthError};

fn authentication_context() -> SpaceAdmissionAuthContext {
    SpaceAdmissionAuthContext::new(
        SpaceAdmissionProtocolVersion::V1,
        SpaceAdmissionId::from_bytes([0x11; 32]).expect("non-zero admission id"),
        InvitationId::from_bytes([0x22; 32]).expect("non-zero invitation id"),
        AdmissionChannelPeerId::from_bytes([0x33; 32]).expect("non-zero Joiner peer id"),
        AdmissionChannelPeerId::from_bytes([0x44; 32]).expect("non-zero Sponsor peer id"),
    )
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
