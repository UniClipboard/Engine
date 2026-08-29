fn join_request_envelope_fixture(
    admission_id: SpaceAdmissionId,
    message_id: AdmissionMessageId,
) -> SpaceAdmissionEnvelopeV1 {
    let device_id = DeviceId::new("joining-device");
    let credential = MembershipCredential::new(1, vec![0xe3; 32]);
    let signature = vec![0xe6; 64];
    let request = AdmissionJoinRequestV1::new(
        InvitationId::from_bytes([0xe2; 32]).expect("non-zero invitation id fixture"),
        device_id.clone(),
        join_request_identity_facts(device_id, &credential, signature.clone()),
        credential,
        AdmissionKeyPackage::from_bytes(vec![0xe4; 48]).expect("bounded key package fixture"),
        AdmissionRecoveryPublicKey::from_bytes([0xe5; 32])
            .expect("non-zero recovery public key fixture"),
        AdmissionIdentitySignature::from_bytes(signature).expect("bounded signature fixture"),
        UnreadableHistoryPolicy::Discard,
    )
    .expect("complete JoinRequest fixture");
    SpaceAdmissionEnvelopeV1::new(
        admission_id,
        AdmissionRole::Joiner,
        0,
        message_id,
        None,
        SpaceAdmissionBodyV1::JoinRequest(request),
    )
    .expect("valid initial JoinRequest envelope fixture")
}

#[test]
fn canonical_transport_envelope_round_trips_the_complete_join_request() {
    let admission_id =
        SpaceAdmissionId::from_bytes([0xd1; 32]).expect("non-zero admission id fixture");
    let original = join_request_envelope_fixture(
        admission_id,
        AdmissionMessageId::from_bytes([0xd2; 32]).expect("non-zero message id fixture"),
    );

    let encoded = original
        .encode_canonical_v1()
        .expect("typed envelope should encode");
    let decoded = SpaceAdmissionEnvelopeV1::decode_canonical_v1(&encoded)
        .expect("canonical envelope should decode");

    assert_eq!(decoded, original);
}

#[test]
fn new_joiner_aggregate_starts_with_all_required_durable_material() {
    let admission_id =
        SpaceAdmissionId::from_bytes([0xe7; 32]).expect("non-zero admission id fixture");
    let join_id = JoinId::from_bytes([0xe8; 16]).expect("non-zero join id fixture");
    let request = join_request_envelope_fixture(
        admission_id,
        AdmissionMessageId::from_bytes([0xe9; 32]).expect("non-zero message id fixture"),
    );
    let exchange = PendingAdmissionExchange::new(
        SpaceAdmissionRoute::from_bytes(vec![0xea; 32]).expect("bounded route fixture"),
        request,
        SpaceAdmissionMessageKind::Candidate,
        AdmissionRetryState::new(0, 0).expect("valid initial retry state"),
    )
    .expect("JoinRequest expects Candidate");
    let aggregate = SpaceAdmissionAggregate::start_join(
        admission_id,
        join_id,
        7,
        AdmissionSourceSnapshot::from_bytes(vec![0xeb; 64])
            .expect("bounded source snapshot fixture"),
        AdmissionJoinerPrivateState::from_bytes(vec![0xed; 64])
            .expect("bounded Joiner private state fixture"),
        AdmissionEncryptedPasswordEquivalent::from_bytes(vec![0xec; 64])
            .expect("bounded encrypted password fixture"),
        exchange,
    )
    .expect("complete initial joiner state");

    assert_eq!(aggregate.record_version(), 0);
    assert_eq!(aggregate.admission_id(), admission_id);
    let state = match aggregate.state() {
        SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Initiated(state)) => state,
        _ => panic!("new joiner must start in Initiated"),
    };
    assert_eq!(state.join_id(), join_id);
    assert_eq!(state.local_join_ordinal(), 7);
    assert_eq!(
        state.pending_exchange().request_envelope().kind(),
        SpaceAdmissionMessageKind::JoinRequest
    );
    let output = format!("{aggregate:?}");
    assert!(!output.contains("e7e7"));
    assert!(!output.contains("ecec"));
}

