use uc_core::ids::DeviceId;
use uc_core::membership::{
    AdmissionBaseSnapshot, AdmissionCandidateV1, AdmissionChangeFacts, AdmissionChannelPeerId,
    AdmissionContinuationCredential, AdmissionContinuationRoute,
    AdmissionEncryptedPasswordEquivalent, AdmissionIdentitySignature, AdmissionInvitationClaim,
    AdmissionJoinRequestV1, AdmissionJoinerPrivateState, AdmissionJoinerStartContext,
    AdmissionKeyPackage, AdmissionMessageId, AdmissionMlsCommit, AdmissionMlsWelcome,
    AdmissionPeerBinding, AdmissionPendingRecovery, AdmissionPreparedV1,
    AdmissionRecordPersistence, AdmissionRecoveryPublicKey, AdmissionRetryState, AdmissionRole,
    AdmissionSecurityCommitmentV1, AdmissionShortInvitationCode, AdmissionSignedMembershipHistory,
    AdmissionSourceSnapshot, AdmissionStagedSecurityState, AdmissionStagedTarget,
    AdmissionStagedTargetInput, BaseMembershipHistoryPosition, InvitationId, JoinId,
    JoinerAdmission, JoinerInvitationResolution, MemberInstanceId, MembershipAdmissionV2,
    MembershipCredential, MembershipEventV2, MembershipOperationV2, PendingAdmissionExchange,
    PreparedAdmissionProofV1, SpaceAdmissionBodyV1, SpaceAdmissionEnvelopeV1, SpaceAdmissionId,
    SpaceAdmissionMessageKind, SpaceAdmissionPersistenceError, SpaceAdmissionRoute,
    SponsorAdmission, UnreadableHistoryPolicy, ADMISSION_SECURITY_COMMITMENT_FORMAT_V1,
    ED25519_SIGNATURE_ALGORITHM_V1, MEMBERSHIP_EVENT_FORMAT_V2,
};
use uc_core::pairing::invitation::FullInvitation;
use uc_core::security::IdentityFingerprint;

#[test]
fn joiner_short_invitation_resolution_is_at_most_once_and_persisted() {
    let admission_id = admission_id();
    let resolving = JoinerAdmission::start_resolving_invitation(
        admission_id,
        JoinId::from_bytes([0x72; 16]).expect("valid join id"),
        8,
        AdmissionSourceSnapshot::from_bytes(vec![0x73; 32]).expect("valid source snapshot"),
        AdmissionJoinerStartContext::from_bytes(vec![0x74; 64]).expect("valid start context"),
        AdmissionShortInvitationCode::from_bytes(b"ABCD-1234".to_vec()).expect("valid short code"),
    )
    .expect("short invitation resolution should start")
    .into_replacement();
    let resolving = round_trip_joiner(resolving);
    assert!(matches!(
        resolving.invitation_resolution(),
        Some(JoinerInvitationResolution::Ready { short_code, .. })
            if short_code.as_bytes() == b"ABCD-1234"
    ));

    let started = resolving
        .mark_invitation_resolution_started()
        .expect("saved short code can be consumed once");
    let (started, short_code) = started.into_parts();
    assert_eq!(short_code.as_bytes(), b"ABCD-1234");
    let started = started.into_replacement();
    let started_bytes = started
        .encode_persisted()
        .expect("started resolution should encode");
    assert!(!started_bytes
        .windows(b"ABCD-1234".len())
        .any(|window| window == b"ABCD-1234"));
    let started = round_trip_joiner(started);
    assert!(matches!(
        started.invitation_resolution(),
        Some(JoinerInvitationResolution::Started { .. })
    ));
    let abandoned = JoinerAdmission::decode_persisted(&started_bytes)
        .expect("started resolution should decode")
        .reject_started_invitation_resolution()
        .expect("an ambiguous started resolution must be abandoned")
        .into_replacement();
    assert!(abandoned.is_terminal());

    let full_invitation =
        FullInvitation::new("ucspace1_resolved-invitation").expect("valid full invitation");
    let resolved = started
        .save_resolved_invitation(full_invitation.clone())
        .expect("the single response can be saved")
        .into_replacement();
    let resolved = round_trip_joiner(resolved);
    assert!(matches!(
        resolved.invitation_resolution(),
        Some(JoinerInvitationResolution::Resolved {
            full_invitation: saved,
            start_context,
        }) if saved == &full_invitation && start_context.as_bytes() == &[0x74; 64]
    ));
}

