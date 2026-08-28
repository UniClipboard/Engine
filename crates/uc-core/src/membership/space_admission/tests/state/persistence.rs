fn assert_admission_persistence_round_trip(aggregate: SpaceAdmissionAggregate) {
    let encoded = aggregate
        .encode_persisted()
        .expect("legal admission state must be persistable");
    let decoded = SpaceAdmissionAggregate::decode_persisted(&encoded)
        .expect("persisted legal admission state must be recoverable");

    assert_eq!(decoded, aggregate);
}

fn cancelling_joiner_aggregate_fixture() -> SpaceAdmissionAggregate {
    let candidate = joiner_candidate_aggregate_fixture();
    let candidate_message_id = match candidate.state() {
        SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Candidate(state)) => {
            state.candidate_evidence().message_id()
        }
        _ => panic!("fixture must be Candidate Joiner"),
    };
    let cancel_request = SpaceAdmissionEnvelopeV1::new(
        candidate.admission_id(),
        AdmissionRole::Joiner,
        1,
        AdmissionMessageId::from_bytes([0x11; 32]).expect("non-zero message id fixture"),
        Some(candidate_message_id),
        SpaceAdmissionBodyV1::CancelRequested,
    )
    .expect("valid CancelRequested fixture");

    candidate
        .cancel(
            PendingAdmissionExchange::new(
                SpaceAdmissionRoute::from_bytes(vec![0x12; 32]).expect("bounded route fixture"),
                cancel_request,
                SpaceAdmissionMessageKind::Rejected,
                AdmissionRetryState::new(2, 42).expect("valid retry state"),
            )
            .expect("CancelRequested expects Rejected"),
        )
        .expect("Candidate Joiner can be cancelled")
        .into_replacement()
}

fn active_settled_aggregate_fixture() -> SpaceAdmissionAggregate {
    let active = active_pending_settlement_aggregate_fixture();
    let ack_message_id = match active.state() {
        SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Active(
            SpaceAdmissionActiveState::PendingSettlement(state),
        )) => state
            .pending_exchange()
            .request_envelope()
            .header()
            .message_id(),
        _ => panic!("fixture must be Active pending settlement"),
    };
    let settled = SpaceAdmissionEnvelopeV1::new(
        active.admission_id(),
        AdmissionRole::Sponsor,
        3,
        AdmissionMessageId::from_bytes([0x13; 32]).expect("non-zero message id fixture"),
        Some(ack_message_id),
        SpaceAdmissionBodyV1::Settled(
            AdmissionSettledV1::new([0x14; 32]).expect("non-zero ack digest fixture"),
        ),
    )
    .expect("valid Settled fixture");

    active
        .accept_settled(settled, [0x15; 32])
        .expect("Active Joiner accepts Settled")
        .into_replacement()
}

fn completed_aggregate_fixture() -> SpaceAdmissionAggregate {
    let sponsor = sponsor_applied_aggregate_fixture();
    let complete_message_id = match sponsor.state() {
        SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Applied(state)) => state
            .saved_reply()
            .exact_reply_envelope()
            .header()
            .message_id(),
        _ => panic!("fixture must be Applied Sponsor"),
    };
    let ack_message_id =
        AdmissionMessageId::from_bytes([0x16; 32]).expect("non-zero message id fixture");
    let ack = SpaceAdmissionEnvelopeV1::new(
        sponsor.admission_id(),
        AdmissionRole::Joiner,
        3,
        ack_message_id,
        Some(complete_message_id),
        SpaceAdmissionBodyV1::CompleteAck(
            AdmissionCompleteAckV1::new([0x17; 32]).expect("non-zero completion digest fixture"),
        ),
    )
    .expect("valid CompleteAck fixture");
    let settled_reply = SpaceAdmissionEnvelopeV1::new(
        sponsor.admission_id(),
        AdmissionRole::Sponsor,
        3,
        AdmissionMessageId::from_bytes([0x18; 32]).expect("non-zero message id fixture"),
        Some(ack_message_id),
        SpaceAdmissionBodyV1::Settled(
            AdmissionSettledV1::new([0x19; 32]).expect("non-zero ack digest fixture"),
        ),
    )
    .expect("valid Settled reply fixture");

    sponsor
        .settle_complete_ack(ack, [0x1a; 32], settled_reply)
        .expect("Applied Sponsor settles CompleteAck")
        .into_replacement()
}

