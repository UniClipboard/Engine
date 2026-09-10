use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use uc_observability_contract::diagnostics::connectivity::complete_admission_authentication_failure;

use async_trait::async_trait;
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh::Endpoint;
use uc_application::deps::{
    AuthenticatedSpaceAdmissionMessage, HandleAuthenticatedSpaceAdmissionMessagePort,
    SpaceAdmissionTransportError, SpaceAdmissionTransportPort,
};
use uc_core::membership::{
    AdmissionChannelPeerId, AdmissionContinuationCredential, AdmissionEncryptedPasswordEquivalent,
    InvitationId, SpaceAdmissionEnvelopeV1, SpaceAdmissionId, SpaceAdmissionProtocolVersion,
    SpaceAdmissionRoute,
};
use uc_observability_contract::diagnostics::{
    operation_span, DiagnosticDomain, DiagnosticOperation, DiagnosticRole, DiagnosticSpanKind,
    OperationContext,
};

use crate::security::{
    SpaceAdmissionAuth, SpaceAdmissionAuthContext, SpaceAdmissionKe1, SpaceAdmissionKe3,
};

use super::super::space_admission_wire::LARGE_MESSAGE_LIMIT;
use super::super::space_admission_wire::{
    read_raw_with_limit, read_typed, write_typed, ContinuationHelloV1, FrameKind, InitialHelloV1,
    OpaqueFinishV1, OpaqueResponseV1, WireError, AUTH_FRAME_LIMIT, IO_DEADLINE,
};
use super::super::trace_context::set_remote_parent;
use super::super::trace_context::{inject_current, WireTraceContext};
use uc_observability_contract::diagnostics::connectivity::{AuthenticationFailure, ProofFailure};
use uc_observability_contract::diagnostics::DiagnosticErrorType;

use super::crypto::{calculate_mac, peer_id, verify_mac};
use super::diagnostics::{record_client_completion, server_error_type};
use super::errors::{
    map_application_close_code, map_reply_wire_error, map_request_wire_error,
    map_server_wire_error, HandlerError, CLOSE_AUTHENTICATION, CLOSE_BUSY,
    CLOSE_PEER_UPGRADE_REQUIRED, CLOSE_PROTOCOL, LEGACY_CLOSE_PROTOCOL,
};
use super::server::read_peer_acknowledgement;

use iroh::endpoint::presets;
use iroh::protocol::Router;
use iroh::RelayMode;
use opentelemetry::logs::AnyValue;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_sdk::logs::{InMemoryLogExporter, SdkLoggerProvider};
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
use tokio::sync::{Mutex, Notify};
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Layer;
use uc_core::ids::DeviceId;
use uc_core::membership::{
    AdmissionBaseSnapshot, AdmissionCandidateV1, AdmissionChangeFacts, AdmissionCommitV1,
    AdmissionContinuationRoute, AdmissionIdentitySignature, AdmissionInvitationClaim,
    AdmissionJoinRequestV1, AdmissionKeyPackage, AdmissionMessageId, AdmissionMlsCommit,
    AdmissionMlsWelcome, AdmissionPreparedV1, AdmissionRecoveryPublicKey,
    AdmissionSealedRecoveryMaterial, AdmissionSealedSecurityState, AdmissionSecurityCommitmentV1,
    AdmissionSignedMembershipHistory, AdmissionStagedSecurityState, BaseMembershipHistoryPosition,
    MemberInstanceId, MembershipAdmissionV2, MembershipCredential, MembershipEventV2,
    MembershipOperationV2, PreparedAdmissionProofV1, SponsorAdmission, UnreadableHistoryPolicy,
    ADMISSION_SECURITY_COMMITMENT_FORMAT_V1, ED25519_SIGNATURE_ALGORITHM_V1,
    MEMBERSHIP_EVENT_FORMAT_V2,
};
use uc_core::membership::{AdmissionRecordPersistence, AdmissionRole, SpaceAdmissionBodyV1};
use uc_core::security::IdentityFingerprint;

use super::*;

fn credential() -> AdmissionContinuationCredential {
    AdmissionContinuationCredential::from_bytes(vec![0x31; 64])
        .expect("bounded continuation credential")
}

fn admission_id() -> SpaceAdmissionId {
    SpaceAdmissionId::from_bytes([0x32; 32]).expect("non-zero admission id")
}

