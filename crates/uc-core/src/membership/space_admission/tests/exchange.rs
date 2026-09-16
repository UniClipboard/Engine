use super::*;

#[test]
fn message_evidence_preserves_ordering_facts_without_leaking_ids() {
    let message_id =
        AdmissionMessageId::from_bytes([0x41; 32]).expect("non-zero message id fixture");
    let predecessor =
        AdmissionMessageId::from_bytes([0x42; 32]).expect("non-zero message id fixture");
    let evidence = AdmissionMessageEvidence::new(
        AdmissionRole::Joiner,
        3,
        message_id,
        Some(predecessor),
        [0x43; 32],
    )
    .expect("non-zero message digest fixture");

    assert_eq!(evidence.sender_role(), AdmissionRole::Joiner);
    assert_eq!(evidence.sender_sequence(), 3);
    assert_eq!(evidence.message_id(), message_id);
    assert_eq!(evidence.predecessor_message_id(), Some(predecessor));
    assert_eq!(evidence.canonical_digest(), &[0x43; 32]);
    assert_eq!(
            format!("{evidence:?}"),
            "AdmissionMessageEvidence { sender_role: Joiner, sender_sequence: 3, message_id: \"[REDACTED]\", has_predecessor: true, canonical_digest: \"[REDACTED]\" }"
        );
}

#[test]
fn message_evidence_rejects_a_zero_digest() {
    let message_id =
        AdmissionMessageId::from_bytes([0x44; 32]).expect("non-zero message id fixture");

    assert!(
        AdmissionMessageEvidence::new(AdmissionRole::Sponsor, 0, message_id, None, [0; 32],)
            .is_none()
    );
}

#[test]
fn identical_message_evidence_is_an_exact_replay() {
    let message_id =
        AdmissionMessageId::from_bytes([0x51; 32]).expect("non-zero message id fixture");
    let predecessor =
        AdmissionMessageId::from_bytes([0x52; 32]).expect("non-zero message id fixture");
    let known = AdmissionMessageEvidence::new(
        AdmissionRole::Joiner,
        4,
        message_id,
        Some(predecessor),
        [0x53; 32],
    )
    .expect("non-zero message digest fixture");
    let incoming = AdmissionMessageEvidence::new(
        AdmissionRole::Joiner,
        4,
        message_id,
        Some(predecessor),
        [0x53; 32],
    )
    .expect("non-zero message digest fixture");

    assert_eq!(
        known.relation_to(&incoming),
        AdmissionEvidenceRelation::ExactReplay
    );
}

#[test]
fn reused_message_id_with_changed_evidence_is_a_conflict() {
    let message_id =
        AdmissionMessageId::from_bytes([0x54; 32]).expect("non-zero message id fixture");
    let known =
        AdmissionMessageEvidence::new(AdmissionRole::Sponsor, 2, message_id, None, [0x55; 32])
            .expect("non-zero message digest fixture");

    for incoming in [
        AdmissionMessageEvidence::new(AdmissionRole::Sponsor, 2, message_id, None, [0x56; 32])
            .expect("non-zero message digest fixture"),
        AdmissionMessageEvidence::new(AdmissionRole::Sponsor, 3, message_id, None, [0x55; 32])
            .expect("non-zero message digest fixture"),
        AdmissionMessageEvidence::new(
            AdmissionRole::CompletionHelper,
            2,
            message_id,
            None,
            [0x55; 32],
        )
        .expect("non-zero message digest fixture"),
    ] {
        assert_eq!(
            known.relation_to(&incoming),
            AdmissionEvidenceRelation::Conflict
        );
    }
}

#[test]
fn a_different_message_id_is_not_a_replay() {
    let known = AdmissionMessageEvidence::new(
        AdmissionRole::Joiner,
        0,
        AdmissionMessageId::from_bytes([0x57; 32]).expect("non-zero message id fixture"),
        None,
        [0x58; 32],
    )
    .expect("non-zero message digest fixture");
    let incoming = AdmissionMessageEvidence::new(
        AdmissionRole::Joiner,
        1,
        AdmissionMessageId::from_bytes([0x59; 32]).expect("non-zero message id fixture"),
        Some(known.message_id()),
        [0x5a; 32],
    )
    .expect("non-zero message digest fixture");

    assert_eq!(
        known.relation_to(&incoming),
        AdmissionEvidenceRelation::Distinct
    );
}

