use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tracing::Instrument;
use uc_application::deps::{
    AuthenticatedSpaceAdmissionMessage, LoadMembershipLedgerPort, LoadedMembershipLedger,
    MembershipLedgerError, SponsorAdmissionMutation, SponsorAdmissionState,
    SponsorAdmissionStateError, SponsorAdmissionStatePort,
};
use uc_observability_contract::diagnostics::{
    operation_span, DiagnosticDomain, DiagnosticOperation, DiagnosticRole, DiagnosticSpanKind,
    OperationContext,
};
use uc_observability_runtime::{
    DeploymentEnvironment, LocalLogConfig, ObservabilityConfig, ObservabilityResource,
    OperatingSystem, ProcessObservabilityRuntime, SignalResult,
};

use super::*;

#[derive(Clone)]
struct FixedMembershipLedger {
    loaded: LoadedMembershipLedger,
}

#[tokio::test]
async fn sponsor_state_load_is_correlated_in_standard_log_file() {
    let logs = tempfile::tempdir().expect("logs");
    let _runtime = ProcessObservabilityRuntime::install(
        ObservabilityConfig::new(
            ObservabilityResource::new(
                "1.1.0",
                DeploymentEnvironment::Test,
                OperatingSystem::Macos,
                "test",
            )
            .expect("resource"),
        )
        .with_local_logs(LocalLogConfig::new(logs.path())),
    )
    .expect("runtime");
    let fixture = Fixture::new();
    let store = sponsor_store(&fixture);
    let message = authenticated_join_request(0xe1, 0xe2);
    let span = operation_span(OperationContext {
        domain: DiagnosticDomain::SpaceAdmission,
        operation: DiagnosticOperation::SpaceAdmission,
        role: DiagnosticRole::Sponsor,
        kind: DiagnosticSpanKind::Internal,
    });
    assert!(!span.is_disabled(), "operation span must be enabled");
    span.in_scope(|| uc_observability_contract::diagnostics::ObservationContext::capture())
        .scope(SponsorAdmissionStatePort::load(&store, &message))
        .instrument(span)
        .await
        .expect("fresh state");
    assert_eq!(
        ProcessObservabilityRuntime::flush_local_logs(std::time::Duration::from_secs(5)),
        SignalResult::Completed
    );
    let records: Vec<serde_json::Value> = std::fs::read_dir(logs.path())
        .expect("files")
        .flat_map(|entry| {
            std::fs::read_to_string(entry.expect("entry").path())
                .expect("file")
                .lines()
                .map(|line| serde_json::from_str(line).expect("record"))
                .collect::<Vec<_>>()
        })
        .collect();
    let steps: Vec<_> = records
        .iter()
        .filter(|record| {
            record["fields"]["step"] == "sponsor_state_load" && record["trace_id"].is_string()
        })
        .collect();
    assert_eq!(steps.len(), 2, "{records:?}");
    assert_eq!(steps[0]["fields"]["event.name"], "runtime.work.started");
    assert_eq!(steps[1]["fields"]["event.name"], "runtime.work.finished");
    assert_eq!(steps[1]["fields"]["uc.outcome"], "ok");
    assert_eq!(steps[0]["trace_id"], steps[1]["trace_id"]);
    assert_eq!(steps[0]["span_id"], steps[1]["span_id"]);
    assert_eq!(steps[1]["capture_mode"], "standard");
    assert!(steps[1]["fields"]["duration_ms"].is_u64());
}

#[async_trait]
impl LoadMembershipLedgerPort for FixedMembershipLedger {
    async fn load(&self) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        Ok(self.loaded.clone())
    }
}

