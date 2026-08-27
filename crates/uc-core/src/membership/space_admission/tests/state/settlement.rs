#[test]
fn joiner_applied_accepts_complete_before_space_activation() {
    let applied = joiner_applied_aggregate_fixture();
    let (applied_message_id, event_id, security_commitment_id) = match applied.state() {
        SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Applied(state)) => {
            let SpaceAdmissionBodyV1::Applied(applied) =
                state.pending_exchange().request_envelope().body()
            else {
                panic!("Applied Joiner must hold Applied request");
            };
            (
                state
                    .pending_exchange()
                    .request_envelope()
                    .header()
                    .message_id(),
                applied.activation_receipt().event_id,
                applied
                    .activation_receipt()
                    .installed_security_commitment_id,
            )
        }
        _ => panic!("fixture must be Applied Joiner"),
    };
    let completion = AdmissionCompletionV1::new(
        *applied.admission_id().as_bytes(),
        event_id,
        [0x87; 32],
        security_commitment_id,
        MemberInstanceId::from_bytes([0x88; 32]),
        MembershipCredential::new(1, vec![0x89; 32]).credential_id,
        BaseMembershipHistoryPosition {
            event_id: Some(event_id),
            depth: 1,
            history_digest: [0x8a; 32],
        },
        vec![0x8b; 64],
    );
    let complete = SpaceAdmissionEnvelopeV1::new(
        applied.admission_id(),
        AdmissionRole::Sponsor,
        2,
        AdmissionMessageId::from_bytes([0x8c; 32]).expect("non-zero message id fixture"),
        Some(applied_message_id),
        SpaceAdmissionBodyV1::Complete(AdmissionCompleteV1::new(completion)),
    )
    .expect("valid Complete fixture");

    let activating = applied
        .accept_complete(
            complete,
            [0x8d; 32],
            crate::membership::AdmissionSpaceTransition::from_bytes(vec![0x8e; 128])
                .expect("bounded Space transition fixture"),
        )
        .expect("Applied Joiner accepts Complete");

    assert_eq!(activating.effects(), &[AdmissionEffect::ActivateSpace]);
    assert_eq!(activating.record_version(), 6);
    let state = match activating.state() {
        SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Activating(state)) => state,
        _ => panic!("Joiner must advance to Activating"),
    };
    assert_eq!(
        state.completion().kind(),
        SpaceAdmissionMessageKind::Complete
    );
    assert_eq!(state.completion_evidence().canonical_digest(), &[0x8d; 32]);
}

fn joiner_activating_aggregate_fixture() -> SpaceAdmissionAggregate {
    let applied = joiner_applied_aggregate_fixture();
    let (applied_message_id, event_id, security_commitment_id) = match applied.state() {
        SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Applied(state)) => {
            let SpaceAdmissionBodyV1::Applied(applied) =
                state.pending_exchange().request_envelope().body()
            else {
                panic!("Applied Joiner must hold Applied request");
            };
            (
                state
                    .pending_exchange()
                    .request_envelope()
                    .header()
                    .message_id(),
                applied.activation_receipt().event_id,
                applied
                    .activation_receipt()
                    .installed_security_commitment_id,
            )
        }
        _ => panic!("fixture must be Applied Joiner"),
    };
    let completion = AdmissionCompletionV1::new(
        *applied.admission_id().as_bytes(),
        event_id,
        [0x8f; 32],
        security_commitment_id,
        MemberInstanceId::from_bytes([0x90; 32]),
        MembershipCredential::new(1, vec![0x91; 32]).credential_id,
        BaseMembershipHistoryPosition {
            event_id: Some(event_id),
            depth: 1,
            history_digest: [0x92; 32],
        },
        vec![0x93; 64],
    );
    let complete = SpaceAdmissionEnvelopeV1::new(
        applied.admission_id(),
        AdmissionRole::Sponsor,
        2,
        AdmissionMessageId::from_bytes([0x94; 32]).expect("non-zero message id fixture"),
        Some(applied_message_id),
        SpaceAdmissionBodyV1::Complete(AdmissionCompleteV1::new(completion)),
    )
    .expect("valid Complete fixture");

    applied
        .accept_complete(
            complete,
            [0x95; 32],
            crate::membership::AdmissionSpaceTransition::from_bytes(vec![0x96; 128])
                .expect("bounded Space transition fixture"),
        )
        .expect("Activating Joiner fixture")
        .into_replacement()
}

