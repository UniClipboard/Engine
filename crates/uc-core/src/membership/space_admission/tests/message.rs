use super::*;

#[test]
fn protocol_version_accepts_only_the_single_current_version() {
    let version = SpaceAdmissionProtocolVersion::from_u16(1)
        .expect("version 1 is the only supported protocol");

    assert_eq!(version.as_u16(), 1);
    assert!(SpaceAdmissionProtocolVersion::from_u16(0).is_none());
    assert!(SpaceAdmissionProtocolVersion::from_u16(2).is_none());
}

#[test]
fn admission_roles_are_distinct() {
    assert_ne!(AdmissionRole::Joiner, AdmissionRole::Sponsor);
    assert_ne!(AdmissionRole::Sponsor, AdmissionRole::CompletionHelper);
    assert_ne!(AdmissionRole::CompletionHelper, AdmissionRole::Joiner);
}

#[test]
fn every_message_kind_accepts_only_its_protocol_sender() {
    let cases = [
        (
            SpaceAdmissionMessageKind::JoinRequest,
            &[AdmissionRole::Joiner][..],
        ),
        (
            SpaceAdmissionMessageKind::Candidate,
            &[AdmissionRole::Sponsor][..],
        ),
        (
            SpaceAdmissionMessageKind::Prepared,
            &[AdmissionRole::Joiner][..],
        ),
        (
            SpaceAdmissionMessageKind::Commit,
            &[AdmissionRole::Sponsor][..],
        ),
        (
            SpaceAdmissionMessageKind::Applied,
            &[AdmissionRole::Joiner][..],
        ),
        (
            SpaceAdmissionMessageKind::Complete,
            &[AdmissionRole::Sponsor, AdmissionRole::CompletionHelper][..],
        ),
        (
            SpaceAdmissionMessageKind::CompleteAck,
            &[AdmissionRole::Joiner][..],
        ),
        (
            SpaceAdmissionMessageKind::Settled,
            &[AdmissionRole::Sponsor, AdmissionRole::CompletionHelper][..],
        ),
        (
            SpaceAdmissionMessageKind::CancelRequested,
            &[AdmissionRole::Joiner][..],
        ),
        (
            SpaceAdmissionMessageKind::Rejected,
            &[AdmissionRole::Sponsor, AdmissionRole::CompletionHelper][..],
        ),
    ];

    for (kind, expected_senders) in cases {
        for sender in [
            AdmissionRole::Joiner,
            AdmissionRole::Sponsor,
            AdmissionRole::CompletionHelper,
        ] {
            assert_eq!(
                kind.accepts_sender(sender),
                expected_senders.contains(&sender)
            );
        }
    }
}

#[test]
fn helper_can_send_only_completion_messages() {
    assert!(SpaceAdmissionMessageKind::Complete.accepts_sender(AdmissionRole::CompletionHelper));
    assert!(SpaceAdmissionMessageKind::Settled.accepts_sender(AdmissionRole::CompletionHelper));
    assert!(SpaceAdmissionMessageKind::Rejected.accepts_sender(AdmissionRole::CompletionHelper));
    assert!(!SpaceAdmissionMessageKind::Commit.accepts_sender(AdmissionRole::CompletionHelper));
    assert!(!SpaceAdmissionMessageKind::Candidate.accepts_sender(AdmissionRole::CompletionHelper));
}

#[test]
fn initial_join_request_header_is_valid_and_redacted() {
    let admission_id =
        SpaceAdmissionId::from_bytes([0x61; 32]).expect("non-zero admission id fixture");
    let message_id =
        AdmissionMessageId::from_bytes([0x62; 32]).expect("non-zero message id fixture");

    let header = SpaceAdmissionEnvelopeHeaderV1::new(
        admission_id,
        SpaceAdmissionMessageKind::JoinRequest,
        AdmissionRole::Joiner,
        0,
        message_id,
        None,
    )
    .expect("initial JoinRequest header must be valid");

    assert_eq!(header.protocol_version(), SpaceAdmissionProtocolVersion::V1);
    assert_eq!(header.admission_id(), admission_id);
    assert_eq!(header.kind(), SpaceAdmissionMessageKind::JoinRequest);
    assert_eq!(header.sender_role(), AdmissionRole::Joiner);
    assert_eq!(header.sender_sequence(), 0);
    assert_eq!(header.message_id(), message_id);
    assert_eq!(header.predecessor_message_id(), None);
    let output = format!("{header:?}");
    assert!(!output.contains("6161"));
    assert!(!output.contains("6262"));
}

