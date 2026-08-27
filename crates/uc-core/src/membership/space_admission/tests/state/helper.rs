fn completion_helper_applied_fixture() -> SpaceAdmissionAggregate {
    let admission_id =
        SpaceAdmissionId::from_bytes([0xc3; 32]).expect("non-zero admission id fixture");
    let helper = SpaceAdmissionAggregate::challenge_completion_helper(
        admission_id,
        AdmissionPeerBinding::new(
            AdmissionChannelPeerId::from_bytes([0xc4; 32]).expect("non-zero local peer fixture"),
            AdmissionChannelPeerId::from_bytes([0xc5; 32]).expect("non-zero remote peer fixture"),
        )
        .expect("distinct peer binding fixture"),
        AdmissionContinuationCredential::from_bytes(vec![0xc6; 64])
            .expect("bounded continuation credential fixture"),
        1,
        crate::membership::AdmissionHelperNonce::from_bytes([0xc7; 32])
            .expect("non-zero helper nonce fixture"),
        AdmissionMessageId::from_bytes([0xc8; 32]).expect("non-zero message id fixture"),
        AdmissionMessageId::from_bytes([0xc9; 32]).expect("non-zero message id fixture"),
    )
    .expect("valid challenged helper fixture")
    .into_replacement();
    let candidate = candidate_body_fixture();
    let event_id = candidate.candidate_event().event_id();
    let security_commitment_id = candidate.security_commitment().security_commitment_id;
    let verified_commit = SpaceAdmissionEnvelopeV1::new(
        admission_id,
        AdmissionRole::Sponsor,
        1,
        AdmissionMessageId::from_bytes([0xca; 32]).expect("non-zero message id fixture"),
        Some(AdmissionMessageId::from_bytes([0xc8; 32]).expect("non-zero predecessor fixture")),
        SpaceAdmissionBodyV1::Commit(AdmissionCommitV1::new(
            candidate,
            AdmissionSignedMembershipHistory::from_bytes(vec![0xcb; 128])
                .expect("bounded verified history fixture"),
            AdmissionSealedRecoveryMaterial::from_bytes(vec![0xcc; 128])
                .expect("bounded sealed recovery fixture"),
        )),
    )
    .expect("valid verified Commit fixture");
    let receipt = AdmissionActivationReceipt::new(
        1,
        *admission_id.as_bytes(),
        event_id,
        [0xcd; 32],
        security_commitment_id,
        MemberInstanceId::from_bytes([0xce; 32]),
        vec![0xcf; 64],
    );
    let inbound = AdmissionMessageEvidence::new(
        AdmissionRole::Joiner,
        3,
        AdmissionMessageId::from_bytes([0xd0; 32]).expect("non-zero message id fixture"),
        Some(AdmissionMessageId::from_bytes([0xc9; 32]).expect("non-zero predecessor fixture")),
        [0xd1; 32],
    )
    .expect("non-zero helper request evidence fixture");
    let complete_reply = SpaceAdmissionEnvelopeV1::new(
        admission_id,
        AdmissionRole::CompletionHelper,
        0,
        AdmissionMessageId::from_bytes([0xd2; 32]).expect("non-zero message id fixture"),
        Some(inbound.message_id()),
        SpaceAdmissionBodyV1::Complete(AdmissionCompleteV1::new(AdmissionCompletionV1::new(
            *admission_id.as_bytes(),
            event_id,
            [0xd3; 32],
            security_commitment_id,
            MemberInstanceId::from_bytes([0xd4; 32]),
            MembershipCredential::new(1, vec![0xd5; 32]).credential_id,
            BaseMembershipHistoryPosition {
                event_id: Some(event_id),
                depth: 1,
                history_digest: [0xd6; 32],
            },
            vec![0xd7; 64],
        ))),
    )
    .expect("valid helper Complete fixture");

    helper
        .complete_as_helper(
            inbound,
            verified_commit,
            receipt,
            crate::membership::AdmissionHelperSecurityState::from_bytes(vec![0xd8; 128])
                .expect("bounded helper security fixture"),
            complete_reply,
        )
        .expect("challenged helper completes verified admission")
        .into_replacement()
}

