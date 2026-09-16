#[test]
fn initiated_joiner_exposes_one_complete_initial_recovery_view() {
    let aggregate = initiated_joiner_aggregate_fixture();

    let recovery = aggregate
        .pending_recovery()
        .expect("Initiated Joiner requires an initial authenticated exchange");

    let AdmissionPendingRecovery::Initial {
        encrypted_password_equivalent,
        pending_exchange,
    } = recovery
    else {
        panic!("unauthenticated Joiner must require initial recovery");
    };
    assert_eq!(encrypted_password_equivalent.as_bytes(), &[0xf8; 32]);
    assert_eq!(pending_exchange.route().as_bytes(), &[0xf9; 32]);
    assert_eq!(
        pending_exchange.request_envelope().kind(),
        SpaceAdmissionMessageKind::JoinRequest
    );
}

#[test]
fn stable_initial_authentication_failure_is_saved_as_local_rejection() {
    let rejected = initiated_joiner_aggregate_fixture()
        .reject_before_authentication(SpaceAdmissionRejectionReason::AuthenticationRejected)
        .expect("an unauthenticated Joiner can save a stable authentication rejection");

    assert_eq!(rejected.record_version(), 1);
    assert!(rejected.is_terminal());
}

#[test]
fn authenticated_joiner_exposes_continuation_recovery_for_the_same_request() {
    let aggregate = initiated_joiner_aggregate_fixture()
        .with_authenticated_channel(
            AdmissionPeerBinding::new(
                AdmissionChannelPeerId::from_bytes([0x35; 32]).expect("valid local peer"),
                AdmissionChannelPeerId::from_bytes([0x36; 32]).expect("valid remote peer"),
            )
            .expect("distinct peers"),
            AdmissionContinuationCredential::from_bytes(vec![0x37; 64])
                .expect("valid continuation credential"),
        )
        .expect("initial authentication transition")
        .into_replacement();

    let AdmissionPendingRecovery::Continuation {
        peer_binding,
        continuation_credential,
        pending_exchange,
    } = aggregate
        .pending_recovery()
        .expect("authenticated Joiner must resume the same exchange")
    else {
        panic!("authenticated Joiner must not repeat initial authentication");
    };
    assert_eq!(peer_binding.local_peer_id().as_bytes(), &[0x35; 32]);
    assert_eq!(continuation_credential.as_bytes(), &[0x37; 64]);
    assert_eq!(pending_exchange.route().as_bytes(), &[0xf9; 32]);
    assert_eq!(
        pending_exchange
            .request_envelope()
            .header()
            .message_id()
            .as_bytes(),
        &[0xf5; 32]
    );
}

#[test]
fn authenticated_initial_join_can_persist_an_explicit_peer_upgrade_rejection() {
    let authenticated = initiated_joiner_aggregate_fixture()
        .with_authenticated_channel(
            AdmissionPeerBinding::new(
                AdmissionChannelPeerId::from_bytes([0x35; 32]).expect("valid local peer"),
                AdmissionChannelPeerId::from_bytes([0x36; 32]).expect("valid remote peer"),
            )
            .expect("distinct peers"),
            AdmissionContinuationCredential::from_bytes(vec![0x37; 64])
                .expect("valid continuation credential"),
        )
        .expect("initial authentication transition")
        .into_replacement();

    let rejected = authenticated
        .reject_peer_upgrade()
        .expect("authenticated initial join can report an incompatible peer")
        .into_replacement();

    assert!(rejected.is_terminal());
    let SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Rejected(
        SpaceAdmissionRejectedState::LocalJoiner(rejected),
    )) = rejected.state()
    else {
        panic!("peer upgrade must become a local Joiner rejection");
    };
    assert_eq!(
        rejected.reason(),
        SpaceAdmissionRejectionReason::PeerUpgradeRequired
    );
}

