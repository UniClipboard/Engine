#[test]
fn joiner_candidate_saves_cancel_request_without_becoming_terminal() {
    let candidate = joiner_candidate_aggregate_fixture();
    let cancelling = candidate
        .request_cancel(
        AdmissionMessageId::from_bytes([0xb3; 32]).expect("non-zero message id fixture"),
        AdmissionRetryState::new(0, 0).expect("valid retry state"),
        )
        .expect("Candidate Joiner can be cancelled");

    let state = match cancelling.state() {
        SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Cancelling(state)) => state,
        _ => panic!("Joiner must advance to Cancelling"),
    };
    assert_eq!(
        state.pending_exchange().request_envelope().kind(),
        SpaceAdmissionMessageKind::CancelRequested
    );
}

#[test]
fn supersession_is_allowed_through_candidate_but_not_after_prepared() {
    let superseded = joiner_candidate_aggregate_fixture()
        .supersede()
        .expect("Candidate Joiner can be superseded");
    assert!(matches!(
        superseded.state(),
        SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Superseded(_))
    ));

    assert_eq!(
        joiner_prepared_aggregate_fixture().supersede(),
        Err(SpaceAdmissionAggregateError::UnsafeSupersession)
    );
}

#[test]
fn initiated_joiner_can_be_superseded_before_authentication() {
    let superseded = initiated_joiner_aggregate_fixture()
        .supersede()
        .expect("Initiated Joiner can be superseded");

    assert!(matches!(
        superseded.state(),
        SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Superseded(_))
    ));
}

#[test]
fn prepared_joiner_can_cancel_before_formal_commit() {
    let prepared = joiner_prepared_aggregate_fixture();
    let cancelling = prepared
        .request_cancel(
        AdmissionMessageId::from_bytes([0xb5; 32]).expect("non-zero message id fixture"),
        AdmissionRetryState::new(0, 0).expect("valid retry state"),
        )
        .expect("Prepared Joiner can cancel before Commit");

    assert!(matches!(
        cancelling.state(),
        SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Cancelling(_))
    ));
}

#[test]
fn formal_commit_blocks_cancellation_and_supersession() {
    let committed = joiner_committed_aggregate_fixture();
    let commit_message_id = match committed.state() {
        SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Committed(state)) => {
            state.commit_evidence().message_id()
        }
        _ => panic!("fixture must be Committed Joiner"),
    };
    let cancel_request = SpaceAdmissionEnvelopeV1::new(
        committed.admission_id(),
        AdmissionRole::Joiner,
        2,
        AdmissionMessageId::from_bytes([0xec; 32]).expect("non-zero message id fixture"),
        Some(commit_message_id),
        SpaceAdmissionBodyV1::CancelRequested,
    )
    .expect("valid CancelRequested fixture");
    let pending_exchange = PendingAdmissionExchange::new(
        SpaceAdmissionRoute::from_bytes(vec![0xed; 32]).expect("bounded route fixture"),
        cancel_request,
        SpaceAdmissionMessageKind::Rejected,
        AdmissionRetryState::new(0, 0).expect("valid retry state"),
    )
    .expect("CancelRequested expects Rejected");

    assert_eq!(
        committed.cancel(pending_exchange),
        Err(SpaceAdmissionAggregateError::TooLateCommitted)
    );
    assert_eq!(
        joiner_committed_aggregate_fixture().supersede(),
        Err(SpaceAdmissionAggregateError::UnsafeSupersession)
    );
}

#[test]
fn unauthenticated_initiated_joiner_cancels_locally_without_network_message() {
    let cancelled = initiated_joiner_aggregate_fixture()
        .cancel_before_authentication()
        .expect("unauthenticated Initiated Joiner cancels locally");

    assert!(cancelled.exact_reply().is_none());
    let state = match cancelled.state() {
        SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Rejected(
            SpaceAdmissionRejectedState::LocalJoiner(state),
        )) => state,
        _ => panic!("Joiner must become locally Rejected"),
    };
    assert_eq!(state.reason(), SpaceAdmissionRejectionReason::Cancelled);
}

#[test]
fn bounded_joiner_expires_only_at_its_persisted_deadline() {
    let before_deadline = JoinerAdmission::try_from_record(initiated_joiner_aggregate_fixture())
        .expect("joiner fixture")
        .terminate_if_expired(300_999)
        .expect("expiry check succeeds");
    assert!(before_deadline.is_none());

    let expired = JoinerAdmission::try_from_record(initiated_joiner_aggregate_fixture())
        .expect("joiner fixture")
        .terminate_if_expired(301_000)
        .expect("expiry transition succeeds")
        .expect("deadline is due")
        .into_replacement();
    assert_eq!(
        expired.termination_reason(),
        Some(SpaceAdmissionTerminationReason::Expired)
    );
    assert_eq!(expired.termination_local_join_ordinal(), Some(2));
    let encoded = expired
        .encode_persisted()
        .expect("bounded terminal result encodes");
    let recovered = JoinerAdmission::decode_persisted(&encoded)
        .expect("bounded terminal result decodes");
    assert_eq!(recovered.termination_local_join_ordinal(), Some(2));
}

#[test]
fn bounded_joiner_cancels_locally_but_prepared_joiner_is_outside_s1() {
    let cancelled = JoinerAdmission::try_from_record(initiated_joiner_aggregate_fixture())
        .expect("joiner fixture")
        .cancel_locally()
        .expect("early join cancels locally")
        .into_replacement();
    assert_eq!(
        cancelled.termination_reason(),
        Some(SpaceAdmissionTerminationReason::Cancelled)
    );

    assert!(matches!(
        JoinerAdmission::try_from_record(joiner_prepared_aggregate_fixture())
            .expect("prepared joiner fixture")
            .terminate_if_expired(301_000),
        Err(SpaceAdmissionAggregateError::UnsafeCancellation)
    ));
}