#[tokio::test]
async fn fresh_join_request_returns_claim_snapshot_and_commit_token() {
    let fixture = Fixture::new();
    let store = sponsor_store(&fixture);
    let message = authenticated_join_request(0xe1, 0xe2);

    let loaded = SponsorAdmissionStatePort::load(&store, &message)
        .await
        .unwrap();
    let (state, token) = loaded.into_parts();

    let SponsorAdmissionState::Fresh {
        invitation_claim,
        base_snapshot,
    } = state
    else {
        panic!("new admission must load Fresh sponsor state");
    };
    assert!(!invitation_claim.as_bytes().is_empty());
    assert!(!base_snapshot.as_bytes().is_empty());
    assert_ne!(token.as_bytes(), &[0; 32]);
}

#[tokio::test]
async fn accepted_sponsor_record_survives_restart_and_loads_existing() {
    let fixture = Fixture::new();
    let store = sponsor_store(&fixture);
    let message = authenticated_join_request(0xe3, 0xe4);
    let loaded = SponsorAdmissionStatePort::load(&store, &message)
        .await
        .unwrap();
    let (state, token) = loaded.into_parts();
    let SponsorAdmissionState::Fresh {
        invitation_claim,
        base_snapshot,
    } = state
    else {
        panic!("new admission must load Fresh sponsor state");
    };
    let (peer_binding, envelope, digest, continuation, attempt_contract) = message.into_parts();
    let evidence = envelope.evidence(digest).unwrap();
    let transition = SponsorAdmission::accept_join_request_with_contract(
        envelope.header().admission_id(),
        invitation_claim,
        envelope,
        evidence,
        base_snapshot,
        peer_binding,
        continuation.unwrap(),
        attempt_contract.unwrap(),
    )
    .unwrap();

    let committed =
        SponsorAdmissionStatePort::commit(&store, token, SponsorAdmissionMutation::new(transition))
            .await
            .unwrap();
    let (expected, _) = committed.into_parts();
    let expected = expected.encode_persisted().unwrap();

    let reopened = sponsor_store(&fixture);
    let duplicate = authenticated_join_request(0xe3, 0xe4);
    let loaded = SponsorAdmissionStatePort::load(&reopened, &duplicate)
        .await
        .unwrap();
    let (state, _) = loaded.into_parts();
    let SponsorAdmissionState::Existing(existing) = state else {
        panic!("saved sponsor admission must load Existing");
    };
    assert_eq!(existing.encode_persisted().unwrap(), expected);
}

#[tokio::test]
async fn stale_fresh_sponsor_token_cannot_commit_after_first_writer() {
    let fixture = Fixture::new();
    let store = sponsor_store(&fixture);
    let first_message = authenticated_join_request(0xe5, 0xe6);
    let second_message = authenticated_join_request(0xe5, 0xe6);
    let first = SponsorAdmissionStatePort::load(&store, &first_message)
        .await
        .unwrap();
    let second = SponsorAdmissionStatePort::load(&store, &second_message)
        .await
        .unwrap();
    let first_mutation = accepted_mutation(first_message, first);
    let (second_state, second_token) = second.into_parts();
    let second_mutation = accepted_transition(second_message, second_state);

    SponsorAdmissionStatePort::commit(&store, first_mutation.0, first_mutation.1)
        .await
        .unwrap();
    assert!(matches!(
        SponsorAdmissionStatePort::commit(
            &store,
            second_token,
            SponsorAdmissionMutation::new(second_mutation),
        )
        .await,
        Err(SponsorAdmissionStateError::StateChanged { .. })
    ));
}

#[tokio::test]
async fn one_invitation_cannot_start_two_sponsor_admissions() {
    let fixture = Fixture::new();
    let store = sponsor_store(&fixture);
    let first_message = authenticated_join_request(0xe7, 0xe8);
    let first = SponsorAdmissionStatePort::load(&store, &first_message)
        .await
        .unwrap();
    let (token, mutation) = accepted_mutation(first_message, first);
    SponsorAdmissionStatePort::commit(&store, token, mutation)
        .await
        .unwrap();

    let conflicting = authenticated_join_request(0xe9, 0xe8);
    assert!(matches!(
        SponsorAdmissionStatePort::load(&store, &conflicting).await,
        Err(SponsorAdmissionStateError::StateChanged { .. })
    ));
}