fn active_pending_settlement_aggregate_fixture() -> SpaceAdmissionAggregate {
    let activating = joiner_activating_aggregate_fixture();
    let completion_message_id = match activating.state() {
        SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Activating(state)) => {
            state.completion().header().message_id()
        }
        _ => panic!("fixture must be Activating Joiner"),
    };
    let complete_ack = SpaceAdmissionEnvelopeV1::new(
        activating.admission_id(),
        AdmissionRole::Joiner,
        3,
        AdmissionMessageId::from_bytes([0xac; 32]).expect("non-zero message id fixture"),
        Some(completion_message_id),
        SpaceAdmissionBodyV1::CompleteAck(
            AdmissionCompleteAckV1::new([0xad; 32]).expect("non-zero completion digest fixture"),
        ),
    )
    .expect("valid CompleteAck fixture");
    activating
        .activate_complete(
            crate::membership::AdmissionSpaceTransitionResult::from_bytes(vec![0xae; 128])
                .expect("bounded Space transition result fixture"),
            PendingAdmissionExchange::new(
                SpaceAdmissionRoute::from_bytes(vec![0xaf; 32]).expect("bounded route fixture"),
                complete_ack,
                SpaceAdmissionMessageKind::Settled,
                AdmissionRetryState::new(0, 0).expect("valid retry state"),
            )
            .expect("CompleteAck expects Settled"),
        )
        .expect("Active pending settlement fixture")
        .into_replacement()
}

#[test]
fn joiner_becomes_active_only_after_space_transition_completes() {
    let activating = joiner_activating_aggregate_fixture();
    let completion_message_id = match activating.state() {
        SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Activating(state)) => {
            state.completion().header().message_id()
        }
        _ => panic!("fixture must be Activating Joiner"),
    };
    let complete_ack = SpaceAdmissionEnvelopeV1::new(
        activating.admission_id(),
        AdmissionRole::Joiner,
        3,
        AdmissionMessageId::from_bytes([0x97; 32]).expect("non-zero message id fixture"),
        Some(completion_message_id),
        SpaceAdmissionBodyV1::CompleteAck(
            AdmissionCompleteAckV1::new([0x98; 32]).expect("non-zero completion digest fixture"),
        ),
    )
    .expect("valid CompleteAck fixture");
    let pending_exchange = PendingAdmissionExchange::new(
        SpaceAdmissionRoute::from_bytes(vec![0x99; 32]).expect("bounded route fixture"),
        complete_ack,
        SpaceAdmissionMessageKind::Settled,
        AdmissionRetryState::new(0, 0).expect("valid retry state"),
    )
    .expect("CompleteAck expects Settled");

    let active = activating
        .activate_complete(
            crate::membership::AdmissionSpaceTransitionResult::from_bytes(vec![0x9a; 128])
                .expect("bounded Space transition result fixture"),
            pending_exchange,
        )
        .expect("Activating Joiner becomes Active");

    assert_eq!(active.effects(), &[AdmissionEffect::PublishActive]);
    assert_eq!(active.record_version(), 7);
    let state = match active.state() {
        SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Active(
            SpaceAdmissionActiveState::PendingSettlement(state),
        )) => state,
        _ => panic!("Joiner must become Active pending settlement"),
    };
    assert_eq!(
        state.pending_exchange().request_envelope().kind(),
        SpaceAdmissionMessageKind::CompleteAck
    );
}

#[test]
fn sponsor_settles_complete_ack_with_exact_settled_reply() {
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
        AdmissionMessageId::from_bytes([0xa7; 32]).expect("non-zero message id fixture");
    let ack = SpaceAdmissionEnvelopeV1::new(
        sponsor.admission_id(),
        AdmissionRole::Joiner,
        3,
        ack_message_id,
        Some(complete_message_id),
        SpaceAdmissionBodyV1::CompleteAck(
            AdmissionCompleteAckV1::new([0xa8; 32]).expect("non-zero completion digest fixture"),
        ),
    )
    .expect("valid CompleteAck fixture");
    let settled_reply = SpaceAdmissionEnvelopeV1::new(
        sponsor.admission_id(),
        AdmissionRole::Sponsor,
        3,
        AdmissionMessageId::from_bytes([0xa9; 32]).expect("non-zero message id fixture"),
        Some(ack_message_id),
        SpaceAdmissionBodyV1::Settled(
            AdmissionSettledV1::new([0xaa; 32]).expect("non-zero ack digest fixture"),
        ),
    )
    .expect("valid Settled reply fixture");

    let completed = sponsor
        .settle_complete_ack(ack, [0xab; 32], settled_reply)
        .expect("Applied Sponsor settles CompleteAck");

    assert_eq!(completed.record_version(), 4);
    let state = match completed.state() {
        SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Completed(state)) => state,
        _ => panic!("Sponsor must advance to Completed"),
    };
    assert_eq!(
        state.saved_reply().exact_reply_envelope().kind(),
        SpaceAdmissionMessageKind::Settled
    );
}

#[test]
fn joiner_compacts_active_terminal_after_settled() {
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
        AdmissionMessageId::from_bytes([0xb0; 32]).expect("non-zero message id fixture"),
        Some(ack_message_id),
        SpaceAdmissionBodyV1::Settled(
            AdmissionSettledV1::new([0xb1; 32]).expect("non-zero ack digest fixture"),
        ),
    )
    .expect("valid Settled fixture");

    let settled = active
        .accept_settled(settled, [0xb2; 32])
        .expect("Active Joiner accepts Settled");

    assert_eq!(settled.record_version(), 8);
    let state = match settled.state() {
        SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Active(
            SpaceAdmissionActiveState::Settled(state),
        )) => state,
        _ => panic!("Joiner must become compacted Active"),
    };
    assert_eq!(state.last_received().canonical_digest(), &[0xb2; 32]);
}