#[test]
fn legacy_joiner_can_still_cancel_locally_without_inventing_a_deadline() {
    let legacy = initiated_joiner_aggregate_fixture().into_legacy_persistence_fixture();
    let cancelled = JoinerAdmission::try_from_record(legacy)
        .expect("legacy joiner fixture")
        .cancel_locally()
        .expect("legacy early join cancels locally")
        .into_replacement();
    assert_eq!(
        cancelled.termination_reason(),
        Some(SpaceAdmissionTerminationReason::Cancelled)
    );
    assert_eq!(cancelled.expires_at_ms(), None);

    let encoded = cancelled.encode_persisted().expect("legacy result encodes");
    let decoded = JoinerAdmission::decode_persisted(&encoded).expect("legacy result decodes");
    assert_eq!(
        decoded.termination_reason(),
        Some(SpaceAdmissionTerminationReason::Cancelled)
    );
    assert_eq!(decoded.expires_at_ms(), None);
}

#[test]
fn sponsor_rejects_cancel_before_commit_and_saves_exact_reply() {
    let sponsor = sponsor_candidate_aggregate_fixture();
    let candidate_message_id = match sponsor.state() {
        SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Candidate(state)) => state
            .saved_reply()
            .exact_reply_envelope()
            .header()
            .message_id(),
        _ => panic!("fixture must be Candidate Sponsor"),
    };
    let cancel_message_id =
        AdmissionMessageId::from_bytes([0xb7; 32]).expect("non-zero message id fixture");
    let cancel = SpaceAdmissionEnvelopeV1::new(
        sponsor.admission_id(),
        AdmissionRole::Joiner,
        1,
        cancel_message_id,
        Some(candidate_message_id),
        SpaceAdmissionBodyV1::CancelRequested,
    )
    .expect("valid CancelRequested fixture");
    let rejected_reply = SpaceAdmissionEnvelopeV1::new(
        sponsor.admission_id(),
        AdmissionRole::Sponsor,
        1,
        AdmissionMessageId::from_bytes([0xb8; 32]).expect("non-zero message id fixture"),
        Some(cancel_message_id),
        SpaceAdmissionBodyV1::Rejected {
            reason: SpaceAdmissionRejectionReason::Cancelled,
        },
    )
    .expect("valid Rejected fixture");

    let rejected = sponsor
        .reject_cancel(cancel, [0xb9; 32], rejected_reply)
        .expect("Candidate Sponsor rejects cancellation");

    assert!(rejected.effects().is_empty());
    let state = match rejected.state() {
        SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Rejected(
            SpaceAdmissionRejectedState::Sponsor(state),
        )) => state,
        _ => panic!("Sponsor must advance to Rejected"),
    };
    assert_eq!(state.reason(), SpaceAdmissionRejectionReason::Cancelled);
    assert_eq!(
        state.saved_reply().exact_reply_envelope().kind(),
        SpaceAdmissionMessageKind::Rejected
    );
}

#[test]
fn cancelling_joiner_accepts_cancelled_rejection() {
    let candidate = joiner_candidate_aggregate_fixture();
    let candidate_message_id = match candidate.state() {
        SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Candidate(state)) => {
            state.candidate_evidence().message_id()
        }
        _ => panic!("fixture must be Candidate Joiner"),
    };
    let cancel_message_id =
        AdmissionMessageId::from_bytes([0xba; 32]).expect("non-zero message id fixture");
    let cancel_request = SpaceAdmissionEnvelopeV1::new(
        candidate.admission_id(),
        AdmissionRole::Joiner,
        1,
        cancel_message_id,
        Some(candidate_message_id),
        SpaceAdmissionBodyV1::CancelRequested,
    )
    .expect("valid CancelRequested fixture");
    let cancelling = candidate
        .cancel(
            PendingAdmissionExchange::new(
                SpaceAdmissionRoute::from_bytes(vec![0xbb; 32]).expect("bounded route fixture"),
                cancel_request,
                SpaceAdmissionMessageKind::Rejected,
                AdmissionRetryState::new(0, 0).expect("valid retry state"),
            )
            .expect("CancelRequested expects Rejected"),
        )
        .expect("Cancelling Joiner fixture")
        .into_replacement();
    let rejected = SpaceAdmissionEnvelopeV1::new(
        cancelling.admission_id(),
        AdmissionRole::Sponsor,
        1,
        AdmissionMessageId::from_bytes([0xbc; 32]).expect("non-zero message id fixture"),
        Some(cancel_message_id),
        SpaceAdmissionBodyV1::Rejected {
            reason: SpaceAdmissionRejectionReason::Cancelled,
        },
    )
    .expect("valid Rejected fixture");

    let rejected = cancelling
        .accept_rejection(rejected, [0xbd; 32])
        .expect("Cancelling Joiner accepts Rejected");

    let state = match rejected.state() {
        SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Rejected(
            SpaceAdmissionRejectedState::Joiner(state),
        )) => state,
        _ => panic!("Joiner must advance to Rejected"),
    };
    assert_eq!(state.reason(), SpaceAdmissionRejectionReason::Cancelled);
    assert_eq!(state.last_received().canonical_digest(), &[0xbd; 32]);
}
