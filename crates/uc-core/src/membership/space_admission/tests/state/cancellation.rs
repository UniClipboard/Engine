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
fn supersession_terminates_bounded_joiners_before_and_after_prepared() {
    let superseded = joiner_candidate_aggregate_fixture()
        .supersede()
        .expect("Candidate Joiner can be superseded");
    assert!(matches!(
        superseded.state(),
        SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Superseded(_))
    ));

    let prepared = joiner_prepared_aggregate_fixture()
        .supersede()
        .expect("Prepared Joiner can terminate locally when superseded")
        .into_replacement();
    let prepared = JoinerAdmission::try_from_record(prepared).expect("terminated Joiner record");
    assert_eq!(
        prepared.termination_reason(),
        Some(SpaceAdmissionTerminationReason::Superseded)
    );
    assert_eq!(
        prepared
            .cleanup_obligation()
            .expect("Prepared Joiner keeps cleanup responsibility")
            .commit_knowledge(),
        AdmissionCommitKnowledge::Unknown
    );

    let cancelling = cancelling_joiner_aggregate_fixture()
        .supersede()
        .expect("Cancelling Joiner can be superseded by a new intent")
        .into_replacement();
    let cancelling = JoinerAdmission::decode_persisted(
        &cancelling
            .encode_persisted()
            .expect("superseded Cancelling Joiner encodes"),
    )
    .expect("superseded Cancelling Joiner decodes");
    assert_eq!(
        cancelling.termination_reason(),
        Some(SpaceAdmissionTerminationReason::Superseded)
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
fn formal_commit_blocks_cancel_message_but_allows_local_supersession_cleanup() {
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
    let superseded = JoinerAdmission::try_from_record(joiner_committed_aggregate_fixture())
        .expect("committed joiner")
        .supersede()
        .expect("a committed bounded join can terminate locally")
        .into_replacement();
    assert_eq!(
        superseded.termination_reason(),
        Some(SpaceAdmissionTerminationReason::Superseded)
    );
    let cleanup = superseded
        .cleanup_obligation()
        .expect("known commit keeps cleanup responsibility");
    assert_eq!(cleanup.commit_knowledge(), AdmissionCommitKnowledge::Known);
    assert!(cleanup.member_binding().is_some());
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
fn bounded_joiner_and_prepared_joiner_both_terminate_locally() {
    let cancelled = JoinerAdmission::try_from_record(initiated_joiner_aggregate_fixture())
        .expect("joiner fixture")
        .cancel_locally()
        .expect("early join cancels locally")
        .into_replacement();
    assert_eq!(
        cancelled.termination_reason(),
        Some(SpaceAdmissionTerminationReason::Cancelled)
    );

    let prepared = JoinerAdmission::try_from_record(joiner_prepared_aggregate_fixture())
        .expect("prepared joiner fixture")
        .terminate_if_expired(301_000)
        .expect("prepared join can expire locally")
        .expect("deadline is due")
        .into_replacement();
    let cleanup = prepared
        .cleanup_obligation()
        .expect("unknown remote commit keeps cleanup responsibility");
    assert_eq!(
        cleanup.commit_knowledge(),
        AdmissionCommitKnowledge::Unknown
    );
    assert!(cleanup.member_binding().is_none());
    assert_eq!(
        cleanup
            .pending_exchange()
            .expect("cleanup delivery remains pending")
            .request_envelope()
            .kind(),
        SpaceAdmissionMessageKind::Abandonment
    );
    let encoded = prepared.encode_persisted().expect("cleanup state encodes");
    let recovered = JoinerAdmission::decode_persisted(&encoded).expect("cleanup state decodes");
    assert_eq!(
        recovered
            .cleanup_obligation()
            .expect("cleanup survives restart")
            .commit_knowledge(),
        AdmissionCommitKnowledge::Unknown
    );
}

#[test]
fn activating_joiner_keeps_the_exact_local_transition_after_termination() {
    let activating = JoinerAdmission::try_from_record(joiner_activating_aggregate_fixture())
        .expect("activating Joiner fixture");
    let expected = activating
        .joiner_activation_preparation()
        .expect("activation preparation")
        .space_transition()
        .as_bytes()
        .to_vec();

    let terminated = activating
        .cancel_locally()
        .expect("activating Joiner terminates locally")
        .into_replacement();
    let encoded = terminated
        .encode_persisted()
        .expect("termination responsibility encodes");
    let reopened = JoinerAdmission::decode_persisted(&encoded)
        .expect("termination responsibility survives restart");

    assert_eq!(
        reopened
            .cleanup_obligation()
            .and_then(AdmissionCleanupObligation::local_space_transition)
            .expect("local transition remains recoverable")
            .as_bytes(),
        expected
    );
}

#[test]
fn completed_local_termination_releases_only_the_large_transition_plan() {
    let terminated = JoinerAdmission::try_from_record(joiner_activating_aggregate_fixture())
        .expect("activating Joiner fixture")
        .cancel_locally()
        .expect("activating Joiner terminates locally")
        .into_replacement();

    let completed = terminated
        .complete_local_space_termination()
        .expect("local termination completion is recorded")
        .into_replacement();
    let encoded = completed
        .encode_persisted()
        .expect("compacted termination record encodes");
    let reopened = JoinerAdmission::decode_persisted(&encoded)
        .expect("compacted termination record survives restart");

    let cleanup = reopened
        .cleanup_obligation()
        .expect("minimum cleanup evidence remains");
    assert!(cleanup.local_space_transition().is_none());
    assert!(cleanup.pending_exchange().is_some());
    assert_eq!(
        reopened.termination_reason(),
        Some(SpaceAdmissionTerminationReason::Cancelled)
    );
}

#[test]
fn abandonment_acknowledgement_ends_delivery_without_removing_the_fence() {
    let terminated = JoinerAdmission::try_from_record(joiner_prepared_aggregate_fixture())
        .expect("prepared joiner fixture")
        .cancel_locally()
        .expect("prepared join terminates locally")
        .into_replacement();
    let request = terminated
        .cleanup_obligation()
        .and_then(AdmissionCleanupObligation::pending_exchange)
        .expect("abandonment delivery is pending")
        .request_envelope();
    let request_digest: [u8; 32] = Sha256::digest(
        request
            .encode_canonical_v1()
            .expect("abandonment request encodes"),
    )
    .into();
    let acknowledged = SpaceAdmissionEnvelopeV1::reply_to(
        request,
        AdmissionRole::Sponsor,
        4,
        AdmissionMessageId::from_bytes([0xf4; 32]).expect("ack message id fixture"),
        SpaceAdmissionBodyV1::Abandoned(
            AdmissionAbandonedV2::new(request_digest).expect("request digest fixture"),
        ),
    )
    .expect("valid Abandoned reply");
    let acknowledged_digest: [u8; 32] = Sha256::digest(
        acknowledged
            .encode_canonical_v1()
            .expect("Abandoned reply encodes"),
    )
    .into();

    let completed = terminated
        .accept_abandoned(acknowledged, acknowledged_digest)
        .expect("matching acknowledgement is accepted")
        .into_replacement();

    let cleanup = completed
        .cleanup_obligation()
        .expect("termination fence remains saved");
    assert!(cleanup.pending_exchange().is_none());
    assert_eq!(
        completed.termination_reason(),
        Some(SpaceAdmissionTerminationReason::Cancelled)
    );
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
fn legacy_post_decision_joiners_can_be_ended_for_a_new_intent() {
    for (name, aggregate) in [
        ("prepared", joiner_prepared_aggregate_fixture()),
        ("committed", joiner_committed_aggregate_fixture()),
        ("applied", joiner_applied_aggregate_fixture()),
        ("activating", joiner_activating_aggregate_fixture()),
        ("cancelling", cancelling_joiner_aggregate_fixture()),
    ] {
        let legacy = aggregate.into_legacy_persistence_fixture();
        let legacy = JoinerAdmission::decode_persisted(
            &legacy
                .encode_persisted()
                .unwrap_or_else(|error| panic!("{name} legacy fixture encodes: {error}")),
        )
        .unwrap_or_else(|error| panic!("{name} legacy fixture decodes: {error}"));
        assert!(legacy.can_terminate_locally(), "{name}");
        let ended = legacy
            .supersede()
            .unwrap_or_else(|error| panic!("{name} legacy Joiner can end: {error}"))
            .into_replacement();

        assert!(ended.is_terminal(), "{name}");
        assert_eq!(
            ended.termination_reason(),
            Some(SpaceAdmissionTerminationReason::Cancelled),
            "{name}"
        );
        let encoded = ended
            .encode_persisted()
            .unwrap_or_else(|error| panic!("{name} terminal result encodes: {error}"));
        let decoded = JoinerAdmission::decode_persisted(&encoded)
            .unwrap_or_else(|error| panic!("{name} terminal result decodes: {error}"));
        assert!(decoded.is_terminal(), "{name}");
    }
}

#[test]
fn legacy_joiner_can_still_save_an_authenticated_channel() {
    let legacy = initiated_joiner_aggregate_fixture().into_legacy_persistence_fixture();
    let peer_binding = AdmissionPeerBinding::new(
        AdmissionChannelPeerId::from_bytes([0xf1; 32]).expect("local peer fixture"),
        AdmissionChannelPeerId::from_bytes([0xf2; 32]).expect("remote peer fixture"),
    )
    .expect("distinct peer fixture");
    let continuation = AdmissionContinuationCredential::from_bytes(vec![0xf3; 64])
        .expect("continuation fixture");
    let authenticated = JoinerAdmission::try_from_record(legacy)
        .expect("legacy joiner fixture")
        .with_authenticated_channel(peer_binding, continuation)
        .expect("legacy joiner can continue authentication")
        .into_replacement();

    assert!(matches!(
        authenticated.pending_recovery(),
        Some(AdmissionPendingRecovery::Continuation { .. })
    ));
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
use sha2::{Digest, Sha256};