#[test]
fn later_joiner_recovery_states_preserve_the_exact_exchange_when_peer_upgrade_is_required() {
    for (name, aggregate, request_kind) in [
        (
            "prepared",
            joiner_prepared_aggregate_fixture(),
            SpaceAdmissionMessageKind::Prepared,
        ),
        (
            "applied",
            joiner_applied_aggregate_fixture(),
            SpaceAdmissionMessageKind::Applied,
        ),
        (
            "cancelling",
            cancelling_joiner_aggregate_fixture(),
            SpaceAdmissionMessageKind::CancelRequested,
        ),
        (
            "active pending settlement",
            active_pending_settlement_aggregate_fixture(),
            SpaceAdmissionMessageKind::CompleteAck,
        ),
    ] {
        let original_version = aggregate.record_version();
        let original_exchange = aggregate
            .pending_exchange()
            .expect("fixture must contain a pending exchange");
        let original_route = original_exchange.route().as_bytes().to_vec();
        let original_request = original_exchange
            .request_envelope()
            .encode_canonical_v1()
            .expect("fixture request must encode");
        let original_retry = *original_exchange.retry_state();

        let transition = aggregate
            .mark_peer_upgrade_required()
            .unwrap_or_else(|error| panic!("{name} must preserve a peer-upgrade block: {error}"));

        assert!(transition.effects().is_empty(), "{name}");
        let blocked = transition.into_replacement();
        assert_eq!(blocked.record_version(), original_version + 1, "{name}");
        assert!(blocked.peer_upgrade_required(), "{name}");
        let blocked_exchange = blocked
            .pending_exchange()
            .expect("blocked state must retain its pending exchange");
        assert_eq!(
            blocked_exchange.request_envelope().kind(),
            request_kind,
            "{name}"
        );
        assert_eq!(blocked_exchange.route().as_bytes(), original_route, "{name}");
        assert_eq!(
            blocked_exchange
                .request_envelope()
                .encode_canonical_v1()
                .expect("blocked request must encode"),
            original_request,
            "{name}"
        );
        assert_eq!(*blocked_exchange.retry_state(), original_retry, "{name}");
        assert_eq!(
            blocked_exchange.block_reason(),
            Some(AdmissionExchangeBlockReason::PeerUpgradeRequired),
            "{name}"
        );
        assert_admission_persistence_round_trip(blocked);
    }
}

#[test]
fn successful_settlement_clears_the_saved_peer_upgrade_block() {
    let blocked = active_pending_settlement_aggregate_fixture()
        .mark_peer_upgrade_required()
        .expect("active settlement may wait for a peer upgrade")
        .into_replacement();
    let complete_ack_id = blocked
        .pending_exchange()
        .expect("active settlement keeps CompleteAck")
        .request_envelope()
        .header()
        .message_id();
    let settled = SpaceAdmissionEnvelopeV1::new(
        blocked.admission_id(),
        AdmissionRole::Sponsor,
        3,
        AdmissionMessageId::from_bytes([0xd8; 32]).expect("non-zero settled message id"),
        Some(complete_ack_id),
        SpaceAdmissionBodyV1::Settled(
            AdmissionSettledV1::new([0xd9; 32]).expect("non-zero acknowledgement digest"),
        ),
    )
    .expect("valid Settled reply");

    let completed = blocked
        .accept_settled(settled, [0xda; 32])
        .expect("the exact reply must resume a blocked settlement")
        .into_replacement();

    assert!(completed.is_active_settled());
    assert!(!completed.peer_upgrade_required());
    assert!(completed.pending_exchange().is_none());
}

#[test]
fn cancelling_a_blocked_prepared_join_keeps_the_upgrade_prompt() {
    let blocked = joiner_prepared_aggregate_fixture()
        .mark_peer_upgrade_required()
        .expect("prepared join may wait for a peer upgrade")
        .into_replacement();
    let cancelling = blocked
        .request_cancel(
            AdmissionMessageId::from_bytes([0xdb; 32]).expect("non-zero cancel message id"),
            AdmissionRetryState::new(0, 0).expect("initial retry state"),
        )
        .expect("blocked prepared join may still be cancelled")
        .into_replacement();

    assert!(matches!(
        cancelling.state(),
        SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Cancelling(_))
    ));
    assert!(cancelling.peer_upgrade_required());
    assert_admission_persistence_round_trip(cancelling);
}

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