fn peer(byte: u8) -> AdmissionChannelPeerId {
    AdmissionChannelPeerId::from_bytes([byte; 32]).expect("non-zero peer id")
}

struct LoopbackCredentials {
    initial: Mutex<Option<SponsorOpaqueMaterial>>,
    continuation: Mutex<Option<Vec<u8>>>,
}

#[async_trait]
impl SpaceAdmissionChannelCredentialPort for LoopbackCredentials {
    async fn resolve_initial(
        &self,
        _invitation_id: InvitationId,
        _admission_id: SpaceAdmissionId,
    ) -> Result<SponsorOpaqueMaterial, SpaceAdmissionChannelCredentialError> {
        self.initial.lock().await.take().ok_or_else(|| {
            SpaceAdmissionChannelCredentialError::Rejected {
                source: anyhow::anyhow!("initial credential already consumed"),
            }
        })
    }

    async fn load_continuation(
        &self,
        _admission_id: SpaceAdmissionId,
    ) -> Result<AdmissionContinuationCredential, SpaceAdmissionChannelCredentialError> {
        let bytes = self.continuation.lock().await.clone().ok_or_else(|| {
            SpaceAdmissionChannelCredentialError::Unavailable {
                source: anyhow::anyhow!("continuation is not committed"),
            }
        })?;
        AdmissionContinuationCredential::from_bytes(bytes).map_err(|source| {
            SpaceAdmissionChannelCredentialError::Rejected {
                source: anyhow::Error::new(source),
            }
        })
    }
}

struct PersistingLoopbackEndpoint {
    credentials: Arc<LoopbackCredentials>,
    candidate_state: Mutex<Option<Vec<u8>>>,
    calls: AtomicUsize,
    completed: AtomicUsize,
    continuation_route: Vec<u8>,
}

struct HangingLoopbackEndpoint {
    calls: AtomicUsize,
    entered: Notify,
}

#[async_trait]
impl HandleAuthenticatedSpaceAdmissionMessagePort for HangingLoopbackEndpoint {
    async fn handle(
        &self,
        _message: AuthenticatedSpaceAdmissionMessage,
    ) -> Result<
        uc_application::deps::SpaceAdmissionMessageReply,
        uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError,
    > {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        std::future::pending().await
    }
}

struct LegacyLayoutHandler {
    local_peer_id: AdmissionChannelPeerId,
    credentials: Arc<LoopbackCredentials>,
}

impl std::fmt::Debug for LegacyLayoutHandler {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("LegacyLayoutHandler(REDACTED)")
    }
}

impl ProtocolHandler for LegacyLayoutHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let remote_peer_id =
            peer_id(connection.remote_id().as_bytes()).expect("legacy remote peer");
        let (mut send, mut receive) = connection.accept_bi().await.expect("legacy stream");
        let (kind, payload) = read_raw_with_limit(&mut receive, AUTH_FRAME_LIMIT)
            .await
            .expect("legacy initial hello");
        assert_eq!(kind, FrameKind::InitialHello);
        let hello: InitialHelloV1 = postcard::from_bytes(&payload).expect("legacy hello layout");
        let admission_id =
            SpaceAdmissionId::from_bytes(hello.admission_id).expect("legacy admission id");
        let invitation_id =
            InvitationId::from_bytes(hello.invitation_id).expect("legacy invitation id");
        assert_eq!(
            hello.protocol_version,
            SpaceAdmissionProtocolVersion::V1.as_u16()
        );
        assert_eq!(hello.joiner_peer_id, *remote_peer_id.as_bytes());
        let material = self
            .credentials
            .resolve_initial(invitation_id, admission_id)
            .await
            .expect("legacy credential");
        let context = SpaceAdmissionAuthContext::new(
            SpaceAdmissionProtocolVersion::V1,
            admission_id,
            invitation_id,
            remote_peer_id,
            self.local_peer_id,
        );
        let ke1 = SpaceAdmissionKe1::decode_from_transport(&hello.ke1)
            .expect("legacy client authentication start");
        let (server, ke2) = SpaceAdmissionAuth::start_server(
            &material.server_setup,
            &material.registration,
            &context,
            ke1,
        )
        .expect("legacy server authentication start");
        write_typed(
            &mut send,
            FrameKind::OpaqueResponse,
            &OpaqueResponseV1 {
                sponsor_peer_id: *self.local_peer_id.as_bytes(),
                ke2: ke2.encode_for_transport(),
            },
            AUTH_FRAME_LIMIT,
        )
        .await
        .expect("legacy authentication response");
        let finish: OpaqueFinishV1 =
            read_typed(&mut receive, FrameKind::OpaqueFinish, AUTH_FRAME_LIMIT)
                .await
                .expect("legacy authentication finish");
        let ke3 = SpaceAdmissionKe3::decode_from_transport(&finish.ke3)
            .expect("legacy authentication finish payload");
        let _credential = server
            .finish(&context, ke3)
            .expect("legacy authenticated peer");
        let (kind, payload) = read_raw_with_limit(&mut receive, LARGE_MESSAGE_LIMIT)
            .await
            .expect("legacy authenticated request");
        assert_eq!(kind, FrameKind::Request);
        assert!(!payload.is_empty());
        connection.close(LEGACY_CLOSE_PROTOCOL.into(), b"protocol_rejected");
        Ok(())
    }
}

