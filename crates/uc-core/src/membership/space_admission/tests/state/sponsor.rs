#[test]
fn sponsor_candidate_commits_prepared_with_exact_commit_reply() {
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
        AdmissionMessageId::from_bytes([0x4b; 32]).expect("non-zero message id fixture");
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
                history_digest: [0x4c; 32],
            },
            candidate_body_fixture().candidate_event().event_id(),
            [0x4d; 32],
            [0x4e; 32],
            MemberInstanceId::from_bytes([0x4f; 32]),
            MembershipCredential::new(1, vec![0x50; 32]).credential_id,
            vec![0x51; 64],
        ))),
    )
    .expect("valid Prepared fixture");
    let committed_history_bytes = vec![0x52; 128];
    let commit_reply = SpaceAdmissionEnvelopeV1::new(
        sponsor.admission_id(),
        AdmissionRole::Sponsor,
        1,
        AdmissionMessageId::from_bytes([0x53; 32]).expect("non-zero message id fixture"),
        Some(prepared_message_id),
        SpaceAdmissionBodyV1::Commit(AdmissionCommitV1::new(
            candidate_body_fixture(),
            AdmissionSignedMembershipHistory::from_bytes(committed_history_bytes.clone())
                .expect("bounded committed history fixture"),
            AdmissionSealedRecoveryMaterial::from_bytes(vec![0x54; 128])
                .expect("bounded sealed recovery fixture"),
        )),
    )
    .expect("valid Commit reply fixture");

    let committed = sponsor
        .commit_prepared(
            prepared,
            [0x55; 32],
            AdmissionSignedMembershipHistory::from_bytes(committed_history_bytes)
                .expect("bounded committed history fixture"),
            crate::membership::AdmissionSealedSecurityState::from_bytes(vec![0x56; 128])
                .expect("bounded sealed security fixture"),
            commit_reply,
        )
        .expect("Candidate Sponsor commits Prepared");

    assert_eq!(committed.effects(), &[AdmissionEffect::CommitMembership]);
    assert_eq!(
        committed
            .exact_reply()
            .expect("Commit transition has exact reply")
            .kind(),
        SpaceAdmissionMessageKind::Commit
    );
    assert_eq!(committed.record_version(), 2);
    let state = match committed.state() {
        SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Committed(state)) => state,
        _ => panic!("Sponsor must advance to Committed"),
    };
    assert_eq!(
        state.saved_reply().exact_reply_envelope().kind(),
        SpaceAdmissionMessageKind::Commit
    );
    assert_eq!(
        state.saved_reply().inbound_evidence().canonical_digest(),
        &[0x55; 32]
    );
}

#[test]
fn joiner_prepared_accepts_only_the_exact_committed_candidate() {
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
        AdmissionMessageId::from_bytes([0x5f; 32]).expect("non-zero message id fixture"),
        Some(prepared_message_id),
        SpaceAdmissionBodyV1::Commit(AdmissionCommitV1::new(
            candidate_body_fixture(),
            AdmissionSignedMembershipHistory::from_bytes(vec![0x5d; 128])
                .expect("bounded committed history fixture"),
            AdmissionSealedRecoveryMaterial::from_bytes(vec![0x60; 128])
                .expect("bounded sealed recovery fixture"),
        )),
    )
    .expect("valid Commit fixture");

    let committed = prepared
        .accept_commit(commit, [0x61; 32])
        .expect("Prepared Joiner accepts matching Commit");

    assert_eq!(committed.record_version(), 4);
    let state = match committed.state() {
        SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Committed(state)) => state,
        _ => panic!("Joiner must advance to Committed"),
    };
    assert_eq!(
        state.exact_commit().kind(),
        SpaceAdmissionMessageKind::Commit
    );
    assert_eq!(state.commit_evidence().canonical_digest(), &[0x61; 32]);
}

#[test]
fn joiner_committed_saves_exact_applied_request_before_delivery() {
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
        [0x65; 32],
        security_commitment_id,
        MemberInstanceId::from_bytes([0x66; 32]),
        vec![0x67; 64],
    );
    let applied_request = SpaceAdmissionEnvelopeV1::new(
        committed.admission_id(),
        AdmissionRole::Joiner,
        2,
        AdmissionMessageId::from_bytes([0x68; 32]).expect("non-zero message id fixture"),
        Some(commit_message_id),
        SpaceAdmissionBodyV1::Applied(AdmissionAppliedV1::new(receipt)),
    )
    .expect("valid Applied request fixture");
    let pending_exchange = PendingAdmissionExchange::new(
        SpaceAdmissionRoute::from_bytes(vec![0x69; 32]).expect("bounded route fixture"),
        applied_request,
        SpaceAdmissionMessageKind::Complete,
        AdmissionRetryState::new(0, 0).expect("valid retry state"),
    )
    .expect("Applied expects Complete");

    let applied = committed
        .apply_commit(pending_exchange)
        .expect("Committed Joiner saves Applied request");

    assert_eq!(applied.effects(), &[AdmissionEffect::ApplyMembership]);
    assert_eq!(applied.record_version(), 5);
    let state = match applied.state() {
        SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Applied(state)) => state,
        _ => panic!("Joiner must advance to Applied"),
    };
    assert_eq!(
        state.pending_exchange().request_envelope().kind(),
        SpaceAdmissionMessageKind::Applied
    );
    assert!(state
        .pending_exchange()
        .exact_reply_for(state.commit_evidence())
        .is_some());
}

#[test]
fn sponsor_committed_completes_applied_with_exact_complete_reply() {
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
        AdmissionMessageId::from_bytes([0x76; 32]).expect("non-zero message id fixture");
    let receipt = AdmissionActivationReceipt::new(
        1,
        *sponsor.admission_id().as_bytes(),
        event_id,
        [0x77; 32],
        security_commitment_id,
        MemberInstanceId::from_bytes([0x78; 32]),
        vec![0x79; 64],
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
        [0x7a; 32],
        security_commitment_id,
        MemberInstanceId::from_bytes([0x7b; 32]),
        MembershipCredential::new(1, vec![0x7c; 32]).credential_id,
        BaseMembershipHistoryPosition {
            event_id: Some(event_id),
            depth: 1,
            history_digest: [0x7d; 32],
        },
        vec![0x7e; 64],
    );
    let complete_reply = SpaceAdmissionEnvelopeV1::new(
        sponsor.admission_id(),
        AdmissionRole::Sponsor,
        2,
        AdmissionMessageId::from_bytes([0x7f; 32]).expect("non-zero message id fixture"),
        Some(applied_message_id),
        SpaceAdmissionBodyV1::Complete(AdmissionCompleteV1::new(completion)),
    )
    .expect("valid Complete reply fixture");

    let applied = sponsor
        .complete_applied(
            applied,
            [0x80; 32],
            crate::membership::AdmissionActivatedSecurityState::from_bytes(vec![0x81; 128])
                .expect("bounded activated security fixture"),
            complete_reply,
        )
        .expect("Committed Sponsor completes Applied");

    assert_eq!(
        applied.effects(),
        &[
            AdmissionEffect::ActivateSecurity,
            AdmissionEffect::PublishMembership,
        ]
    );
    assert_eq!(applied.record_version(), 3);
    let state = match applied.state() {
        SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Applied(state)) => state,
        _ => panic!("Sponsor must advance to Applied"),
    };
    assert_eq!(
        state.saved_reply().exact_reply_envelope().kind(),
        SpaceAdmissionMessageKind::Complete
    );
    assert_eq!(
        state.saved_reply().inbound_evidence().canonical_digest(),
        &[0x80; 32]
    );
}
