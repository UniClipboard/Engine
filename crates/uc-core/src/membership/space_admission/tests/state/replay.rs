fn copied_evidence(evidence: &AdmissionMessageEvidence) -> AdmissionMessageEvidence {
    AdmissionMessageEvidence::new(
        evidence.sender_role(),
        evidence.sender_sequence(),
        evidence.message_id(),
        evidence.predecessor_message_id(),
        *evidence.canonical_digest(),
    )
    .expect("saved evidence digest is non-zero")
}

#[test]
fn aggregate_replays_exact_saved_reply_for_identical_evidence() {
    let sponsor = sponsor_candidate_aggregate_fixture();
    let incoming = match sponsor.state() {
        SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Candidate(state)) => {
            copied_evidence(state.saved_reply().inbound_evidence())
        }
        _ => panic!("fixture must be Candidate Sponsor"),
    };

    let decision = sponsor
        .replay_or_reject(&incoming)
        .expect("identical evidence is replayable");
    let AdmissionReplayDecision::ExactReply(reply) = decision else {
        panic!("identical evidence must return exact reply");
    };
    assert_eq!(reply.kind(), SpaceAdmissionMessageKind::Candidate);
}

#[test]
fn aggregate_rejects_reused_message_id_with_changed_digest() {
    let sponsor = sponsor_candidate_aggregate_fixture();
    let conflicting = match sponsor.state() {
        SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Candidate(state)) => {
            let known = state.saved_reply().inbound_evidence();
            AdmissionMessageEvidence::new(
                known.sender_role(),
                known.sender_sequence(),
                known.message_id(),
                known.predecessor_message_id(),
                [0xbe; 32],
            )
            .expect("non-zero conflicting digest fixture")
        }
        _ => panic!("fixture must be Candidate Sponsor"),
    };

    assert!(matches!(
        sponsor.replay_or_reject(&conflicting),
        Err(AdmissionReplayError::Conflict)
    ));
}

#[test]
fn aggregate_accepts_only_the_next_expected_evidence() {
    let sponsor = sponsor_candidate_aggregate_fixture();
    let predecessor = match sponsor.state() {
        SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Candidate(state)) => state
            .saved_reply()
            .exact_reply_envelope()
            .header()
            .message_id(),
        _ => panic!("fixture must be Candidate Sponsor"),
    };
    let next = AdmissionMessageEvidence::new(
        AdmissionRole::Joiner,
        1,
        AdmissionMessageId::from_bytes([0xbf; 32]).expect("non-zero message id fixture"),
        Some(predecessor),
        [0xc0; 32],
    )
    .expect("non-zero next evidence fixture");
    assert!(matches!(
        sponsor.replay_or_reject(&next),
        Ok(AdmissionReplayDecision::New)
    ));

    let out_of_order = AdmissionMessageEvidence::new(
        AdmissionRole::Joiner,
        2,
        AdmissionMessageId::from_bytes([0xc1; 32]).expect("non-zero message id fixture"),
        Some(predecessor),
        [0xc2; 32],
    )
    .expect("non-zero out-of-order evidence fixture");
    assert!(matches!(
        sponsor.replay_or_reject(&out_of_order),
        Err(AdmissionReplayError::OutOfOrder)
    ));
}