#[test]
fn completion_helper_can_only_complete_verified_applied_admission() {
    let applied = completion_helper_applied_fixture();
    assert!(matches!(
        applied.state(),
        SpaceAdmissionRecordState::CompletionHelper(SpaceAdmissionCompletionHelperState::Applied(
            _
        ))
    ));
}

#[test]
fn completion_helper_can_settle_its_exact_complete_reply() {
    let helper = completion_helper_applied_fixture();
    let complete_message_id = match helper.state() {
        SpaceAdmissionRecordState::CompletionHelper(
            SpaceAdmissionCompletionHelperState::Applied(state),
        ) => state
            .saved_reply()
            .exact_reply_envelope()
            .header()
            .message_id(),
        _ => panic!("fixture must be Applied CompletionHelper"),
    };
    let ack_message_id =
        AdmissionMessageId::from_bytes([0xd9; 32]).expect("non-zero message id fixture");
    let ack = SpaceAdmissionEnvelopeV1::new(
        helper.admission_id(),
        AdmissionRole::Joiner,
        3,
        ack_message_id,
        Some(complete_message_id),
        SpaceAdmissionBodyV1::CompleteAck(
            AdmissionCompleteAckV1::new([0xda; 32]).expect("non-zero completion digest fixture"),
        ),
    )
    .expect("valid CompleteAck fixture");
    let settled_reply = SpaceAdmissionEnvelopeV1::new(
        helper.admission_id(),
        AdmissionRole::CompletionHelper,
        1,
        AdmissionMessageId::from_bytes([0xdb; 32]).expect("non-zero message id fixture"),
        Some(ack_message_id),
        SpaceAdmissionBodyV1::Settled(
            AdmissionSettledV1::new([0xdc; 32]).expect("non-zero ack digest fixture"),
        ),
    )
    .expect("valid helper Settled fixture");

    let completed = helper
        .settle_complete_ack(ack, [0xdd; 32], settled_reply)
        .expect("CompletionHelper settles CompleteAck");

    assert!(matches!(
        completed.state(),
        SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Completed(_))
    ));
}

fn challenged_helper_with_counter(counter: u64) -> SpaceAdmissionAggregate {
    SpaceAdmissionAggregate::challenge_completion_helper(
        SpaceAdmissionId::from_bytes([0xe3; 32]).expect("non-zero admission id fixture"),
        AdmissionPeerBinding::new(
            AdmissionChannelPeerId::from_bytes([0xe4; 32]).expect("non-zero local peer fixture"),
            AdmissionChannelPeerId::from_bytes([0xe5; 32]).expect("non-zero remote peer fixture"),
        )
        .expect("distinct peer binding fixture"),
        AdmissionContinuationCredential::from_bytes(vec![0xe6; 64])
            .expect("bounded continuation credential fixture"),
        counter,
        crate::membership::AdmissionHelperNonce::from_bytes([0xe7; 32])
            .expect("non-zero helper nonce fixture"),
        AdmissionMessageId::from_bytes([0xe8; 32]).expect("non-zero message id fixture"),
        AdmissionMessageId::from_bytes([0xe9; 32]).expect("non-zero message id fixture"),
    )
    .expect("valid challenged helper fixture")
    .into_replacement()
}

#[test]
fn completion_helper_counter_advances_once_and_fails_on_overflow() {
    let advanced = challenged_helper_with_counter(1)
        .advance_helper_challenge(
            crate::membership::AdmissionHelperNonce::from_bytes([0xea; 32])
                .expect("non-zero helper nonce fixture"),
        )
        .expect("helper counter advances");
    let state = match advanced.state() {
        SpaceAdmissionRecordState::CompletionHelper(
            SpaceAdmissionCompletionHelperState::Challenged(state),
        ) => state,
        _ => panic!("helper must remain Challenged"),
    };
    assert_eq!(state.challenge_counter(), 2);

    assert_eq!(
        challenged_helper_with_counter(u64::MAX).advance_helper_challenge(
            crate::membership::AdmissionHelperNonce::from_bytes([0xeb; 32])
                .expect("non-zero helper nonce fixture"),
        ),
        Err(SpaceAdmissionAggregateError::CounterOverflow)
    );
}

#[test]
fn completion_helper_cannot_execute_joiner_supersession() {
    assert_eq!(
        challenged_helper_with_counter(1).supersede(),
        Err(SpaceAdmissionAggregateError::UnsafeSupersession)
    );
}
