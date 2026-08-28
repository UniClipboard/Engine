use async_trait::async_trait;
use uc_application::deps::{
    AuthenticatedSpaceAdmissionMessage, LoadMembershipLedgerPort, LoadedMembershipLedger,
    MembershipLedgerError, SponsorAdmissionMutation, SponsorJoinRequestState,
    SponsorJoinRequestStateError, SponsorJoinRequestStatePort,
};

use super::*;

#[derive(Clone)]
struct FixedMembershipLedger {
    loaded: LoadedMembershipLedger,
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

    let loaded = SponsorJoinRequestStatePort::load(&store, &message)
        .await
        .unwrap();
    let (state, token) = loaded.into_parts();

    let SponsorJoinRequestState::Fresh {
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
    let loaded = SponsorJoinRequestStatePort::load(&store, &message)
        .await
        .unwrap();
    let (state, token) = loaded.into_parts();
    let SponsorJoinRequestState::Fresh {
        invitation_claim,
        base_snapshot,
    } = state
    else {
        panic!("new admission must load Fresh sponsor state");
    };
    let (peer_binding, envelope, digest, continuation) = message.into_parts();
    let evidence = envelope.evidence(digest).unwrap();
    let transition = SpaceAdmissionAggregate::accept_join_request(
        envelope.header().admission_id(),
        invitation_claim,
        envelope,
        evidence,
        base_snapshot,
        peer_binding,
        continuation.unwrap(),
    )
    .unwrap();

    let committed = SponsorJoinRequestStatePort::commit(
        &store,
        token,
        SponsorAdmissionMutation::new(transition),
    )
    .await
    .unwrap();
    let (expected, _) = committed.into_parts();
    let expected = expected.encode_persisted().unwrap();

    let reopened = sponsor_store(&fixture);
    let duplicate = authenticated_join_request(0xe3, 0xe4);
    let loaded = SponsorJoinRequestStatePort::load(&reopened, &duplicate)
        .await
        .unwrap();
    let (state, _) = loaded.into_parts();
    let SponsorJoinRequestState::Existing(existing) = state else {
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
    let first = SponsorJoinRequestStatePort::load(&store, &first_message)
        .await
        .unwrap();
    let second = SponsorJoinRequestStatePort::load(&store, &second_message)
        .await
        .unwrap();
    let first_mutation = accepted_mutation(first_message, first);
    let (second_state, second_token) = second.into_parts();
    let second_mutation = accepted_transition(second_message, second_state);

    SponsorJoinRequestStatePort::commit(&store, first_mutation.0, first_mutation.1)
        .await
        .unwrap();
    assert!(matches!(
        SponsorJoinRequestStatePort::commit(
            &store,
            second_token,
            SponsorAdmissionMutation::new(second_mutation),
        )
        .await,
        Err(SponsorJoinRequestStateError::StateChanged)
    ));
}

#[tokio::test]
async fn one_invitation_cannot_start_two_sponsor_admissions() {
    let fixture = Fixture::new();
    let store = sponsor_store(&fixture);
    let first_message = authenticated_join_request(0xe7, 0xe8);
    let first = SponsorJoinRequestStatePort::load(&store, &first_message)
        .await
        .unwrap();
    let (token, mutation) = accepted_mutation(first_message, first);
    SponsorJoinRequestStatePort::commit(&store, token, mutation)
        .await
        .unwrap();

    let conflicting = authenticated_join_request(0xe9, 0xe8);
    assert!(matches!(
        SponsorJoinRequestStatePort::load(&store, &conflicting).await,
        Err(SponsorJoinRequestStateError::StateChanged)
    ));
}

#[tokio::test]
async fn sponsor_payload_does_not_expose_invitation_or_membership_history() {
    let fixture = Fixture::new();
    let store = sponsor_store(&fixture);
    let message = authenticated_join_request(0xea, 0xeb);
    let loaded = SponsorJoinRequestStatePort::load(&store, &message)
        .await
        .unwrap();
    let (token, mutation) = accepted_mutation(message, loaded);

    SponsorJoinRequestStatePort::commit(&store, token, mutation)
        .await
        .unwrap();
    let encrypted = fixture.encrypted_payload();

    assert!(!encrypted.windows(32).any(|window| window == [0xeb; 32]));
    assert!(!encrypted.windows(64).any(|window| window == [0x44; 64]));
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
    let envelope = SpaceAdmissionEnvelopeV1::new(
        admission_id,
        AdmissionRole::Joiner,
        0,
        uc_core::membership::AdmissionMessageId::from_bytes([admission_byte + 1; 32]).unwrap(),
        None,
        SpaceAdmissionBodyV1::JoinRequest(
            AdmissionJoinRequestV1::new(
                InvitationId::from_bytes([invitation_byte; 32]).unwrap(),
                DeviceId::new("joining-device"),
                MembershipCredential::new(1, vec![admission_byte + 2; 32]),
                AdmissionKeyPackage::from_bytes(vec![admission_byte + 3; 48]).unwrap(),
                AdmissionRecoveryPublicKey::from_bytes([admission_byte + 4; 32]).unwrap(),
                AdmissionIdentitySignature::from_bytes(vec![admission_byte + 5; 64]).unwrap(),
                UnreadableHistoryPolicy::Discard,
            )
            .unwrap(),
        ),
    )
    .unwrap();
    AuthenticatedSpaceAdmissionMessage::new(
        peer_binding(),
        envelope,
        [admission_byte + 6; 32],
        Some(continuation()),
    )
    .unwrap()
}

fn accepted_mutation(
    message: AuthenticatedSpaceAdmissionMessage,
    loaded: uc_application::deps::LoadedSponsorJoinRequest,
) -> (
    uc_application::deps::SponsorJoinRequestCommitToken,
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
    state: SponsorJoinRequestState,
) -> uc_core::membership::AdmissionTransition {
    let SponsorJoinRequestState::Fresh {
        invitation_claim,
        base_snapshot,
    } = state
    else {
        panic!("fixture must be Fresh sponsor state");
    };
    let (peer_binding, envelope, digest, continuation) = message.into_parts();
    let evidence = envelope.evidence(digest).unwrap();
    SpaceAdmissionAggregate::accept_join_request(
        envelope.header().admission_id(),
        invitation_claim,
        envelope,
        evidence,
        base_snapshot,
        peer_binding,
        continuation.unwrap(),
    )
    .unwrap()
}