#[async_trait]
impl HandleAuthenticatedSpaceAdmissionMessagePort for PersistingLoopbackEndpoint {
    async fn handle(
        &self,
        message: AuthenticatedSpaceAdmissionMessage,
    ) -> Result<
        uc_application::deps::SpaceAdmissionMessageReply,
        uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError,
    > {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let (binding, envelope, digest, continuation) = message.into_parts();
        match envelope.body() {
            SpaceAdmissionBodyV1::JoinRequest(_) => {
                let continuation = continuation.ok_or_else(|| {
                    uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::invalid(
                        anyhow::anyhow!("fresh request missing continuation"),
                    )
                })?;
                *self.credentials.continuation.lock().await =
                    Some(continuation.as_bytes().to_vec());
                let admission_id = envelope.header().admission_id();
                let predecessor = envelope.header().message_id();
                let accepted = SponsorAdmission::accept_join_request(
                    admission_id,
                    AdmissionInvitationClaim::from_bytes(vec![0x41; 32]).map_err(
                        uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::invalid,
                    )?,
                    envelope,
                    uc_core::membership::AdmissionMessageEvidence::new(
                        AdmissionRole::Joiner,
                        0,
                        predecessor,
                        None,
                        digest,
                    )
                    .ok_or_else(|| uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::invalid(anyhow::anyhow!("invalid evidence")))?,
                    AdmissionBaseSnapshot::from_bytes(vec![0x42; 64]).map_err(
                        uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::invalid,
                    )?,
                    binding,
                    continuation,
                )
                .map_err(uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::invalid)?
                .into_replacement();
                let candidate = SpaceAdmissionEnvelopeV1::new(
                    admission_id,
                    AdmissionRole::Sponsor,
                    0,
                    message_id(0x43),
                    Some(predecessor),
                    SpaceAdmissionBodyV1::Candidate(candidate_body(
                        admission_id,
                        self.continuation_route.clone(),
                    )),
                )
                .map_err(
                    uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::invalid,
                )?;
                let candidate = accepted
                    .fix_candidate(
                        candidate,
                        AdmissionStagedSecurityState::from_bytes(vec![0x44; 64]).map_err(
                            uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::invalid,
                        )?,
                    )
                    .map_err(uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::invalid)?
                    .into_replacement();
                let encoded = candidate.encode_persisted().map_err(
                    uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::invalid,
                )?;
                *self.candidate_state.lock().await = Some(encoded.clone());
                let reply = SponsorAdmission::decode_persisted(&encoded).map_err(
                    uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::invalid,
                )?;
                let reply = uc_application::deps::SpaceAdmissionMessageReply::new(reply).ok_or_else(|| {
                    uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::invalid(
                        anyhow::anyhow!("candidate reply was not saved"),
                    )
                })?;
                self.completed.store(1, Ordering::SeqCst);
                Ok(reply)
            }
            SpaceAdmissionBodyV1::Prepared(_) => {
                if continuation.is_some() {
                    return Err(uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::invalid(anyhow::anyhow!("resume created a new continuation")));
                }
                let encoded = self.candidate_state.lock().await.clone().ok_or_else(|| {
                    uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::recovery_required(anyhow::anyhow!("candidate state missing"))
                })?;
                let candidate = SponsorAdmission::decode_persisted(&encoded).map_err(
                    uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::invalid,
                )?;
                let fixed_bytes = candidate
                    .sponsor_commit_preparation()
                    .ok_or_else(|| uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::recovery_required(anyhow::anyhow!("fixed candidate missing")))?
                    .candidate_reply()
                    .encode_canonical_v1()
                    .map_err(uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::invalid)?;
                let fixed = match SpaceAdmissionEnvelopeV1::decode_canonical_v1(&fixed_bytes)
                    .map_err(uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::invalid)?
                    .into_body()
                {
                    SpaceAdmissionBodyV1::Candidate(body) => body,
                    _ => return Err(uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::recovery_required(anyhow::anyhow!("fixed candidate body missing"))),
                };
                let history = AdmissionSignedMembershipHistory::from_bytes(vec![0x45; 64])
                    .map_err(
                    uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::invalid,
                )?;
                let commit = SpaceAdmissionEnvelopeV1::new(
                    envelope.header().admission_id(),
                    AdmissionRole::Sponsor,
                    1,
                    message_id(0x46),
                    Some(envelope.header().message_id()),
                    SpaceAdmissionBodyV1::Commit(AdmissionCommitV1::new(
                        fixed,
                        AdmissionSignedMembershipHistory::from_bytes(history.as_bytes().to_vec()).map_err(uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::invalid)?,
                        AdmissionSealedRecoveryMaterial::from_bytes(vec![0x47; 64]).map_err(uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::invalid)?,
                    )),
                ).map_err(uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::invalid)?;
                let committed = candidate.commit_prepared(
                    envelope,
                    digest,
                    history,
                    AdmissionSealedSecurityState::from_bytes(vec![0x48; 64]).map_err(uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::invalid)?,
                    commit,
                ).map_err(uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::invalid)?.into_replacement();
                uc_application::deps::SpaceAdmissionMessageReply::new(committed).ok_or_else(|| {
                    uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::invalid(
                        anyhow::anyhow!("commit reply was not saved"),
                    )
                })
            }
            _ => Err(
                uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::out_of_order(
                    anyhow::anyhow!("unexpected loopback message"),
                ),
            ),
        }
    }
}