fn cancellation_envelope(
    admission_id: SpaceAdmissionId,
    sender_sequence: u64,
    message_id: AdmissionMessageId,
    predecessor: AdmissionMessageId,
) -> SpaceAdmissionEnvelopeV1 {
    SpaceAdmissionEnvelopeV1::new(
        admission_id,
        AdmissionRole::Joiner,
        sender_sequence,
        message_id,
        Some(predecessor),
        SpaceAdmissionBodyV1::CancelRequested,
    )
    .expect("valid cancellation envelope fixture")
}

#[test]
fn inbound_expectation_accepts_only_the_exact_next_message() {
    let admission_id =
        SpaceAdmissionId::from_bytes([0x91; 32]).expect("non-zero admission id fixture");
    let message_id =
        AdmissionMessageId::from_bytes([0x92; 32]).expect("non-zero message id fixture");
    let predecessor =
        AdmissionMessageId::from_bytes([0x93; 32]).expect("non-zero message id fixture");
    let envelope = cancellation_envelope(admission_id, 3, message_id, predecessor);
    let expectation =
        AdmissionInboundExpectation::new(admission_id, AdmissionRole::Joiner, 3, Some(predecessor));

    let decision = expectation
        .classify(&envelope, [0x94; 32], None)
        .expect("the exact next message must be accepted");
    let AdmissionInboundDecision::New(evidence) = decision else {
        panic!("expected a new-message decision");
    };
    assert_eq!(evidence.message_id(), message_id);
    assert_eq!(evidence.canonical_digest(), &[0x94; 32]);
}

#[test]
fn inbound_expectation_replays_known_message_before_sequence_checks() {
    let admission_id =
        SpaceAdmissionId::from_bytes([0x95; 32]).expect("non-zero admission id fixture");
    let message_id =
        AdmissionMessageId::from_bytes([0x96; 32]).expect("non-zero message id fixture");
    let predecessor =
        AdmissionMessageId::from_bytes([0x97; 32]).expect("non-zero message id fixture");
    let envelope = cancellation_envelope(admission_id, 1, message_id, predecessor);
    let known = envelope
        .evidence([0x98; 32])
        .expect("non-zero canonical digest fixture");
    let advanced_expectation =
        AdmissionInboundExpectation::new(admission_id, AdmissionRole::Joiner, 2, Some(message_id));

    assert!(matches!(
        advanced_expectation.classify(&envelope, [0x98; 32], Some(&known)),
        Ok(AdmissionInboundDecision::ExactReplay)
    ));
}

#[test]
fn inbound_expectation_rejects_conflict_and_out_of_order_messages() {
    let admission_id =
        SpaceAdmissionId::from_bytes([0x99; 32]).expect("non-zero admission id fixture");
    let message_id =
        AdmissionMessageId::from_bytes([0x9a; 32]).expect("non-zero message id fixture");
    let predecessor =
        AdmissionMessageId::from_bytes([0x9b; 32]).expect("non-zero message id fixture");
    let envelope = cancellation_envelope(admission_id, 4, message_id, predecessor);
    let known = envelope
        .evidence([0x9c; 32])
        .expect("non-zero canonical digest fixture");
    let expectation =
        AdmissionInboundExpectation::new(admission_id, AdmissionRole::Joiner, 4, Some(predecessor));

    assert_eq!(
        expectation.classify(&envelope, [0x9d; 32], Some(&known)),
        Err(AdmissionProtocolMessageError::Conflict)
    );

    let wrong_sequence = cancellation_envelope(admission_id, 5, message_id, predecessor);
    assert_eq!(
        expectation.classify(&wrong_sequence, [0x9e; 32], None),
        Err(AdmissionProtocolMessageError::OutOfOrder)
    );

    let wrong_predecessor = cancellation_envelope(
        admission_id,
        4,
        message_id,
        AdmissionMessageId::from_bytes([0x9f; 32]).expect("non-zero message id fixture"),
    );
    assert_eq!(
        expectation.classify(&wrong_predecessor, [0xa0; 32], None),
        Err(AdmissionProtocolMessageError::OutOfOrder)
    );
}