#[test]
fn joiner_invitation_resolution_can_cancel_before_any_connection() {
    let ready = JoinerAdmission::start_resolving_invitation(
        admission_id(),
        JoinId::from_bytes([0x75; 16]).expect("valid join id"),
        9,
        AdmissionSourceSnapshot::from_bytes(vec![0x76; 32]).expect("valid source snapshot"),
        AdmissionJoinerStartContext::from_bytes(vec![0x77; 64]).expect("valid start context"),
        AdmissionShortInvitationCode::from_bytes(b"CANCEL-ME".to_vec()).expect("valid short code"),
    )
    .expect("ready resolution")
    .into_replacement();

    let cancelled = ready
        .cancel_before_authentication()
        .expect("pre-connection resolution can cancel")
        .into_replacement();

    assert!(cancelled.is_terminal());
}

#[test]
fn initiated_joiner_awaiting_authentication_round_trips_through_persistence() {
    let decoded = round_trip_joiner(initiated_joiner_fixture());
    assert!(matches!(
        decoded.pending_recovery(),
        Some(AdmissionPendingRecovery::Initial { .. })
    ));
}

#[test]
fn initiated_joiner_authenticated_channel_round_trips_through_persistence() {
    let decoded = round_trip_joiner(authenticated_joiner_fixture());
    assert!(matches!(
        decoded.pending_recovery(),
        Some(AdmissionPendingRecovery::Continuation { .. })
    ));
    assert_eq!(
        decoded
            .joiner_candidate_preparation()
            .expect("authenticated Joiner retains private state")
            .private_state()
            .as_bytes(),
        &[0x80; 64]
    );
}

#[test]
fn joiner_candidate_round_trips_through_persistence() {
    let candidate = round_trip_joiner(joiner_candidate_fixture());
    assert_joiner_private_state_absent(&candidate);
}

#[test]
fn joiner_prepared_round_trips_through_persistence() {
    let decoded = round_trip_joiner(joiner_prepared_fixture());
    assert_eq!(
        decoded.current_exact_reply().map(|reply| reply.kind()),
        Some(SpaceAdmissionMessageKind::Prepared)
    );
}

#[test]
fn sponsor_accepted_round_trips_through_persistence() {
    let decoded = round_trip_sponsor(sponsor_accepted_fixture());
    assert!(decoded.sponsor_candidate_preparation().is_some());
}

#[test]
fn sponsor_candidate_round_trips_through_persistence_with_exact_reply() {
    let decoded = round_trip_sponsor(sponsor_candidate_fixture());
    assert_eq!(
        decoded.current_exact_reply().map(|reply| reply.kind()),
        Some(SpaceAdmissionMessageKind::Candidate)
    );
}

#[test]
fn persistence_rejects_unknown_version_and_corrupt_payload() {
    let mut unknown_version = initiated_joiner_fixture()
        .encode_persisted()
        .expect("initial Joiner state should encode");
    unknown_version[0] = 2;

    assert_eq!(
        JoinerAdmission::decode_persisted(&unknown_version),
        Err(SpaceAdmissionPersistenceError::UnsupportedVersion)
    );
    assert_eq!(
        JoinerAdmission::decode_persisted(&[0xff]),
        Err(SpaceAdmissionPersistenceError::InvalidEncoding)
    );
}

#[test]
fn superseded_terminal_round_trips_through_persistence() {
    let superseded = initiated_joiner_fixture()
        .supersede()
        .expect("Initiated Joiner can be superseded")
        .into_replacement();

    let superseded = round_trip_joiner(superseded);
    assert_joiner_private_state_absent(&superseded);
}