#[test]
fn new_joiner_aggregate_rejects_an_exchange_for_another_admission() {
    let aggregate_admission =
        SpaceAdmissionId::from_bytes([0xed; 32]).expect("non-zero admission id fixture");
    let request_admission =
        SpaceAdmissionId::from_bytes([0xee; 32]).expect("non-zero admission id fixture");
    let request = join_request_envelope_fixture(
        request_admission,
        AdmissionMessageId::from_bytes([0xef; 32]).expect("non-zero message id fixture"),
    );
    let exchange = PendingAdmissionExchange::new(
        SpaceAdmissionRoute::from_bytes(vec![0xf0; 32]).expect("bounded route fixture"),
        request,
        SpaceAdmissionMessageKind::Candidate,
        AdmissionRetryState::new(0, 0).expect("valid initial retry state"),
    )
    .expect("JoinRequest expects Candidate");

    assert_eq!(
        SpaceAdmissionAggregate::start_join(
            aggregate_admission,
            JoinId::from_bytes([0xf1; 16]).expect("non-zero join id fixture"),
            1,
            AdmissionSourceSnapshot::from_bytes(vec![0xf2])
                .expect("bounded source snapshot fixture"),
            AdmissionJoinerPrivateState::from_bytes(vec![0xf4])
                .expect("bounded Joiner private state fixture"),
            AdmissionEncryptedPasswordEquivalent::from_bytes(vec![0xf3])
                .expect("bounded encrypted password fixture"),
            exchange,
        ),
        Err(SpaceAdmissionAggregateError::AdmissionMismatch)
    );
}

fn initiated_joiner_aggregate_fixture() -> SpaceAdmissionAggregate {
    let admission_id =
        SpaceAdmissionId::from_bytes([0xf4; 32]).expect("non-zero admission id fixture");
    let request = join_request_envelope_fixture(
        admission_id,
        AdmissionMessageId::from_bytes([0xf5; 32]).expect("non-zero message id fixture"),
    );
    SpaceAdmissionAggregate::start_join(
        admission_id,
        JoinId::from_bytes([0xf6; 16]).expect("non-zero join id fixture"),
        2,
        AdmissionSourceSnapshot::from_bytes(vec![0xf7; 32])
            .expect("bounded source snapshot fixture"),
        AdmissionJoinerPrivateState::from_bytes(vec![0xfa; 32])
            .expect("bounded Joiner private state fixture"),
        AdmissionEncryptedPasswordEquivalent::from_bytes(vec![0xf8; 32])
            .expect("bounded encrypted password fixture"),
        PendingAdmissionExchange::new(
            SpaceAdmissionRoute::from_bytes(vec![0xf9; 32]).expect("bounded route fixture"),
            request,
            SpaceAdmissionMessageKind::Candidate,
            AdmissionRetryState::new(0, 0).expect("valid initial retry state"),
        )
        .expect("JoinRequest expects Candidate"),
    )
    .expect("complete initial joiner fixture")
    .into_replacement()
}

fn joiner_candidate_aggregate_fixture() -> SpaceAdmissionAggregate {
    let authenticated = initiated_joiner_aggregate_fixture()
        .with_authenticated_channel(
            AdmissionPeerBinding::new(
                AdmissionChannelPeerId::from_bytes([0x27; 32])
                    .expect("non-zero local peer fixture"),
                AdmissionChannelPeerId::from_bytes([0x28; 32])
                    .expect("non-zero remote peer fixture"),
            )
            .expect("distinct peer binding fixture"),
            AdmissionContinuationCredential::from_bytes(vec![0x29; 64])
                .expect("bounded continuation credential fixture"),
        )
        .expect("authenticated Joiner fixture")
        .into_replacement();

    let join_request_id = match authenticated.state() {
        SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Initiated(state)) => state
            .pending_exchange()
            .request_envelope()
            .header()
            .message_id(),
        _ => panic!("fixture must be Initiated Joiner"),
    };

    let candidate = SpaceAdmissionEnvelopeV1::new(
        authenticated.admission_id(),
        AdmissionRole::Sponsor,
        0,
        AdmissionMessageId::from_bytes([0x2a; 32]).expect("non-zero message id fixture"),
        Some(join_request_id),
        SpaceAdmissionBodyV1::Candidate(candidate_body_fixture()),
    )
    .expect("valid Candidate fixture");

    authenticated
        .accept_candidate(
            candidate,
            [0x2b; 32],
            AdmissionStagedTargetInput::from_bytes(vec![0x2c; 128])
                .expect("bounded staged target input fixture"),
        )
        .expect("Joiner Candidate fixture")
        .into_replacement()
}