#[test]
fn pending_exchange_keeps_the_exact_request_and_checked_retry_state() {
    let admission_id =
        SpaceAdmissionId::from_bytes([0xa1; 32]).expect("non-zero admission id fixture");
    let message_id =
        AdmissionMessageId::from_bytes([0xa2; 32]).expect("non-zero message id fixture");
    let predecessor =
        AdmissionMessageId::from_bytes([0xa3; 32]).expect("non-zero message id fixture");
    let request = cancellation_envelope(admission_id, 1, message_id, predecessor);
    let route =
        SpaceAdmissionRoute::from_bytes(vec![0xa4; 64]).expect("bounded non-empty route fixture");
    let retry = AdmissionRetryState::new(0, 100).expect("valid initial retry state");
    let exchange =
        PendingAdmissionExchange::new(route, request, SpaceAdmissionMessageKind::Rejected, retry)
            .expect("Rejected is the reply to CancelRequested");

    assert_eq!(
        exchange.request_envelope().kind(),
        SpaceAdmissionMessageKind::CancelRequested
    );
    assert_eq!(
        exchange.exact_expected_reply_kind(),
        SpaceAdmissionMessageKind::Rejected
    );
    let next_retry = exchange
        .retry_state()
        .after_failure(200)
        .expect("checked retry increment");
    assert_eq!(next_retry.attempt_count(), 1);
    assert_eq!(next_retry.next_attempt_at_ms(), 200);
    let inbound =
        AdmissionMessageEvidence::new(AdmissionRole::Sponsor, 0, predecessor, None, [0xa5; 32])
            .expect("non-zero inbound digest fixture");
    assert_eq!(
        exchange
            .exact_reply_for(&inbound)
            .map(SpaceAdmissionEnvelopeV1::kind),
        Some(SpaceAdmissionMessageKind::CancelRequested)
    );
    assert_eq!(
        AdmissionRetryState::new(u32::MAX, 100)
            .expect("valid exhausted retry fixture")
            .after_failure(200),
        Err(AdmissionPendingExchangeError::RetryCountOverflow)
    );
}

#[test]
fn pending_exchange_rejects_an_impossible_reply_kind() {
    let request = cancellation_envelope(
        SpaceAdmissionId::from_bytes([0xa5; 32]).expect("non-zero admission id fixture"),
        1,
        AdmissionMessageId::from_bytes([0xa6; 32]).expect("non-zero message id fixture"),
        AdmissionMessageId::from_bytes([0xa7; 32]).expect("non-zero message id fixture"),
    );
    let route =
        SpaceAdmissionRoute::from_bytes(vec![0xa8; 32]).expect("bounded non-empty route fixture");

    assert_eq!(
        PendingAdmissionExchange::new(
            route,
            request,
            SpaceAdmissionMessageKind::Candidate,
            AdmissionRetryState::new(0, 0).expect("valid initial retry state"),
        ),
        Err(AdmissionPendingExchangeError::InvalidExpectedReply)
    );
}

#[test]
fn saved_reply_must_directly_answer_the_saved_inbound_message() {
    let admission_id =
        SpaceAdmissionId::from_bytes([0xa9; 32]).expect("non-zero admission id fixture");
    let inbound_id =
        AdmissionMessageId::from_bytes([0xaa; 32]).expect("non-zero message id fixture");
    let inbound =
        AdmissionMessageEvidence::new(AdmissionRole::Joiner, 0, inbound_id, None, [0xab; 32])
            .expect("non-zero canonical digest fixture");
    let reply = SpaceAdmissionEnvelopeV1::new(
        admission_id,
        AdmissionRole::Sponsor,
        0,
        AdmissionMessageId::from_bytes([0xac; 32]).expect("non-zero message id fixture"),
        Some(inbound_id),
        SpaceAdmissionBodyV1::Rejected {
            reason: SpaceAdmissionRejectionReason::InvitationUnavailable,
        },
    )
    .expect("valid rejection reply fixture");

    let saved = SavedAdmissionReply::new(admission_id, inbound, reply)
        .expect("reply directly follows inbound evidence");
    assert_eq!(
        saved.exact_reply_envelope().kind(),
        SpaceAdmissionMessageKind::Rejected
    );
}