#[test]
fn persisted_records_reject_the_wrong_role_capability() {
    let joiner = initiated_joiner_fixture();
    let sponsor = sponsor_accepted_fixture();

    assert_eq!(
        SponsorAdmission::decode_persisted(&joiner.encode_persisted().unwrap()),
        Err(SpaceAdmissionPersistenceError::InvalidState)
    );
    assert_eq!(
        JoinerAdmission::decode_persisted(&sponsor.encode_persisted().unwrap()),
        Err(SpaceAdmissionPersistenceError::InvalidState)
    );
}

fn initiated_joiner_fixture() -> JoinerAdmission {
    let admission_id = admission_id();
    JoinerAdmission::start_join(
        admission_id,
        JoinId::from_bytes([0x82; 16]).expect("non-zero join id fixture"),
        7,
        AdmissionSourceSnapshot::from_bytes(vec![0x83; 64])
            .expect("bounded source snapshot fixture"),
        AdmissionJoinerPrivateState::from_bytes(vec![0x80; 64])
            .expect("bounded Joiner private state fixture"),
        AdmissionEncryptedPasswordEquivalent::from_bytes(vec![0x84; 64])
            .expect("bounded encrypted password fixture"),
        PendingAdmissionExchange::new(
            SpaceAdmissionRoute::from_bytes(vec![0x85; 32]).expect("bounded route fixture"),
            join_request_envelope(admission_id),
            SpaceAdmissionMessageKind::Candidate,
            AdmissionRetryState::new(3, 42).expect("valid retry state fixture"),
        )
        .expect("JoinRequest expects Candidate"),
    )
    .expect("complete initial Joiner fixture")
    .into_replacement()
}

fn authenticated_joiner_fixture() -> JoinerAdmission {
    initiated_joiner_fixture()
        .with_authenticated_channel(peer_binding(), continuation_credential())
        .expect("initial Joiner can save an authenticated channel")
        .into_replacement()
}

fn joiner_candidate_fixture() -> JoinerAdmission {
    authenticated_joiner_fixture()
        .accept_candidate(
            candidate_envelope(admission_id()),
            [0x94; 32],
            AdmissionStagedTargetInput::from_bytes(vec![0x95; 128])
                .expect("bounded staged target input fixture"),
        )
        .expect("valid Joiner Candidate fixture")
        .into_replacement()
}

fn joiner_prepared_fixture() -> JoinerAdmission {
    let candidate = joiner_candidate_fixture();
    let candidate_material = candidate_body_fixture();
    let prepared = SpaceAdmissionEnvelopeV1::new(
        candidate.admission_id(),
        AdmissionRole::Joiner,
        1,
        AdmissionMessageId::from_bytes([0x96; 32]).expect("non-zero Prepared id fixture"),
        Some(candidate_message_id()),
        SpaceAdmissionBodyV1::Prepared(AdmissionPreparedV1::new(prepared_proof(
            candidate.admission_id(),
            &candidate_material,
        ))),
    )
    .expect("valid Prepared request fixture");
    let pending_exchange = PendingAdmissionExchange::new(
        SpaceAdmissionRoute::from_bytes(vec![0x97; 32]).expect("bounded Prepared route fixture"),
        prepared,
        SpaceAdmissionMessageKind::Commit,
        AdmissionRetryState::new(2, 84).expect("valid Prepared retry fixture"),
    )
    .expect("Prepared expects Commit");
    candidate
        .prepare_candidate(
            AdmissionSignedMembershipHistory::from_bytes(vec![0x98; 128])
                .expect("bounded verified history fixture"),
            AdmissionStagedTarget::from_bytes(vec![0x99; 128])
                .expect("bounded staged target fixture"),
            pending_exchange,
        )
        .expect("valid Joiner Prepared fixture")
        .into_replacement()
}