async fn bound_endpoint() -> Arc<Endpoint> {
    Arc::new(
        Endpoint::builder(presets::N0)
            .alpns(vec![SPACE_ADMISSION_ALPN.to_vec()])
            .relay_mode(RelayMode::Disabled)
            .clear_address_lookup()
            .bind()
            .await
            .expect("bind loopback endpoint"),
    )
}

async fn wait_for_direct_addrs(endpoint: &Endpoint) {
    for _ in 0..100 {
        if !endpoint.addr().addrs.is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("endpoint did not publish a direct address");
}

fn message_id(byte: u8) -> AdmissionMessageId {
    AdmissionMessageId::from_bytes([byte; 32]).expect("non-zero message id")
}

fn join_request(
    admission_id: SpaceAdmissionId,
    invitation_id: InvitationId,
) -> SpaceAdmissionEnvelopeV1 {
    let device = DeviceId::new("loopback-joiner");
    let credential = MembershipCredential::new(1, vec![0x61; 32]);
    let signature = vec![0x62; 64];
    let facts = AdmissionChangeFacts {
        member_instance: credential.member_instance_id(&device),
        device_id: device.clone(),
        device_name: "Loopback joiner".to_owned(),
        identity_fingerprint: IdentityFingerprint::from_display_string("ABCD-EFGH-IJKL-MNOP")
            .expect("fingerprint"),
        transport_public_key: vec![0x63; 32],
        transport_address_blob: vec![0x64; 32],
        identity_signature: signature.clone(),
    };
    let body = AdmissionJoinRequestV1::new(
        invitation_id,
        device,
        facts,
        credential,
        AdmissionKeyPackage::from_bytes(vec![0x65; 48]).expect("key package"),
        AdmissionRecoveryPublicKey::from_bytes([0x66; 32]).expect("recovery key"),
        AdmissionIdentitySignature::from_bytes(signature).expect("identity signature"),
        UnreadableHistoryPolicy::Discard,
    )
    .expect("JoinRequest");
    SpaceAdmissionEnvelopeV1::new(
        admission_id,
        AdmissionRole::Joiner,
        0,
        message_id(0x67),
        None,
        SpaceAdmissionBodyV1::JoinRequest(body),
    )
    .expect("JoinRequest envelope")
}

fn candidate_body(
    admission_id: SpaceAdmissionId,
    continuation_route: Vec<u8>,
) -> AdmissionCandidateV1 {
    let sponsor_credential =
        MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0x71; 32]);
    let joiner_credential =
        MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0x72; 32]);
    let joiner_device = DeviceId::new("candidate-joiner");
    let admission = MembershipAdmissionV2 {
        facts: AdmissionChangeFacts {
            member_instance: joiner_credential.member_instance_id(&joiner_device),
            device_id: joiner_device,
            device_name: "candidate-joiner".to_owned(),
            identity_fingerprint: IdentityFingerprint::from_display_string("ABCD-EFGH-IJKL-MNOP")
                .expect("fingerprint"),
            transport_public_key: vec![0x73; 32],
            transport_address_blob: vec![0x74; 16],
            identity_signature: vec![0x75; 64],
        },
        membership_credential: joiner_credential,
        resume_public_key_digest: [0x76; 32],
        security_commitment_id: [0x77; 32],
    };
    let event = MembershipEventV2::new(
        MEMBERSHIP_EVENT_FORMAT_V2,
        "lineage".to_owned(),
        None,
        0,
        [0x78; 16],
        MemberInstanceId::from_bytes([0x79; 32]),
        sponsor_credential.credential_id,
        ED25519_SIGNATURE_ALGORITHM_V1,
        MembershipOperationV2::AddDevice { admission },
        [0x7a; 32],
        [0x7b; 32],
        vec![0x7c],
        Some([0x7d; 32]),
        vec![0x7e; 64],
    );
    let base = BaseMembershipHistoryPosition {
        event_id: None,
        depth: 0,
        history_digest: [0x7f; 32],
    };
    let commitment = AdmissionSecurityCommitmentV1::new(
        ADMISSION_SECURITY_COMMITMENT_FORMAT_V1,
        "lineage".to_owned(),
        vec![0x80; 16],
        *admission_id.as_bytes(),
        base,
        [0x81; 32],
        1,
        0,
        1,
        [0x82; 32],
        [0x83; 32],
        [0x84; 32],
        [0x85; 32],
        [0x86; 32],
    )
    .expect("security commitment");
    AdmissionCandidateV1::new(
        AdmissionSignedMembershipHistory::from_bytes(vec![0x87; 64]).expect("history"),
        event,
        commitment,
        AdmissionMlsCommit::from_bytes(vec![0x88; 64]).expect("MLS commit"),
        AdmissionMlsWelcome::from_bytes(vec![0x89; 64]).expect("MLS welcome"),
        AdmissionContinuationRoute::from_bytes(continuation_route).expect("continuation route"),
    )
    .expect("Candidate")
}

