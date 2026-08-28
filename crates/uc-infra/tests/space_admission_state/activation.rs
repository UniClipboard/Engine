use std::error::Error as _;
use uc_application::deps::{
    JoinerActivationMutation, JoinerActivationStateError, JoinerActivationStatePort,
};
use uc_core::membership::{
    AdmissionActivationReceipt, AdmissionAppliedV1, AdmissionCandidateV1, AdmissionChangeFacts,
    AdmissionCommitV1, AdmissionCompleteAckV1, AdmissionCompleteV1, AdmissionCompletionV1,
    AdmissionContinuationRoute, AdmissionMlsCommit, AdmissionMlsWelcome, AdmissionPreparedV1,
    AdmissionSealedRecoveryMaterial, AdmissionSecurityCommitmentV1,
    AdmissionSignedMembershipHistory, AdmissionSpaceTransition, AdmissionSpaceTransitionResult,
    AdmissionStagedTarget, AdmissionStagedTargetInput, BaseMembershipHistoryPosition,
    JoinerAdmission, MemberInstanceId, MembershipAdmissionV2, MembershipEventV2,
    MembershipOperationV2, PreparedAdmissionProofV1, ADMISSION_SECURITY_COMMITMENT_FORMAT_V1,
    ED25519_SIGNATURE_ALGORITHM_V1, MEMBERSHIP_EVENT_FORMAT_V2,
};
use uc_core::security::IdentityFingerprint;

use super::*;

#[tokio::test]
async fn activation_load_returns_none_without_an_activating_join() {
    let fixture = Fixture::new();

    let loaded = JoinerActivationStatePort::load(&fixture.store)
        .await
        .unwrap();

    assert!(loaded.is_none());
}

#[tokio::test]
async fn pending_join_is_not_loaded_as_an_activation() {
    let fixture = Fixture::new();
    commit_fresh_join(&fixture, 0xc8, 0xc9).await;

    let loaded = JoinerActivationStatePort::load(&fixture.store)
        .await
        .unwrap();

    assert!(loaded.is_none());
}

#[tokio::test]
async fn activating_join_reopens_and_commits_active_state() {
    let fixture = Fixture::new();
    commit_activating_join(&fixture).await;
    let reopened = fixture.reopen();
    let loaded = JoinerActivationStatePort::load(&reopened)
        .await
        .unwrap()
        .expect("Activating join must survive restart");
    let (joiner, token) = loaded.into_parts();
    let admission_id = joiner.admission_id();

    JoinerActivationStatePort::commit(&reopened, token, activation_mutation(joiner, 0xe1))
        .await
        .unwrap();

    assert!(JoinerActivationStatePort::load(&reopened)
        .await
        .unwrap()
        .is_none());
    let pending =
        PendingAdmissionRecoveryStatePort::load(&reopened, AdmissionRecoveryTrigger::StateChanged)
            .await
            .unwrap();
    assert_eq!(pending.len(), 1);
    let (active, _) = pending.into_iter().next().unwrap().into_parts();
    assert_eq!(active.admission_id(), admission_id);
    assert_eq!(
        active.current_exact_reply().map(|reply| reply.kind()),
        Some(SpaceAdmissionMessageKind::CompleteAck)
    );
}

#[tokio::test]
async fn stale_activation_token_cannot_commit_twice() {
    let fixture = Fixture::new();
    commit_activating_join(&fixture).await;
    let first = JoinerActivationStatePort::load(&fixture.store)
        .await
        .unwrap()
        .unwrap();
    let stale = JoinerActivationStatePort::load(&fixture.store)
        .await
        .unwrap()
        .unwrap();
    let (first_joiner, first_token) = first.into_parts();
    let (stale_joiner, stale_token) = stale.into_parts();

    JoinerActivationStatePort::commit(
        &fixture.store,
        first_token,
        activation_mutation(first_joiner, 0xe2),
    )
    .await
    .unwrap();
    let result = JoinerActivationStatePort::commit(
        &fixture.store,
        stale_token,
        activation_mutation(stale_joiner, 0xe3),
    )
    .await;

    let error = result.expect_err("stale activation token must be rejected");
    assert!(matches!(
        error,
        JoinerActivationStateError::StateChanged { .. }
    ));
    assert!(error.source().is_some());
}