#[test]
fn invalid_join_request_headers_are_rejected() {
    let admission_id =
        SpaceAdmissionId::from_bytes([0x63; 32]).expect("non-zero admission id fixture");
    let message_id =
        AdmissionMessageId::from_bytes([0x64; 32]).expect("non-zero message id fixture");
    let predecessor =
        AdmissionMessageId::from_bytes([0x65; 32]).expect("non-zero message id fixture");

    assert_eq!(
        SpaceAdmissionEnvelopeHeaderV1::new(
            admission_id,
            SpaceAdmissionMessageKind::JoinRequest,
            AdmissionRole::Sponsor,
            0,
            message_id,
            None,
        ),
        Err(AdmissionMessageHeaderError::SenderNotAllowed)
    );
    assert_eq!(
        SpaceAdmissionEnvelopeHeaderV1::new(
            admission_id,
            SpaceAdmissionMessageKind::JoinRequest,
            AdmissionRole::Joiner,
            1,
            message_id,
            None,
        ),
        Err(AdmissionMessageHeaderError::InvalidInitialJoinRequest)
    );
    assert_eq!(
        SpaceAdmissionEnvelopeHeaderV1::new(
            admission_id,
            SpaceAdmissionMessageKind::JoinRequest,
            AdmissionRole::Joiner,
            0,
            message_id,
            Some(predecessor),
        ),
        Err(AdmissionMessageHeaderError::InvalidInitialJoinRequest)
    );
}

#[test]
fn every_non_initial_message_requires_a_predecessor() {
    let admission_id =
        SpaceAdmissionId::from_bytes([0x66; 32]).expect("non-zero admission id fixture");
    let message_id =
        AdmissionMessageId::from_bytes([0x67; 32]).expect("non-zero message id fixture");

    assert_eq!(
        SpaceAdmissionEnvelopeHeaderV1::new(
            admission_id,
            SpaceAdmissionMessageKind::Candidate,
            AdmissionRole::Sponsor,
            0,
            message_id,
            None,
        ),
        Err(AdmissionMessageHeaderError::MissingPredecessor)
    );
}

#[test]
fn a_reply_can_start_its_sender_sequence_at_zero() {
    let admission_id =
        SpaceAdmissionId::from_bytes([0x68; 32]).expect("non-zero admission id fixture");
    let message_id =
        AdmissionMessageId::from_bytes([0x69; 32]).expect("non-zero message id fixture");
    let predecessor =
        AdmissionMessageId::from_bytes([0x6a; 32]).expect("non-zero message id fixture");

    let header = SpaceAdmissionEnvelopeHeaderV1::new(
        admission_id,
        SpaceAdmissionMessageKind::Candidate,
        AdmissionRole::Sponsor,
        0,
        message_id,
        Some(predecessor),
    )
    .expect("a Sponsor's first reply starts its own sequence at zero");

    assert_eq!(header.sender_sequence(), 0);
    assert_eq!(header.predecessor_message_id(), Some(predecessor));
}

