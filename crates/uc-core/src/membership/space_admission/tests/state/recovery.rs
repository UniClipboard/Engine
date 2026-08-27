#[test]
fn recovery_required_terminal_blocks_further_protocol_progress() {
    let recovery = joiner_candidate_aggregate_fixture()
        .require_recovery(AdmissionRecoveryCategory::ProtocolConflict)
        .expect("Candidate can fail closed")
        .into_replacement();

    let state = match recovery.state() {
        SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::RecoveryRequired(
            state,
        )) => state,
        _ => panic!("admission must enter RecoveryRequired"),
    };
    assert_eq!(
        state.category(),
        AdmissionRecoveryCategory::ProtocolConflict
    );
    assert_eq!(
        recovery.supersede(),
        Err(SpaceAdmissionAggregateError::UnsafeSupersession)
    );
}

#[test]
fn authenticated_initiated_joiner_accepts_stable_rejection() {
    let initiated = initiated_joiner_aggregate_fixture()
        .with_authenticated_channel(
            AdmissionPeerBinding::new(
                AdmissionChannelPeerId::from_bytes([0xde; 32])
                    .expect("non-zero local peer fixture"),
                AdmissionChannelPeerId::from_bytes([0xdf; 32])
                    .expect("non-zero remote peer fixture"),
            )
            .expect("distinct peer binding fixture"),
            AdmissionContinuationCredential::from_bytes(vec![0xe0; 64])
                .expect("bounded continuation credential fixture"),
        )
        .expect("authenticated Initiated fixture")
        .into_replacement();
    let join_request_id = match initiated.state() {
        SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Initiated(state)) => state
            .pending_exchange()
            .request_envelope()
            .header()
            .message_id(),
        _ => panic!("fixture must be Initiated Joiner"),
    };
    let rejected = SpaceAdmissionEnvelopeV1::new(
        initiated.admission_id(),
        AdmissionRole::Sponsor,
        0,
        AdmissionMessageId::from_bytes([0xe1; 32]).expect("non-zero message id fixture"),
        Some(join_request_id),
        SpaceAdmissionBodyV1::Rejected {
            reason: SpaceAdmissionRejectionReason::AuthenticationRejected,
        },
    )
    .expect("valid initial Rejected fixture");

    let rejected = initiated
        .accept_rejection(rejected, [0xe2; 32])
        .expect("Initiated Joiner accepts stable Rejected");

    let state = match rejected.state() {
        SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Rejected(
            SpaceAdmissionRejectedState::Joiner(state),
        )) => state,
        _ => panic!("Joiner must advance to Rejected"),
    };
    assert_eq!(
        state.reason(),
        SpaceAdmissionRejectionReason::AuthenticationRejected
    );
}

#[test]
fn aggregate_exposes_exact_reply_without_revealing_internal_stage() {
    let prepared = joiner_prepared_aggregate_fixture();
    assert!(!prepared.is_terminal());
    assert_eq!(
        prepared
            .current_exact_reply()
            .expect("Prepared has exact reply")
            .kind(),
        SpaceAdmissionMessageKind::Prepared
    );

    let completed = sponsor_applied_aggregate_fixture();
    assert_eq!(
        completed
            .current_exact_reply()
            .expect("Applied Sponsor has exact reply")
            .kind(),
        SpaceAdmissionMessageKind::Complete
    );

    let terminal = initiated_joiner_aggregate_fixture()
        .supersede()
        .expect("Initiated can be superseded");
    assert!(terminal.is_terminal());
    assert!(terminal.current_exact_reply().is_none());
}

#[test]
fn aggregate_exposes_pending_exchange_without_revealing_internal_stage() {
    assert_eq!(
        initiated_joiner_aggregate_fixture()
            .pending_exchange()
            .expect("Initiated has JoinRequest")
            .request_envelope()
            .kind(),
        SpaceAdmissionMessageKind::JoinRequest
    );
    assert_eq!(
        joiner_prepared_aggregate_fixture()
            .pending_exchange()
            .expect("Prepared has request")
            .request_envelope()
            .kind(),
        SpaceAdmissionMessageKind::Prepared
    );
    assert!(joiner_committed_aggregate_fixture()
        .pending_exchange()
        .is_none());
}

#[test]
fn internal_transition_errors_map_to_stable_core_categories() {
    assert_eq!(
        SpaceAdmissionAggregateError::InvalidPreparedRequest.category(),
        AdmissionErrorCategory::Invalid
    );
    assert_eq!(
        SpaceAdmissionAggregateError::InvalidTransition.category(),
        AdmissionErrorCategory::OutOfOrder
    );
    assert_eq!(
        SpaceAdmissionAggregateError::AdmissionMismatch.category(),
        AdmissionErrorCategory::Conflict
    );
    assert_eq!(
        SpaceAdmissionAggregateError::TooLateCommitted.category(),
        AdmissionErrorCategory::UnsafeCancellation
    );
    assert_eq!(
        SpaceAdmissionAggregateError::CounterOverflow.category(),
        AdmissionErrorCategory::RecoveryRequired
    );
    assert_eq!(
        AdmissionReplayError::Conflict.category(),
        AdmissionErrorCategory::Conflict
    );
}