#[tokio::test]
async fn sponsor_payload_does_not_expose_invitation_or_membership_history() {
    let fixture = Fixture::new();
    let store = sponsor_store(&fixture);
    let message = authenticated_join_request(0xea, 0xeb);
    let loaded = SponsorAdmissionStatePort::load(&store, &message)
        .await
        .unwrap();
    let (token, mutation) = accepted_mutation(message, loaded);

    SponsorAdmissionStatePort::commit(&store, token, mutation)
        .await
        .unwrap();
    let encrypted = fixture.encrypted_payload();

    assert!(!encrypted.windows(32).any(|window| window == [0xeb; 32]));
    assert!(!encrypted.windows(64).any(|window| window == [0x44; 64]));
}

#[tokio::test]
async fn sponsor_abandonment_cleanup_survives_restart_and_commits_once() {
    let fixture = Fixture::new();
    let store = sponsor_store(&fixture);
    let message = authenticated_join_request(0xec, 0xed);
    let loaded = SponsorAdmissionStatePort::load(&store, &message)
        .await
        .expect("fresh sponsor state");
    let (token, mutation) = accepted_mutation(message, loaded);
    let accepted = SponsorAdmissionStatePort::commit(&store, token, mutation)
        .await
        .expect("accepted state commits");
    let (accepted, token) = accepted.into_parts();
    let join_request = authenticated_join_request(0xec, 0xed);
    let (_, join_request, _, _, contract) = join_request.into_parts();
    let attempt_digest = contract.expect("attempt contract").digest();
    let candidate_reply = SpaceAdmissionEnvelopeV1::reply_to(
        &join_request,
        AdmissionRole::Sponsor,
        0,
        uc_core::membership::AdmissionMessageId::from_bytes([0xee; 32])
            .expect("candidate message id"),
        SpaceAdmissionBodyV1::Candidate(super::activation::candidate_body_fixture()),
    )
    .expect("candidate reply");
    let candidate = accepted
        .fix_candidate(
            candidate_reply,
            uc_core::membership::AdmissionStagedSecurityState::from_bytes(vec![0xef; 64])
                .expect("staged security"),
        )
        .expect("candidate transition");
    let candidate =
        SponsorAdmissionStatePort::commit(&store, token, SponsorAdmissionMutation::new(candidate))
            .await
            .expect("candidate state commits");
    let (candidate, token) = candidate.into_parts();
    let predecessor = candidate
        .current_exact_reply()
        .expect("candidate exact reply");
    let candidate_body = super::activation::candidate_body_fixture();
    let prepared = SpaceAdmissionEnvelopeV1::reply_to(
        predecessor,
        AdmissionRole::Joiner,
        1,
        uc_core::membership::AdmissionMessageId::from_bytes([0xf0; 32])
            .expect("prepared message id"),
        SpaceAdmissionBodyV1::Prepared(uc_core::membership::AdmissionPreparedV1::new(
            uc_core::membership::PreparedAdmissionProofV1::new(
                *predecessor.header().admission_id().as_bytes(),
                candidate_body.candidate_event().lineage_id.clone(),
                uc_core::membership::BaseMembershipHistoryPosition {
                    event_id: None,
                    depth: 0,
                    history_digest: [0xf1; 32],
                },
                candidate_body.candidate_event().event_id(),
                candidate_body.candidate_event().resulting_members_digest,
                candidate_body.security_commitment().security_commitment_id,
                uc_core::membership::MemberInstanceId::from_bytes([0xf2; 32]),
                uc_core::membership::MembershipCredential::new(1, vec![0xf3; 32]).credential_id,
                vec![0xf4; 64],
            ),
        )),
    )
    .expect("prepared request");
    let committed_history =
        uc_core::membership::AdmissionSignedMembershipHistory::from_bytes(vec![0xf5; 128])
            .expect("committed history");
    let commit_reply = SpaceAdmissionEnvelopeV1::reply_to(
        &prepared,
        AdmissionRole::Sponsor,
        1,
        uc_core::membership::AdmissionMessageId::from_bytes([0xf6; 32]).expect("commit message id"),
        SpaceAdmissionBodyV1::Commit(uc_core::membership::AdmissionCommitV1::new(
            super::activation::candidate_body_fixture(),
            uc_core::membership::AdmissionSignedMembershipHistory::from_bytes(vec![0xf5; 128])
                .expect("commit target history"),
            uc_core::membership::AdmissionSealedRecoveryMaterial::from_bytes(vec![0xf7; 128])
                .expect("sealed recovery material"),
        )),
    )
    .expect("commit reply");
    let committed = candidate
        .commit_prepared(
            prepared,
            [0xf8; 32],
            committed_history,
            uc_core::membership::AdmissionSealedSecurityState::from_bytes(vec![0xf9; 128])
                .expect("sealed security"),
            commit_reply,
        )
        .expect("commit transition");
    let committed =
        SponsorAdmissionStatePort::commit(&store, token, SponsorAdmissionMutation::new(committed))
            .await
            .expect("committed state saves");
    let (committed, token) = committed.into_parts();
    let predecessor = committed.current_exact_reply().expect("commit exact reply");
    let abandonment = SpaceAdmissionEnvelopeV1::reply_to(
        predecessor,
        AdmissionRole::Joiner,
        4,
        uc_core::membership::AdmissionMessageId::from_bytes([0xfa; 32])
            .expect("abandonment message id"),
        SpaceAdmissionBodyV1::Abandonment(
            uc_core::membership::AdmissionAbandonmentV2::new(
                attempt_digest,
                None,
                uc_core::membership::AdmissionAbandonmentReasonV2::Cancelled,
            )
            .expect("abandonment body"),
        ),
    )
    .expect("abandonment request");
    let abandonment_digest: [u8; 32] = Sha256::digest(
        abandonment
            .encode_canonical_v1()
            .expect("canonical abandonment request"),
    )
    .into();
    let abandoned_reply = SpaceAdmissionEnvelopeV1::reply_to(
        &abandonment,
        AdmissionRole::Sponsor,
        4,
        uc_core::membership::AdmissionMessageId::from_bytes([0xfb; 32])
            .expect("abandoned message id"),
        SpaceAdmissionBodyV1::Abandoned(
            uc_core::membership::AdmissionAbandonedV2::new(abandonment_digest)
                .expect("abandonment digest"),
        ),
    )
    .expect("abandoned reply");
    let abandoned = committed
        .accept_abandonment(abandonment, abandonment_digest, abandoned_reply)
        .expect("abandonment transition");
    SponsorAdmissionStatePort::commit(&store, token, SponsorAdmissionMutation::new(abandoned))
        .await
        .expect("abandonment state commits");

    let reopened = sponsor_store(&fixture);
    let recovery =
        PendingAdmissionRecoveryStatePort::load(&reopened, AdmissionRecoveryTrigger::Startup, 0)
            .await
            .expect("pending abandonment loads after restart");
    let (_, _, mut pending, _, _) = recovery.into_parts();
    assert_eq!(pending.len(), 1);
    let (abandoned, recovery_token) = pending.pop().expect("one pending abandonment").into_parts();
    let completed = abandoned
        .complete_abandonment_cleanup()
        .expect("cleanup completion transition");
    PendingAdmissionRecoveryStatePort::commit_sponsor_abandonment(
        &reopened,
        recovery_token,
        completed,
    )
    .await
    .expect("cleanup completion commits");

    assert!(PendingAdmissionRecoveryStatePort::load(
        &reopened,
        AdmissionRecoveryTrigger::Startup,
        0,
    )
    .await
    .expect("completed abandonment reloads")
    .is_empty());
}