fn sponsor_accepted_fixture() -> SponsorAdmission {
    let admission_id = admission_id();
    let join_request = join_request_envelope(admission_id);
    let evidence = join_request
        .evidence([0xa1; 32])
        .expect("non-zero canonical digest fixture");
    SponsorAdmission::accept_join_request(
        admission_id,
        AdmissionInvitationClaim::from_bytes(vec![0xa2; 32])
            .expect("bounded invitation claim fixture"),
        join_request,
        evidence,
        AdmissionBaseSnapshot::from_bytes(vec![0xa3; 64]).expect("bounded base snapshot fixture"),
        peer_binding(),
        continuation_credential(),
    )
    .expect("valid Sponsor Accepted fixture")
    .into_replacement()
}

fn sponsor_candidate_fixture() -> SponsorAdmission {
    sponsor_accepted_fixture()
        .fix_candidate(
            candidate_envelope(admission_id()),
            AdmissionStagedSecurityState::from_bytes(vec![0xa4; 128])
                .expect("bounded staged security fixture"),
        )
        .expect("valid Sponsor Candidate fixture")
        .into_replacement()
}

fn join_request_envelope(admission_id: SpaceAdmissionId) -> SpaceAdmissionEnvelopeV1 {
    let request = AdmissionJoinRequestV1::new(
        InvitationId::from_bytes([0x86; 32]).expect("non-zero invitation id fixture"),
        DeviceId::new("persisted-joining-device"),
        MembershipCredential::new(1, vec![0x87; 32]),
        AdmissionKeyPackage::from_bytes(vec![0x88; 48]).expect("bounded key package fixture"),
        AdmissionRecoveryPublicKey::from_bytes([0x89; 32])
            .expect("non-zero recovery public key fixture"),
        AdmissionIdentitySignature::from_bytes(vec![0x8a; 64])
            .expect("bounded identity signature fixture"),
        UnreadableHistoryPolicy::Preserve,
    )
    .expect("complete JoinRequest fixture");
    SpaceAdmissionEnvelopeV1::new(
        admission_id,
        AdmissionRole::Joiner,
        0,
        join_request_message_id(),
        None,
        SpaceAdmissionBodyV1::JoinRequest(request),
    )
    .expect("valid JoinRequest envelope fixture")
}

fn candidate_envelope(admission_id: SpaceAdmissionId) -> SpaceAdmissionEnvelopeV1 {
    SpaceAdmissionEnvelopeV1::new(
        admission_id,
        AdmissionRole::Sponsor,
        0,
        candidate_message_id(),
        Some(join_request_message_id()),
        SpaceAdmissionBodyV1::Candidate(candidate_body_fixture()),
    )
    .expect("valid Candidate envelope fixture")
}

fn candidate_body_fixture() -> AdmissionCandidateV1 {
    let sponsor_credential =
        MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0xb1; 32]);
    let joiner_credential =
        MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0xb2; 32]);
    let joiner_device = DeviceId::new("candidate-joiner");
    let admission = MembershipAdmissionV2 {
        facts: AdmissionChangeFacts {
            member_instance: joiner_credential.member_instance_id(&joiner_device),
            device_id: joiner_device,
            device_name: "candidate-joiner".to_owned(),
            identity_fingerprint: IdentityFingerprint::from_display_string("ABCD-EFGH-IJKL-MNOP")
                .expect("valid fingerprint fixture"),
            transport_public_key: vec![0xb3; 32],
            transport_address_blob: vec![0xb4; 16],
            identity_signature: vec![0xb5; 64],
        },
        membership_credential: joiner_credential,
        resume_public_key_digest: [0xb6; 32],
        security_commitment_id: [0xb7; 32],
    };
    let candidate_event = MembershipEventV2::new(
        MEMBERSHIP_EVENT_FORMAT_V2,
        "lineage".to_owned(),
        None,
        0,
        [0xb8; 16],
        MemberInstanceId::from_bytes([0xb9; 32]),
        sponsor_credential.credential_id,
        ED25519_SIGNATURE_ALGORITHM_V1,
        MembershipOperationV2::AddDevice { admission },
        [0xba; 32],
        [0xbb; 32],
        vec![0xbc],
        Some([0xbd; 32]),
        vec![0xbe; 64],
    );
    let security_commitment = AdmissionSecurityCommitmentV1::new(
        ADMISSION_SECURITY_COMMITMENT_FORMAT_V1,
        "lineage".to_owned(),
        vec![0xc0; 16],
        [0xc1; 32],
        BaseMembershipHistoryPosition {
            event_id: None,
            depth: 0,
            history_digest: [0xbf; 32],
        },
        [0xc2; 32],
        1,
        0,
        1,
        [0xc3; 32],
        [0xc4; 32],
        [0xc5; 32],
        [0xc6; 32],
        [0xc7; 32],
    )
    .expect("valid security commitment fixture");
    AdmissionCandidateV1::new(
        AdmissionSignedMembershipHistory::from_bytes(vec![0xc8; 64])
            .expect("bounded history fixture"),
        candidate_event,
        security_commitment,
        AdmissionMlsCommit::from_bytes(vec![0xc9; 64]).expect("bounded MLS commit fixture"),
        AdmissionMlsWelcome::from_bytes(vec![0xca; 64]).expect("bounded MLS welcome fixture"),
        AdmissionContinuationRoute::from_bytes(vec![0xcb; 32])
            .expect("bounded continuation route fixture"),
    )
    .expect("AddDevice candidate fixture")
}