fn prepared_request(
    admission_id: SpaceAdmissionId,
    candidate: &SpaceAdmissionEnvelopeV1,
) -> SpaceAdmissionEnvelopeV1 {
    let SpaceAdmissionBodyV1::Candidate(body) = candidate.body() else {
        panic!("candidate fixture kind");
    };
    let operation = &body.candidate_event().operation;
    let MembershipOperationV2::AddDevice { admission } = operation else {
        panic!("candidate fixture operation");
    };
    let proof = PreparedAdmissionProofV1::new(
        *admission_id.as_bytes(),
        body.security_commitment().lineage_id.clone(),
        body.security_commitment().base_history_position.clone(),
        body.candidate_event().event_id(),
        body.candidate_event().resulting_members_digest,
        body.security_commitment().security_commitment_id,
        admission.facts.member_instance,
        admission.membership_credential.credential_id,
        vec![0x91; 64],
    );
    SpaceAdmissionEnvelopeV1::new(
        admission_id,
        AdmissionRole::Joiner,
        1,
        message_id(0x92),
        Some(candidate.header().message_id()),
        SpaceAdmissionBodyV1::Prepared(AdmissionPreparedV1::new(proof)),
    )
    .expect("Prepared envelope")
}

mod crypto;
mod diagnostics;
mod protocol;