#[test]
fn typed_envelope_derives_its_kind_and_durable_evidence() {
    let admission_id =
        SpaceAdmissionId::from_bytes([0x71; 32]).expect("non-zero admission id fixture");
    let message_id =
        AdmissionMessageId::from_bytes([0x72; 32]).expect("non-zero message id fixture");
    let predecessor =
        AdmissionMessageId::from_bytes([0x73; 32]).expect("non-zero message id fixture");
    let envelope = SpaceAdmissionEnvelopeV1::new(
        admission_id,
        AdmissionRole::Joiner,
        2,
        message_id,
        Some(predecessor),
        SpaceAdmissionBodyV1::CancelRequested,
    )
    .expect("a typed cancellation envelope must be valid");

    assert_eq!(envelope.kind(), SpaceAdmissionMessageKind::CancelRequested);
    assert!(matches!(
        envelope.body(),
        SpaceAdmissionBodyV1::CancelRequested
    ));
    let evidence = envelope
        .evidence([0x74; 32])
        .expect("non-zero canonical digest fixture");
    assert_eq!(evidence.sender_role(), AdmissionRole::Joiner);
    assert_eq!(evidence.sender_sequence(), 2);
    assert_eq!(evidence.message_id(), message_id);
    assert_eq!(evidence.predecessor_message_id(), Some(predecessor));
    assert_eq!(evidence.canonical_digest(), &[0x74; 32]);
}

#[test]
fn typed_envelope_rejects_a_sender_not_allowed_by_its_body() {
    let admission_id =
        SpaceAdmissionId::from_bytes([0x75; 32]).expect("non-zero admission id fixture");
    let message_id =
        AdmissionMessageId::from_bytes([0x76; 32]).expect("non-zero message id fixture");
    let predecessor =
        AdmissionMessageId::from_bytes([0x77; 32]).expect("non-zero message id fixture");

    assert_eq!(
        SpaceAdmissionEnvelopeV1::new(
            admission_id,
            AdmissionRole::Sponsor,
            1,
            message_id,
            Some(predecessor),
            SpaceAdmissionBodyV1::CompleteAck(
                AdmissionCompleteAckV1::new([0x78; 32])
                    .expect("non-zero completion digest fixture"),
            ),
        ),
        Err(AdmissionProtocolMessageError::SenderNotAllowed)
    );
}

#[test]
fn typed_envelope_debug_never_prints_message_content_or_ids() {
    let envelope = SpaceAdmissionEnvelopeV1::new(
        SpaceAdmissionId::from_bytes([0x79; 32]).expect("non-zero admission id fixture"),
        AdmissionRole::Sponsor,
        3,
        AdmissionMessageId::from_bytes([0x7a; 32]).expect("non-zero message id fixture"),
        Some(AdmissionMessageId::from_bytes([0x7b; 32]).expect("non-zero message id fixture")),
        SpaceAdmissionBodyV1::Rejected {
            reason: super::SpaceAdmissionRejectionReason::HistoryConflict,
        },
    )
    .expect("a typed rejection envelope must be valid");
    let output = format!("{envelope:?}");

    assert!(output.contains("Rejected"));
    assert!(!output.contains("7979"));
    assert!(!output.contains("7a7a"));
    assert!(!output.contains("HistoryConflict"));
}

#[test]
fn admission_artifacts_reject_empty_and_oversized_bytes() {
    assert_eq!(
        AdmissionKeyPackage::from_bytes(Vec::new()),
        Err(AdmissionArtifactError::Empty)
    );
    assert_eq!(
        AdmissionKeyPackage::from_bytes(vec![0x81; 1024 * 1024 + 1]),
        Err(AdmissionArtifactError::Oversized)
    );
    let package = AdmissionKeyPackage::from_bytes(vec![0x82; 32])
        .expect("bounded non-empty key package fixture");
    assert_eq!(package.as_bytes(), &[0x82; 32]);
    assert!(!format!("{package:?}").contains("8282"));
}

#[test]
fn fixed_size_recovery_and_completion_values_reject_zero() {
    assert!(AdmissionRecoveryPublicKey::from_bytes([0; 32]).is_none());
    assert!(AdmissionCompleteAckV1::new([0; 32]).is_none());
}

