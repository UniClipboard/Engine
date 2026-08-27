#[test]
fn authentication_replaces_password_material_with_bound_continuation() {
    let aggregate = initiated_joiner_aggregate_fixture();
    let local_peer = AdmissionChannelPeerId::from_bytes([0xfa; 32])
        .expect("non-zero local channel peer fixture");
    let remote_peer = AdmissionChannelPeerId::from_bytes([0xfb; 32])
        .expect("non-zero remote channel peer fixture");
    let authenticated = aggregate
        .with_authenticated_channel(
            AdmissionPeerBinding::new(local_peer, remote_peer)
                .expect("distinct channel peers fixture"),
            AdmissionContinuationCredential::from_bytes(vec![0xfc; 64])
                .expect("bounded continuation credential fixture"),
        )
        .expect("initial authentication transition");

    assert_eq!(authenticated.record_version(), 1);
    let state = match authenticated.state() {
        SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Initiated(state)) => state,
        _ => panic!("authenticated joiner must remain Initiated"),
    };
    let SpaceAdmissionJoinerChannelState::Authenticated {
        peer_binding,
        continuation_credential,
    } = state.channel_state()
    else {
        panic!("password material must be replaced after authentication");
    };
    assert_eq!(peer_binding.local_peer_id(), local_peer);
    assert_eq!(peer_binding.remote_peer_id(), remote_peer);
    assert_eq!(continuation_credential.as_bytes(), &[0xfc; 64]);
    assert_eq!(
        state.pending_exchange().request_envelope().kind(),
        SpaceAdmissionMessageKind::JoinRequest
    );
}

#[test]
fn peer_binding_rejects_the_same_identity_on_both_sides() {
    let peer =
        AdmissionChannelPeerId::from_bytes([0xfd; 32]).expect("non-zero channel peer fixture");
    assert!(AdmissionPeerBinding::new(peer, peer).is_none());
}

#[test]
fn sponsor_accepts_join_request_then_saves_one_exact_candidate_reply() {
    let admission_id =
        SpaceAdmissionId::from_bytes([0x11; 32]).expect("non-zero admission id fixture");
    let join_request = join_request_envelope_fixture(
        admission_id,
        AdmissionMessageId::from_bytes([0x12; 32]).expect("non-zero message id fixture"),
    );
    let join_request_evidence = join_request
        .evidence([0x13; 32])
        .expect("non-zero request digest fixture");
    let binding = AdmissionPeerBinding::new(
        AdmissionChannelPeerId::from_bytes([0x14; 32]).expect("non-zero local peer fixture"),
        AdmissionChannelPeerId::from_bytes([0x15; 32]).expect("non-zero remote peer fixture"),
    )
    .expect("distinct peer binding fixture");
    let accepted = SpaceAdmissionAggregate::accept_join_request(
        admission_id,
        AdmissionInvitationClaim::from_bytes(vec![0x16; 32])
            .expect("bounded invitation claim fixture"),
        join_request,
        join_request_evidence,
        AdmissionBaseSnapshot::from_bytes(vec![0x17; 64]).expect("bounded base snapshot fixture"),
        binding,
        AdmissionContinuationCredential::from_bytes(vec![0x18; 64])
            .expect("bounded continuation credential fixture"),
    )
    .expect("complete accepted Sponsor state");
    assert_eq!(accepted.effects(), &[AdmissionEffect::ConsumeInvitation]);
    let sponsor = accepted.into_replacement();

    let candidate_reply = SpaceAdmissionEnvelopeV1::new(
        admission_id,
        AdmissionRole::Sponsor,
        0,
        AdmissionMessageId::from_bytes([0x19; 32]).expect("non-zero message id fixture"),
        Some(AdmissionMessageId::from_bytes([0x12; 32]).expect("non-zero predecessor fixture")),
        SpaceAdmissionBodyV1::Candidate(candidate_body_fixture()),
    )
    .expect("valid Candidate reply fixture");
    let candidate = sponsor
        .fix_candidate(
            candidate_reply,
            AdmissionStagedSecurityState::from_bytes(vec![0x1a; 128])
                .expect("bounded staged security fixture"),
        )
        .expect("Accepted Sponsor can fix Candidate once");

    assert!(candidate.effects().is_empty());
    assert_eq!(candidate.record_version(), 1);
    let state = match candidate.state() {
        SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Candidate(state)) => state,
        _ => panic!("Sponsor must advance to Candidate"),
    };
    assert_eq!(
        state.saved_reply().exact_reply_envelope().kind(),
        SpaceAdmissionMessageKind::Candidate
    );
    assert_eq!(
        state.saved_reply().inbound_evidence().canonical_digest(),
        &[0x13; 32]
    );
}