fn sponsor_candidate_aggregate_fixture() -> SpaceAdmissionAggregate {
    let admission_id =
        SpaceAdmissionId::from_bytes([0x41; 32]).expect("non-zero admission id fixture");
    let join_request = join_request_envelope_fixture(
        admission_id,
        AdmissionMessageId::from_bytes([0x42; 32]).expect("non-zero message id fixture"),
    );
    let join_request_evidence = join_request
        .evidence([0x43; 32])
        .expect("non-zero request digest fixture");
    let sponsor = SpaceAdmissionAggregate::accept_join_request(
        admission_id,
        AdmissionInvitationClaim::from_bytes(vec![0x44; 32])
            .expect("bounded invitation claim fixture"),
        join_request,
        join_request_evidence,
        AdmissionBaseSnapshot::from_bytes(vec![0x45; 64]).expect("bounded base snapshot fixture"),
        AdmissionPeerBinding::new(
            AdmissionChannelPeerId::from_bytes([0x46; 32]).expect("non-zero local peer fixture"),
            AdmissionChannelPeerId::from_bytes([0x47; 32]).expect("non-zero remote peer fixture"),
        )
        .expect("distinct peer binding fixture"),
        AdmissionContinuationCredential::from_bytes(vec![0x48; 64])
            .expect("bounded continuation credential fixture"),
    )
    .expect("complete accepted Sponsor fixture")
    .into_replacement();
    let candidate_reply = SpaceAdmissionEnvelopeV1::new(
        admission_id,
        AdmissionRole::Sponsor,
        0,
        AdmissionMessageId::from_bytes([0x49; 32]).expect("non-zero message id fixture"),
        Some(AdmissionMessageId::from_bytes([0x42; 32]).expect("non-zero predecessor fixture")),
        SpaceAdmissionBodyV1::Candidate(candidate_body_fixture()),
    )
    .expect("valid Candidate reply fixture");

    sponsor
        .fix_candidate(
            candidate_reply,
            AdmissionStagedSecurityState::from_bytes(vec![0x4a; 128])
                .expect("bounded staged security fixture"),
        )
        .expect("Sponsor Candidate fixture")
        .into_replacement()
}

fn sponsor_committed_aggregate_fixture() -> SpaceAdmissionAggregate {
    let sponsor = sponsor_candidate_aggregate_fixture();
    let candidate_message_id = match sponsor.state() {
        SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Candidate(state)) => state
            .saved_reply()
            .exact_reply_envelope()
            .header()
            .message_id(),
        _ => panic!("fixture must be Candidate Sponsor"),
    };
    let prepared_message_id =
        AdmissionMessageId::from_bytes([0x6a; 32]).expect("non-zero message id fixture");
    let prepared = SpaceAdmissionEnvelopeV1::new(
        sponsor.admission_id(),
        AdmissionRole::Joiner,
        1,
        prepared_message_id,
        Some(candidate_message_id),
        SpaceAdmissionBodyV1::Prepared(AdmissionPreparedV1::new(PreparedAdmissionProofV1::new(
            *sponsor.admission_id().as_bytes(),
            "lineage".to_owned(),
            BaseMembershipHistoryPosition {
                event_id: None,
                depth: 0,
                history_digest: [0x6b; 32],
            },
            candidate_body_fixture().candidate_event().event_id(),
            [0x6c; 32],
            [0x6d; 32],
            MemberInstanceId::from_bytes([0x6e; 32]),
            MembershipCredential::new(1, vec![0x6f; 32]).credential_id,
            vec![0x70; 64],
        ))),
    )
    .expect("valid Prepared fixture");
    let committed_history_bytes = vec![0x71; 128];
    let commit_reply = SpaceAdmissionEnvelopeV1::new(
        sponsor.admission_id(),
        AdmissionRole::Sponsor,
        1,
        AdmissionMessageId::from_bytes([0x72; 32]).expect("non-zero message id fixture"),
        Some(prepared_message_id),
        SpaceAdmissionBodyV1::Commit(AdmissionCommitV1::new(
            candidate_body_fixture(),
            AdmissionSignedMembershipHistory::from_bytes(committed_history_bytes.clone())
                .expect("bounded committed history fixture"),
            AdmissionSealedRecoveryMaterial::from_bytes(vec![0x73; 128])
                .expect("bounded sealed recovery fixture"),
        )),
    )
    .expect("valid Commit reply fixture");

    sponsor
        .commit_prepared(
            prepared,
            [0x74; 32],
            AdmissionSignedMembershipHistory::from_bytes(committed_history_bytes)
                .expect("bounded committed history fixture"),
            crate::membership::AdmissionSealedSecurityState::from_bytes(vec![0x75; 128])
                .expect("bounded sealed security fixture"),
            commit_reply,
        )
        .expect("Committed Sponsor fixture")
        .into_replacement()
}