#[tokio::test]
async fn activation_commit_rejects_a_non_activation_transition_before_persistence() {
    let fixture = Fixture::new();
    commit_activating_join(&fixture).await;
    let loaded = JoinerActivationStatePort::load(&fixture.store)
        .await
        .unwrap()
        .unwrap();
    let (_, token) = loaded.into_parts();
    let invalid = start_join_transition(
        0xc1,
        0xc2,
        9,
        AdmissionSourceSnapshot::from_bytes(vec![0xc3; 32]).unwrap(),
    );

    let result = JoinerActivationStatePort::commit(
        &fixture.store,
        token,
        JoinerActivationMutation::new(invalid),
    )
    .await;

    let error = result.expect_err("non-activation effects must be rejected");
    assert!(matches!(
        error,
        JoinerActivationStateError::RecoveryRequired { .. }
    ));
    assert!(error.source().is_some());
    assert!(JoinerActivationStatePort::load(&fixture.store)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn activation_commit_failure_keeps_the_saved_plan_for_retry() {
    let fixture = Fixture::new();
    commit_activating_join(&fixture).await;
    let loaded = JoinerActivationStatePort::load(&fixture.store)
        .await
        .unwrap()
        .unwrap();
    let (joiner, token) = loaded.into_parts();
    fixture.execute(
        "CREATE TRIGGER fail_activation_update BEFORE UPDATE ON admission_repository_state \
         BEGIN SELECT RAISE(ABORT, 'injected'); END",
    );

    let result =
        JoinerActivationStatePort::commit(&fixture.store, token, activation_mutation(joiner, 0xb1))
            .await;

    let error = result.expect_err("injected save failure must be returned");
    assert!(matches!(
        error,
        JoinerActivationStateError::Unavailable { .. }
    ));
    assert!(error.source().is_some());
    fixture.execute("DROP TRIGGER fail_activation_update");
    let loaded = JoinerActivationStatePort::load(&fixture.store)
        .await
        .unwrap()
        .expect("failed commit must preserve the Activating record");
    let (joiner, token) = loaded.into_parts();
    JoinerActivationStatePort::commit(&fixture.store, token, activation_mutation(joiner, 0xb5))
        .await
        .unwrap();
}

#[tokio::test]
async fn activation_plan_and_result_remain_encrypted() {
    let fixture = Fixture::new();
    commit_activating_join(&fixture).await;
    let loaded = JoinerActivationStatePort::load(&fixture.store)
        .await
        .unwrap()
        .unwrap();
    let (joiner, token) = loaded.into_parts();

    JoinerActivationStatePort::commit(&fixture.store, token, activation_mutation(joiner, 0xe4))
        .await
        .unwrap();
    let encrypted = fixture.encrypted_payload();

    assert!(!encrypted.windows(128).any(|window| window == [0xd9; 128]));
    assert!(!encrypted.windows(128).any(|window| window == [0xe4; 128]));
}

async fn commit_activating_join(fixture: &Fixture) {
    let loaded = JoinerStartStatePort::load(&fixture.store).await.unwrap();
    let (ordinal, snapshot, _, _, token) = loaded.into_parts();
    JoinerStartStatePort::commit(
        &fixture.store,
        token,
        JoinerStartMutation::new(start_join_transition(0xd0, 0xd1, ordinal, snapshot), None),
    )
    .await
    .unwrap();

    let loaded = load_one_pending(fixture).await;
    let (joiner, token) = loaded.into_parts();
    let join_request_id = joiner
        .pending_exchange()
        .unwrap()
        .request_envelope()
        .header()
        .message_id();
    let transition = joiner
        .with_authenticated_channel(peer_binding(), continuation())
        .unwrap();
    let loaded = PendingAdmissionRecoveryStatePort::commit(&fixture.store, token, transition)
        .await
        .unwrap();

    let (joiner, token) = loaded.into_parts();
    let candidate_id = uc_core::membership::AdmissionMessageId::from_bytes([0xd2; 32]).unwrap();
    let candidate = SpaceAdmissionEnvelopeV1::new(
        joiner.admission_id(),
        AdmissionRole::Sponsor,
        0,
        candidate_id,
        Some(join_request_id),
        SpaceAdmissionBodyV1::Candidate(candidate_body_fixture()),
    )
    .unwrap();
    let transition = joiner
        .accept_candidate(
            candidate,
            [0xd3; 32],
            AdmissionStagedTargetInput::from_bytes(vec![0xd4; 128]).unwrap(),
        )
        .unwrap();
    let loaded = PendingAdmissionRecoveryStatePort::commit(&fixture.store, token, transition)
        .await
        .unwrap();

    let (joiner, token) = loaded.into_parts();
    let admission_id = joiner.admission_id();
    let candidate_body = candidate_body_fixture();
    let prepared_id = uc_core::membership::AdmissionMessageId::from_bytes([0xd5; 32]).unwrap();
    let prepared = SpaceAdmissionEnvelopeV1::new(
        admission_id,
        AdmissionRole::Joiner,
        1,
        prepared_id,
        Some(candidate_id),
        SpaceAdmissionBodyV1::Prepared(AdmissionPreparedV1::new(PreparedAdmissionProofV1::new(
            *admission_id.as_bytes(),
            "lineage".to_owned(),
            BaseMembershipHistoryPosition {
                event_id: None,
                depth: 0,
                history_digest: [0xd6; 32],
            },
            candidate_body.candidate_event().event_id(),
            candidate_body.candidate_event().resulting_members_digest,
            candidate_body.security_commitment().security_commitment_id,
            MemberInstanceId::from_bytes([0xd7; 32]),
            MembershipCredential::new(1, vec![0xd8; 32]).credential_id,
            vec![0xda; 64],
        ))),
    )
    .unwrap();
    let prepared_exchange = PendingAdmissionExchange::new(
        SpaceAdmissionRoute::from_bytes(vec![0xdb; 32]).unwrap(),
        prepared,
        SpaceAdmissionMessageKind::Commit,
        AdmissionRetryState::new(0, 0).unwrap(),
    )
    .unwrap();
    let verified_history = AdmissionSignedMembershipHistory::from_bytes(vec![0xdc; 128]).unwrap();
    let transition = joiner
        .prepare_candidate(
            verified_history,
            AdmissionStagedTarget::from_bytes(vec![0xdd; 128]).unwrap(),
            prepared_exchange,
        )
        .unwrap();
    let loaded = PendingAdmissionRecoveryStatePort::commit(&fixture.store, token, transition)
        .await
        .unwrap();

    let (joiner, token) = loaded.into_parts();
    let commit_id = uc_core::membership::AdmissionMessageId::from_bytes([0xde; 32]).unwrap();
    let commit = SpaceAdmissionEnvelopeV1::new(
        admission_id,
        AdmissionRole::Sponsor,
        1,
        commit_id,
        Some(prepared_id),
        SpaceAdmissionBodyV1::Commit(AdmissionCommitV1::new(
            candidate_body_fixture(),
            AdmissionSignedMembershipHistory::from_bytes(vec![0xdc; 128]).unwrap(),
            AdmissionSealedRecoveryMaterial::from_bytes(vec![0xdf; 128]).unwrap(),
        )),
    )
    .unwrap();
    let transition = joiner.accept_commit(commit, [0xe0; 32]).unwrap();
    let loaded = PendingAdmissionRecoveryStatePort::commit(&fixture.store, token, transition)
        .await
        .unwrap();

    let (joiner, token) = loaded.into_parts();
    let candidate = candidate_body_fixture();
    let receipt = AdmissionActivationReceipt::new(
        1,
        *admission_id.as_bytes(),
        candidate.candidate_event().event_id(),
        [0xe5; 32],
        candidate.security_commitment().security_commitment_id,
        MemberInstanceId::from_bytes([0xe6; 32]),
        vec![0xe7; 64],
    );
    let applied_id = uc_core::membership::AdmissionMessageId::from_bytes([0xe8; 32]).unwrap();
    let applied = SpaceAdmissionEnvelopeV1::new(
        admission_id,
        AdmissionRole::Joiner,
        2,
        applied_id,
        Some(commit_id),
        SpaceAdmissionBodyV1::Applied(AdmissionAppliedV1::new(receipt.clone())),
    )
    .unwrap();
    let applied_exchange = PendingAdmissionExchange::new(
        SpaceAdmissionRoute::from_bytes(vec![0xe9; 32]).unwrap(),
        applied,
        SpaceAdmissionMessageKind::Complete,
        AdmissionRetryState::new(0, 0).unwrap(),
    )
    .unwrap();
    let transition = joiner.apply_commit(applied_exchange).unwrap();
    let loaded = PendingAdmissionRecoveryStatePort::commit(&fixture.store, token, transition)
        .await
        .unwrap();

    let (joiner, token) = loaded.into_parts();
    let complete = SpaceAdmissionEnvelopeV1::new(
        admission_id,
        AdmissionRole::Sponsor,
        2,
        uc_core::membership::AdmissionMessageId::from_bytes([0xea; 32]).unwrap(),
        Some(applied_id),
        SpaceAdmissionBodyV1::Complete(AdmissionCompleteV1::new(AdmissionCompletionV1::new(
            *admission_id.as_bytes(),
            receipt.event_id,
            [0xeb; 32],
            receipt.installed_security_commitment_id,
            MemberInstanceId::from_bytes([0xec; 32]),
            MembershipCredential::new(1, vec![0xed; 32]).credential_id,
            BaseMembershipHistoryPosition {
                event_id: Some(receipt.event_id),
                depth: 1,
                history_digest: [0xee; 32],
            },
            vec![0xef; 64],
        ))),
    )
    .unwrap();
    let transition = joiner
        .accept_complete(
            complete,
            [0xf0; 32],
            AdmissionSpaceTransition::from_bytes(vec![0xd9; 128]).unwrap(),
        )
        .unwrap();
    PendingAdmissionRecoveryStatePort::commit(&fixture.store, token, transition)
        .await
        .unwrap();
}

async fn load_one_pending(fixture: &Fixture) -> uc_application::deps::LoadedPendingAdmission {
    let mut pending = PendingAdmissionRecoveryStatePort::load(
        &fixture.store,
        AdmissionRecoveryTrigger::StateChanged,
    )
    .await
    .unwrap();
    assert_eq!(pending.len(), 1);
    pending.pop().unwrap()
}

fn activation_mutation(joiner: JoinerAdmission, result_byte: u8) -> JoinerActivationMutation {
    let completion_id = joiner
        .joiner_activation_preparation()
        .unwrap()
        .completion()
        .header()
        .message_id();
    let complete_ack = SpaceAdmissionEnvelopeV1::new(
        joiner.admission_id(),
        AdmissionRole::Joiner,
        3,
        uc_core::membership::AdmissionMessageId::from_bytes([result_byte + 1; 32]).unwrap(),
        Some(completion_id),
        SpaceAdmissionBodyV1::CompleteAck(
            AdmissionCompleteAckV1::new([result_byte + 2; 32]).unwrap(),
        ),
    )
    .unwrap();
    let pending = PendingAdmissionExchange::new(
        SpaceAdmissionRoute::from_bytes(vec![result_byte + 3; 32]).unwrap(),
        complete_ack,
        SpaceAdmissionMessageKind::Settled,
        AdmissionRetryState::new(0, 0).unwrap(),
    )
    .unwrap();
    JoinerActivationMutation::new(
        joiner
            .activate_complete(
                AdmissionSpaceTransitionResult::from_bytes(vec![result_byte; 128]).unwrap(),
                pending,
            )
            .unwrap(),
    )
}

fn candidate_body_fixture() -> AdmissionCandidateV1 {
    let sponsor_credential =
        MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0x91; 32]);
    let joiner_credential =
        MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0x92; 32]);
    let joiner_device = DeviceId::new("activation-joiner");
    let admission = MembershipAdmissionV2 {
        facts: AdmissionChangeFacts {
            member_instance: joiner_credential.member_instance_id(&joiner_device),
            device_id: joiner_device,
            device_name: "activation-joiner".to_owned(),
            identity_fingerprint: IdentityFingerprint::from_display_string("ABCD-EFGH-IJKL-MNOP")
                .unwrap(),
            transport_public_key: vec![0x93; 32],
            transport_address_blob: vec![0x94; 16],
            identity_signature: vec![0x95; 64],
        },
        membership_credential: joiner_credential,
        resume_public_key_digest: [0x96; 32],
        security_commitment_id: [0x97; 32],
    };
    let event = MembershipEventV2::new(
        MEMBERSHIP_EVENT_FORMAT_V2,
        "lineage".to_owned(),
        None,
        0,
        [0x98; 16],
        MemberInstanceId::from_bytes([0x99; 32]),
        sponsor_credential.credential_id,
        ED25519_SIGNATURE_ALGORITHM_V1,
        MembershipOperationV2::AddDevice { admission },
        [0x9a; 32],
        [0x9b; 32],
        vec![0x9c],
        Some([0x9d; 32]),
        vec![0x9e; 64],
    );
    let base = BaseMembershipHistoryPosition {
        event_id: None,
        depth: 0,
        history_digest: [0x9f; 32],
    };
    let commitment = AdmissionSecurityCommitmentV1::new(
        ADMISSION_SECURITY_COMMITMENT_FORMAT_V1,
        "lineage".to_owned(),
        vec![0xa0; 16],
        [0xa1; 32],
        base,
        [0xa2; 32],
        1,
        0,
        1,
        [0xa3; 32],
        [0xa4; 32],
        [0xa5; 32],
        [0xa6; 32],
        [0xa7; 32],
    )
    .unwrap();
    AdmissionCandidateV1::new(
        AdmissionSignedMembershipHistory::from_bytes(vec![0xa8; 64]).unwrap(),
        event,
        commitment,
        AdmissionMlsCommit::from_bytes(vec![0xa9; 64]).unwrap(),
        AdmissionMlsWelcome::from_bytes(vec![0xaa; 64]).unwrap(),
        AdmissionContinuationRoute::from_bytes(vec![0xab; 32]).unwrap(),
    )
    .unwrap()
}