fn sponsor_store(fixture: &Fixture) -> SqliteSpaceAdmissionState<Arc<DieselSqliteExecutor>> {
    let executor = Arc::new(DieselSqliteExecutor::new(
        init_db_pool(fixture.db_path.to_str().unwrap()).unwrap(),
    ));
    let keys = Arc::new(AdmissionKeyManager::new(
        fixture.secure_storage.clone(),
        [0x31; 16],
    ));
    let manifests = Arc::new(ActiveSpaceGenerationManifestStore::new(
        fixture._temp.path().join("vault"),
        Arc::clone(&keys),
    ));
    SqliteSpaceAdmissionState::new(
        executor,
        keys,
        manifests,
        Arc::new(FixedMembershipLedger {
            loaded: membership_ledger(),
        }),
    )
}

fn membership_ledger() -> LoadedMembershipLedger {
    let mut loaded = LoadedMembershipLedger::no_current_space();
    loaded.revision = 7;
    loaded.lineage_id = Some("space-a".to_owned());
    loaded.membership_history = Some(vec![0x44; 128]);
    loaded
}

fn authenticated_join_request(
    admission_byte: u8,
    invitation_byte: u8,
) -> AuthenticatedSpaceAdmissionMessage {
    let admission_id = SpaceAdmissionId::from_bytes([admission_byte; 32]).unwrap();
    let device_id = DeviceId::new("joining-device");
    let credential = MembershipCredential::new(1, vec![admission_byte + 2; 32]);
    let signature = vec![admission_byte + 5; 64];
    let envelope = SpaceAdmissionEnvelopeV1::new_with_version(
        SpaceAdmissionProtocolVersion::V2,
        admission_id,
        AdmissionRole::Joiner,
        0,
        uc_core::membership::AdmissionMessageId::from_bytes([admission_byte + 1; 32]).unwrap(),
        None,
        SpaceAdmissionBodyV1::JoinRequest(
            AdmissionJoinRequestV1::new(
                InvitationId::from_bytes([invitation_byte; 32]).unwrap(),
                device_id.clone(),
                join_request_identity_facts(device_id, &credential, signature.clone()),
                credential,
                AdmissionKeyPackage::from_bytes(vec![admission_byte + 3; 48]).unwrap(),
                AdmissionRecoveryPublicKey::from_bytes([admission_byte + 4; 32]).unwrap(),
                AdmissionIdentitySignature::from_bytes(signature).unwrap(),
                UnreadableHistoryPolicy::Discard,
            )
            .unwrap(),
        ),
    )
    .unwrap();
    let binding = peer_binding();
    AuthenticatedSpaceAdmissionMessage::new(
        binding,
        envelope,
        [admission_byte + 6; 32],
        Some(continuation()),
        Some(
            AdmissionAttemptContractV2::start(
                admission_id,
                InvitationId::from_bytes([invitation_byte; 32]).unwrap(),
                binding.remote_peer_id(),
                binding.local_peer_id(),
                1_000,
            )
            .expect("valid attempt contract"),
        ),
    )
    .unwrap()
}

fn accepted_mutation(
    message: AuthenticatedSpaceAdmissionMessage,
    loaded: uc_application::deps::LoadedSponsorAdmission,
) -> (
    uc_application::deps::SponsorAdmissionCommitToken,
    SponsorAdmissionMutation,
) {
    let (state, token) = loaded.into_parts();
    (
        token,
        SponsorAdmissionMutation::new(accepted_transition(message, state)),
    )
}

fn accepted_transition(
    message: AuthenticatedSpaceAdmissionMessage,
    state: SponsorAdmissionState,
) -> SponsorAdmissionTransition {
    let SponsorAdmissionState::Fresh {
        invitation_claim,
        base_snapshot,
    } = state
    else {
        panic!("fixture must be Fresh sponsor state");
    };
    let (peer_binding, envelope, digest, continuation, attempt_contract) = message.into_parts();
    let evidence = envelope.evidence(digest).unwrap();
    SponsorAdmission::accept_join_request_with_contract(
        envelope.header().admission_id(),
        invitation_claim,
        envelope,
        evidence,
        base_snapshot,
        peer_binding,
        continuation.unwrap(),
        attempt_contract.unwrap(),
    )
    .unwrap()
}