fn sponsor_applied_aggregate_fixture() -> SpaceAdmissionAggregate {
    let sponsor = sponsor_committed_aggregate_fixture();
    let (commit_message_id, event_id, security_commitment_id) = match sponsor.state() {
        SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Committed(state)) => {
            let SpaceAdmissionBodyV1::Commit(commit) =
                state.saved_reply().exact_reply_envelope().body()
            else {
                panic!("Committed Sponsor must hold Commit reply");
            };
            (
                state
                    .saved_reply()
                    .exact_reply_envelope()
                    .header()
                    .message_id(),
                commit.exact_candidate().candidate_event().event_id(),
                commit
                    .exact_candidate()
                    .security_commitment()
                    .security_commitment_id,
            )
        }
        _ => panic!("fixture must be Committed Sponsor"),
    };
    let applied_message_id =
        AdmissionMessageId::from_bytes([0x9b; 32]).expect("non-zero message id fixture");
    let receipt = AdmissionActivationReceipt::new(
        1,
        *sponsor.admission_id().as_bytes(),
        event_id,
        [0x9c; 32],
        security_commitment_id,
        MemberInstanceId::from_bytes([0x9d; 32]),
        vec![0x9e; 64],
    );
    let applied = SpaceAdmissionEnvelopeV1::new(
        sponsor.admission_id(),
        AdmissionRole::Joiner,
        2,
        applied_message_id,
        Some(commit_message_id),
        SpaceAdmissionBodyV1::Applied(AdmissionAppliedV1::new(receipt)),
    )
    .expect("valid Applied fixture");
    let completion = AdmissionCompletionV1::new(
        *sponsor.admission_id().as_bytes(),
        event_id,
        [0x9f; 32],
        security_commitment_id,
        MemberInstanceId::from_bytes([0xa0; 32]),
        MembershipCredential::new(1, vec![0xa1; 32]).credential_id,
        BaseMembershipHistoryPosition {
            event_id: Some(event_id),
            depth: 1,
            history_digest: [0xa2; 32],
        },
        vec![0xa3; 64],
    );
    let complete_reply = SpaceAdmissionEnvelopeV1::new(
        sponsor.admission_id(),
        AdmissionRole::Sponsor,
        2,
        AdmissionMessageId::from_bytes([0xa4; 32]).expect("non-zero message id fixture"),
        Some(applied_message_id),
        SpaceAdmissionBodyV1::Complete(AdmissionCompleteV1::new(completion)),
    )
    .expect("valid Complete reply fixture");

    sponsor
        .complete_applied(
            applied,
            [0xa5; 32],
            crate::membership::AdmissionActivatedSecurityState::from_bytes(vec![0xa6; 128])
                .expect("bounded activated security fixture"),
            complete_reply,
        )
        .expect("Applied Sponsor fixture")
        .into_replacement()
}