fn prepared_proof(
    admission_id: SpaceAdmissionId,
    candidate: &AdmissionCandidateV1,
) -> PreparedAdmissionProofV1 {
    PreparedAdmissionProofV1::new(
        *admission_id.as_bytes(),
        "lineage".to_owned(),
        BaseMembershipHistoryPosition {
            event_id: None,
            depth: 0,
            history_digest: [0xcc; 32],
        },
        candidate.candidate_event().event_id(),
        candidate.candidate_event().resulting_members_digest,
        candidate.security_commitment().security_commitment_id,
        MemberInstanceId::from_bytes([0xcd; 32]),
        MembershipCredential::new(1, vec![0xce; 32]).credential_id,
        vec![0xcf; 64],
    )
}

fn round_trip_joiner(original: JoinerAdmission) -> JoinerAdmission {
    let encoded = original
        .encode_persisted()
        .expect("tracer admission state should encode");
    let decoded = JoinerAdmission::decode_persisted(&encoded)
        .expect("encoded tracer admission state should decode");
    assert_eq!(decoded, original);
    decoded
}

fn assert_joiner_private_state_absent(joiner: &JoinerAdmission) {
    let encoded = joiner
        .encode_persisted()
        .expect("Joiner state should encode");

    assert!(!encoded.windows(64).any(|window| window == [0x80; 64]));
}

fn round_trip_sponsor(original: SponsorAdmission) -> SponsorAdmission {
    let encoded = original
        .encode_persisted()
        .expect("tracer admission state should encode");
    let decoded = SponsorAdmission::decode_persisted(&encoded)
        .expect("encoded tracer admission state should decode");
    assert_eq!(decoded, original);
    decoded
}

fn admission_id() -> SpaceAdmissionId {
    SpaceAdmissionId::from_bytes([0x81; 32]).expect("non-zero admission id fixture")
}

fn join_request_message_id() -> AdmissionMessageId {
    AdmissionMessageId::from_bytes([0x8b; 32]).expect("non-zero JoinRequest id fixture")
}

fn candidate_message_id() -> AdmissionMessageId {
    AdmissionMessageId::from_bytes([0xa5; 32]).expect("non-zero Candidate id fixture")
}

fn peer_binding() -> AdmissionPeerBinding {
    AdmissionPeerBinding::new(
        AdmissionChannelPeerId::from_bytes([0x8c; 32]).expect("non-zero local peer fixture"),
        AdmissionChannelPeerId::from_bytes([0x8d; 32]).expect("non-zero remote peer fixture"),
    )
    .expect("distinct peer binding fixture")
}

fn continuation_credential() -> AdmissionContinuationCredential {
    AdmissionContinuationCredential::from_bytes(vec![0x8e; 64])
        .expect("bounded continuation credential fixture")
}