#[test]
fn join_request_body_requires_complete_typed_material() {
    let request = AdmissionJoinRequestV1::new(
        InvitationId::from_bytes([0x83; 32]).expect("non-zero invitation id fixture"),
        DeviceId::new("joining-device"),
        MembershipCredential::new(1, vec![0x84; 32]),
        AdmissionKeyPackage::from_bytes(vec![0x85; 48])
            .expect("bounded non-empty key package fixture"),
        AdmissionRecoveryPublicKey::from_bytes([0x86; 32])
            .expect("non-zero recovery public key fixture"),
        AdmissionIdentitySignature::from_bytes(vec![0x87; 64])
            .expect("bounded non-empty signature fixture"),
        UnreadableHistoryPolicy::Preserve,
    )
    .expect("complete valid JoinRequest fixture");
    let body = SpaceAdmissionBodyV1::JoinRequest(request);

    assert_eq!(body.kind(), SpaceAdmissionMessageKind::JoinRequest);
    assert!(!format!("{body:?}").contains("joining-device"));
    assert!(!format!("{body:?}").contains("8787"));
}

#[test]
fn every_typed_business_body_has_one_stable_kind() {
    let candidate = SpaceAdmissionBodyV1::Candidate(candidate_body_fixture());
    let prepared =
        SpaceAdmissionBodyV1::Prepared(AdmissionPreparedV1::new(PreparedAdmissionProofV1::new(
            [0xcc; 32],
            "lineage".to_owned(),
            BaseMembershipHistoryPosition {
                event_id: None,
                depth: 0,
                history_digest: [0xcd; 32],
            },
            candidate_body_fixture().candidate_event().event_id(),
            [0xce; 32],
            [0xcf; 32],
            MemberInstanceId::from_bytes([0xd0; 32]),
            MembershipCredential::new(1, vec![0xd1; 32]).credential_id,
            vec![0xd2; 64],
        )));
    let commit = SpaceAdmissionBodyV1::Commit(AdmissionCommitV1::new(
        candidate_body_fixture(),
        AdmissionSignedMembershipHistory::from_bytes(vec![0xd3; 64])
            .expect("bounded target history fixture"),
        AdmissionSealedRecoveryMaterial::from_bytes(vec![0xd4; 64])
            .expect("bounded recovery material fixture"),
    ));
    let receipt = AdmissionActivationReceipt::new(
        1,
        [0xd5; 32],
        candidate_body_fixture().candidate_event().event_id(),
        [0xd6; 32],
        [0xd7; 32],
        MemberInstanceId::from_bytes([0xd8; 32]),
        vec![0xd9; 64],
    );
    let applied = SpaceAdmissionBodyV1::Applied(AdmissionAppliedV1::new(receipt));
    let completion = AdmissionCompletionV1::new(
        [0xda; 32],
        candidate_body_fixture().candidate_event().event_id(),
        [0xdb; 32],
        [0xdc; 32],
        MemberInstanceId::from_bytes([0xdd; 32]),
        MembershipCredential::new(1, vec![0xde; 32]).credential_id,
        BaseMembershipHistoryPosition {
            event_id: None,
            depth: 1,
            history_digest: [0xdf; 32],
        },
        vec![0xe0; 64],
    );
    let complete = SpaceAdmissionBodyV1::Complete(AdmissionCompleteV1::new(completion));
    let settled = SpaceAdmissionBodyV1::Settled(
        AdmissionSettledV1::new([0xe1; 32]).expect("non-zero completion ack digest fixture"),
    );

    for (body, kind) in [
        (candidate, SpaceAdmissionMessageKind::Candidate),
        (prepared, SpaceAdmissionMessageKind::Prepared),
        (commit, SpaceAdmissionMessageKind::Commit),
        (applied, SpaceAdmissionMessageKind::Applied),
        (complete, SpaceAdmissionMessageKind::Complete),
        (settled, SpaceAdmissionMessageKind::Settled),
    ] {
        assert_eq!(body.kind(), kind);
        assert!(!format!("{body:?}").contains("lineage"));
    }
}