fn sponsor_rejected_aggregate_fixture() -> SpaceAdmissionAggregate {
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
        AdmissionMessageId::from_bytes([0x1b; 32]).expect("non-zero message id fixture");
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
        AdmissionMessageId::from_bytes([0x1c; 32]).expect("non-zero message id fixture"),
        Some(cancel_message_id),
        SpaceAdmissionBodyV1::Rejected {
            reason: SpaceAdmissionRejectionReason::Cancelled,
        },
    )
    .expect("valid Rejected fixture");

    sponsor
        .reject_cancel(cancel, [0x1d; 32], rejected_reply)
        .expect("Candidate Sponsor rejects cancellation")
        .into_replacement()
}

fn joiner_rejected_aggregate_fixture() -> SpaceAdmissionAggregate {
    let cancelling = cancelling_joiner_aggregate_fixture();
    let cancel_message_id = match cancelling.state() {
        SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Cancelling(state)) => state
            .pending_exchange()
            .request_envelope()
            .header()
            .message_id(),
        _ => panic!("fixture must be Cancelling Joiner"),
    };
    let rejected = SpaceAdmissionEnvelopeV1::new(
        cancelling.admission_id(),
        AdmissionRole::Sponsor,
        1,
        AdmissionMessageId::from_bytes([0x1e; 32]).expect("non-zero message id fixture"),
        Some(cancel_message_id),
        SpaceAdmissionBodyV1::Rejected {
            reason: SpaceAdmissionRejectionReason::Cancelled,
        },
    )
    .expect("valid Rejected fixture");

    cancelling
        .accept_rejection(rejected, [0x1f; 32])
        .expect("Cancelling Joiner accepts Rejected")
        .into_replacement()
}

#[test]
fn persistence_round_trips_remaining_joiner_states() {
    assert_admission_persistence_round_trip(joiner_committed_aggregate_fixture());
    assert_admission_persistence_round_trip(joiner_applied_aggregate_fixture());
    assert_admission_persistence_round_trip(joiner_activating_aggregate_fixture());
    assert_admission_persistence_round_trip(cancelling_joiner_aggregate_fixture());
}

#[test]
fn persistence_round_trips_remaining_sponsor_and_helper_states() {
    assert_admission_persistence_round_trip(sponsor_committed_aggregate_fixture());
    assert_admission_persistence_round_trip(sponsor_applied_aggregate_fixture());
    assert_admission_persistence_round_trip(challenged_helper_with_counter(7));
    assert_admission_persistence_round_trip(completion_helper_applied_fixture());
}

#[test]
fn persistence_round_trips_active_and_completed_states() {
    assert_admission_persistence_round_trip(active_pending_settlement_aggregate_fixture());
    assert_admission_persistence_round_trip(active_settled_aggregate_fixture());
    assert_admission_persistence_round_trip(completed_aggregate_fixture());
}

#[test]
fn persistence_round_trips_superseded_rejected_and_recovery_states() {
    assert_admission_persistence_round_trip(
        initiated_joiner_aggregate_fixture()
            .supersede()
            .expect("Initiated Joiner can be superseded")
            .into_replacement(),
    );
    assert_admission_persistence_round_trip(
        initiated_joiner_aggregate_fixture()
            .with_authenticated_channel(
                AdmissionPeerBinding::new(
                    AdmissionChannelPeerId::from_bytes([0x20; 32])
                        .expect("non-zero local peer fixture"),
                    AdmissionChannelPeerId::from_bytes([0x21; 32])
                        .expect("non-zero remote peer fixture"),
                )
                .expect("distinct peer binding fixture"),
                AdmissionContinuationCredential::from_bytes(vec![0x22; 64])
                    .expect("bounded continuation credential fixture"),
            )
            .expect("authenticated Initiated fixture")
            .into_replacement()
            .supersede()
            .expect("Authenticated Initiated Joiner can be superseded")
            .into_replacement(),
    );
    assert_admission_persistence_round_trip(
        joiner_candidate_aggregate_fixture()
            .supersede()
            .expect("Candidate Joiner can be superseded")
            .into_replacement(),
    );
    assert_admission_persistence_round_trip(
        initiated_joiner_aggregate_fixture()
            .cancel_before_authentication()
            .expect("Initiated Joiner can cancel locally")
            .into_replacement(),
    );
    assert_admission_persistence_round_trip(joiner_rejected_aggregate_fixture());
    assert_admission_persistence_round_trip(sponsor_rejected_aggregate_fixture());
    assert_admission_persistence_round_trip(
        joiner_candidate_aggregate_fixture()
            .require_recovery(AdmissionRecoveryCategory::SpaceActivation)
            .expect("Candidate Joiner can require recovery")
            .into_replacement(),
    );
}