#[test]
fn authenticated_joiner_accepts_only_candidate_for_saved_join_request() {
    let aggregate = initiated_joiner_aggregate_fixture()
        .with_authenticated_channel(
            AdmissionPeerBinding::new(
                AdmissionChannelPeerId::from_bytes([0x21; 32])
                    .expect("non-zero local peer fixture"),
                AdmissionChannelPeerId::from_bytes([0x22; 32])
                    .expect("non-zero remote peer fixture"),
            )
            .expect("distinct peer binding fixture"),
            AdmissionContinuationCredential::from_bytes(vec![0x23; 64])
                .expect("bounded continuation credential fixture"),
        )
        .expect("authenticated Joiner fixture")
        .into_replacement();
    let request_id = match aggregate.state() {
        SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Initiated(state)) => state
            .pending_exchange()
            .request_envelope()
            .header()
            .message_id(),
        _ => panic!("fixture must be Initiated Joiner"),
    };
    let candidate_reply = SpaceAdmissionEnvelopeV1::new(
        aggregate.admission_id(),
        AdmissionRole::Sponsor,
        0,
        AdmissionMessageId::from_bytes([0x24; 32]).expect("non-zero message id fixture"),
        Some(request_id),
        SpaceAdmissionBodyV1::Candidate(candidate_body_fixture()),
    )
    .expect("valid Candidate reply fixture");
    let candidate = aggregate
        .accept_candidate(
            candidate_reply,
            [0x25; 32],
            AdmissionStagedTargetInput::from_bytes(vec![0x26; 128])
                .expect("bounded staged target input fixture"),
        )
        .expect("authenticated Joiner accepts matching Candidate");

    assert_eq!(candidate.record_version(), 2);
    let state = match candidate.state() {
        SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Candidate(state)) => state,
        _ => panic!("Joiner must advance to Candidate"),
    };
    assert_eq!(state.candidate_evidence().canonical_digest(), &[0x25; 32]);
    assert_eq!(
        state.candidate().kind(),
        SpaceAdmissionMessageKind::Candidate
    );
}

#[test]
fn joiner_candidate_advances_to_prepared_with_exact_saved_request() {
    let candidate = joiner_candidate_aggregate_fixture();

    let candidate_message_id = match candidate.state() {
        SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Candidate(state)) => {
            state.candidate().header().message_id()
        }
        _ => panic!("fixture must be Candidate Joiner"),
    };

    let prepared_request = SpaceAdmissionEnvelopeV1::new(
        candidate.admission_id(),
        AdmissionRole::Joiner,
        1,
        AdmissionMessageId::from_bytes([0x31; 32]).expect("non-zero message id fixture"),
        Some(candidate_message_id),
        SpaceAdmissionBodyV1::Prepared(AdmissionPreparedV1::new(PreparedAdmissionProofV1::new(
            *candidate.admission_id().as_bytes(),
            "lineage".to_owned(),
            BaseMembershipHistoryPosition {
                event_id: None,
                depth: 0,
                history_digest: [0x32; 32],
            },
            candidate_body_fixture().candidate_event().event_id(),
            [0x33; 32],
            [0x34; 32],
            MemberInstanceId::from_bytes([0x35; 32]),
            MembershipCredential::new(1, vec![0x36; 32]).credential_id,
            vec![0x37; 64],
        ))),
    )
    .expect("valid Prepared request fixture");

    let pending_exchange = PendingAdmissionExchange::new(
        SpaceAdmissionRoute::from_bytes(vec![0x38; 32]).expect("bounded route fixture"),
        prepared_request,
        SpaceAdmissionMessageKind::Commit,
        AdmissionRetryState::new(0, 0).expect("valid retry state"),
    )
    .expect("Prepared expects Commit");

    let prepared = candidate
        .prepare_candidate(
            AdmissionSignedMembershipHistory::from_bytes(vec![0x39; 128])
                .expect("bounded verified history fixture"),
            crate::membership::AdmissionStagedTarget::from_bytes(vec![0x3a; 128])
                .expect("bounded staged target fixture"),
            pending_exchange,
        )
        .expect("Candidate Joiner advances to Prepared");

    assert_eq!(prepared.record_version(), 3);

    let state = match prepared.state() {
        SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Prepared(state)) => state,
        _ => panic!("Joiner must advance to Prepared"),
    };

    assert_eq!(
        state.pending_exchange().request_envelope().kind(),
        SpaceAdmissionMessageKind::Prepared
    );
    assert!(state
        .pending_exchange()
        .exact_reply_for(state.candidate_evidence())
        .is_some());
}