fn joiner_prepared_aggregate_fixture() -> SpaceAdmissionAggregate {
    let candidate = joiner_candidate_aggregate_fixture();
    let candidate_message_id = match candidate.state() {
        SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Candidate(state)) => {
            state.candidate().header().message_id()
        }
        _ => panic!("fixture must be Candidate Joiner"),
    };
    let candidate_material = candidate_body_fixture();
    let prepared_request = SpaceAdmissionEnvelopeV1::new(
        candidate.admission_id(),
        AdmissionRole::Joiner,
        1,
        AdmissionMessageId::from_bytes([0x57; 32]).expect("non-zero message id fixture"),
        Some(candidate_message_id),
        SpaceAdmissionBodyV1::Prepared(AdmissionPreparedV1::new(PreparedAdmissionProofV1::new(
            *candidate.admission_id().as_bytes(),
            "lineage".to_owned(),
            BaseMembershipHistoryPosition {
                event_id: None,
                depth: 0,
                history_digest: [0x58; 32],
            },
            candidate_material.candidate_event().event_id(),
            candidate_material
                .candidate_event()
                .resulting_members_digest,
            candidate_material
                .security_commitment()
                .security_commitment_id,
            MemberInstanceId::from_bytes([0x59; 32]),
            MembershipCredential::new(1, vec![0x5a; 32]).credential_id,
            vec![0x5b; 64],
        ))),
    )
    .expect("valid Prepared request fixture");
    let pending_exchange = PendingAdmissionExchange::new(
        SpaceAdmissionRoute::from_bytes(vec![0x5c; 32]).expect("bounded route fixture"),
        prepared_request,
        SpaceAdmissionMessageKind::Commit,
        AdmissionRetryState::new(0, 0).expect("valid retry state"),
    )
    .expect("Prepared expects Commit");

    candidate
        .prepare_candidate(
            AdmissionSignedMembershipHistory::from_bytes(vec![0x5d; 128])
                .expect("bounded verified history fixture"),
            crate::membership::AdmissionStagedTarget::from_bytes(vec![0x5e; 128])
                .expect("bounded staged target fixture"),
            pending_exchange,
        )
        .expect("Prepared Joiner fixture")
        .into_replacement()
}

fn joiner_committed_aggregate_fixture() -> SpaceAdmissionAggregate {
    let prepared = joiner_prepared_aggregate_fixture();
    let prepared_message_id = match prepared.state() {
        SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Prepared(state)) => state
            .pending_exchange()
            .request_envelope()
            .header()
            .message_id(),
        _ => panic!("fixture must be Prepared Joiner"),
    };
    let commit = SpaceAdmissionEnvelopeV1::new(
        prepared.admission_id(),
        AdmissionRole::Sponsor,
        1,
        AdmissionMessageId::from_bytes([0x62; 32]).expect("non-zero message id fixture"),
        Some(prepared_message_id),
        SpaceAdmissionBodyV1::Commit(AdmissionCommitV1::new(
            candidate_body_fixture(),
            AdmissionSignedMembershipHistory::from_bytes(vec![0x5d; 128])
                .expect("bounded committed history fixture"),
            AdmissionSealedRecoveryMaterial::from_bytes(vec![0x63; 128])
                .expect("bounded sealed recovery fixture"),
        )),
    )
    .expect("valid Commit fixture");

    prepared
        .accept_commit(commit, [0x64; 32])
        .expect("Committed Joiner fixture")
        .into_replacement()
}

fn joiner_applied_aggregate_fixture() -> SpaceAdmissionAggregate {
    let committed = joiner_committed_aggregate_fixture();
    let (commit_message_id, event_id, security_commitment_id) = match committed.state() {
        SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Committed(state)) => {
            let SpaceAdmissionBodyV1::Commit(commit) = state.exact_commit().body() else {
                panic!("Committed Joiner must hold Commit");
            };
            (
                state.exact_commit().header().message_id(),
                commit.exact_candidate().candidate_event().event_id(),
                commit
                    .exact_candidate()
                    .security_commitment()
                    .security_commitment_id,
            )
        }
        _ => panic!("fixture must be Committed Joiner"),
    };
    let receipt = AdmissionActivationReceipt::new(
        1,
        *committed.admission_id().as_bytes(),
        event_id,
        [0x82; 32],
        security_commitment_id,
        MemberInstanceId::from_bytes([0x83; 32]),
        vec![0x84; 64],
    );
    let applied_request = SpaceAdmissionEnvelopeV1::new(
        committed.admission_id(),
        AdmissionRole::Joiner,
        2,
        AdmissionMessageId::from_bytes([0x85; 32]).expect("non-zero message id fixture"),
        Some(commit_message_id),
        SpaceAdmissionBodyV1::Applied(AdmissionAppliedV1::new(receipt)),
    )
    .expect("valid Applied request fixture");
    committed
        .apply_commit(
            PendingAdmissionExchange::new(
                SpaceAdmissionRoute::from_bytes(vec![0x86; 32]).expect("bounded route fixture"),
                applied_request,
                SpaceAdmissionMessageKind::Complete,
                AdmissionRetryState::new(0, 0).expect("valid retry state"),
            )
            .expect("Applied expects Complete"),
        )
        .expect("Applied Joiner fixture")
        .into_replacement()
}
