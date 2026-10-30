//! Joiner 负责建立初次或恢复连接，并交付已认证交换。
use super::connection::acquire;
use super::crypto::{calculate_mac, copy_credential, peer_id, random_nonce};
use super::diagnostics::{
    client_completion, record_client_completion, record_network_snapshot, response_failure,
    response_step_failure, wire_failure,
};
use super::errors::{application_close_code, map_initial_hello_close_code};
use super::exchange::EstablishedExchange;
use super::managed::{manager, AdmissionManager, EXCHANGE_DEADLINE, MAX_EXCHANGES};
use super::route::decode_route;
use async_trait::async_trait;
use iroh::Endpoint;
use std::sync::Arc;
use std::time::Instant;
use tokio::time::{timeout_at, Instant as TokioInstant};
use tracing::Instrument;
use uc_application::deps::{
    AuthenticatedAdmissionExchangePort, SpaceAdmissionTransportError, SpaceAdmissionTransportPort,
};
use uc_core::membership::{
    AdmissionAttemptContractV2, AdmissionAttemptTimeline, AdmissionContinuationCredential,
    AdmissionEncryptedPasswordEquivalent, AdmissionPeerBinding, InvitationId, SpaceAdmissionId,
    SpaceAdmissionProtocolVersion, SpaceAdmissionRoute,
};
use uc_infra_crypto::space_admission_auth::{
    SpaceAdmissionAuth, SpaceAdmissionAuthContext, SpaceAdmissionKe2,
};
use uc_observability_contract::diagnostics::connectivity::{
    complete_admission_connection_failure, complete_admission_response_failure,
    AdmissionAttemptRef, AdmissionExchangeFailure, AdmissionExchangeObservation,
    AdmissionExchangeSide, AdmissionExchangeStep, AdmissionNetworkPoint, AdmissionResponseFailure,
};
use uc_observability_contract::diagnostics::{
    operation_span, DiagnosticDomain, DiagnosticOperation, DiagnosticRole, DiagnosticSpanKind,
    OperationContext,
};
use uc_sync_protocol::space_admission::{
    read_typed, write_typed, ContinuationHelloV1, FrameKind, InitialHelloV2, OpaqueFinishV1,
    OpaqueResponseV1, AUTH_FRAME_LIMIT,
};

pub struct IrohSpaceAdmissionTransport {
    pub(super) endpoint: Arc<Endpoint>,
    pub(super) manager: AdmissionManager,
}

impl IrohSpaceAdmissionTransport {
    pub fn new(endpoint: Arc<Endpoint>) -> Self {
        let manager = manager(endpoint.clone(), MAX_EXCHANGES);
        Self { endpoint, manager }
    }
}

#[async_trait]
impl SpaceAdmissionTransportPort for IrohSpaceAdmissionTransport {
    async fn establish_initial(
        &self,
        admission_id: SpaceAdmissionId,
        attempt_timeline: AdmissionAttemptTimeline,
        route: &SpaceAdmissionRoute,
        password: &AdmissionEncryptedPasswordEquivalent,
    ) -> Result<Box<dyn AuthenticatedAdmissionExchangePort>, SpaceAdmissionTransportError> {
        let started = Instant::now();
        let deadline = TokioInstant::now() + EXCHANGE_DEADLINE;
        let span = operation_span(OperationContext {
            domain: DiagnosticDomain::SpaceAdmission,
            operation: DiagnosticOperation::SpaceAdmission,
            role: DiagnosticRole::Joiner,
            kind: DiagnosticSpanKind::Client,
        });
        let mut connection_failure = None;
        let mut read_failure = None;
        let mut initial_progress = None;
        let result = timeout_at(
            deadline,
            async {
                let route = decode_route(route, true)?;
                let invitation_id = route
                    .invitation_id
                    .and_then(InvitationId::from_bytes)
                    .ok_or(SpaceAdmissionTransportError::InvitationUnavailable)?;
                let local = peer_id(self.endpoint.id().as_bytes())?;
                let remote = peer_id(route.endpoint_addr.id.as_bytes())?;
                let binding = AdmissionPeerBinding::new(local, remote)
                    .ok_or_else(SpaceAdmissionTransportError::authentication_rejected)?;
                let attempt_contract = AdmissionAttemptContractV2::new(
                    admission_id,
                    invitation_id,
                    local,
                    remote,
                    attempt_timeline.started_at_ms(),
                    attempt_timeline.expires_at_ms(),
                )
                .map_err(SpaceAdmissionTransportError::authentication_rejected_from)?;
                let context = SpaceAdmissionAuthContext::with_attempt_contract(
                    admission_id,
                    invitation_id,
                    local,
                    remote,
                    attempt_contract.digest(),
                )
                .ok_or_else(SpaceAdmissionTransportError::authentication_rejected)?;
                let (client, ke1) =
                    SpaceAdmissionAuth::start_client_with_password_equivalent(password, &context)
                        .map_err(SpaceAdmissionTransportError::authentication_rejected_from)?;
                let (connection, lease) = acquire(&self.manager, route.endpoint_addr)
                    .await
                    .map_err(|error| {
                        connection_failure = Some(super::connection::dial_failure(&error));
                        SpaceAdmissionTransportError::deferred_from(error)
                    })?;
                let progress = initial_progress.insert(
                    AdmissionExchangeObservation::begin_steps_only(AdmissionExchangeSide::Joiner)
                        .with_attempt(AdmissionAttemptRef::derive(admission_id.as_bytes())),
                );
                progress.start_step(AdmissionExchangeStep::OpenStream);
                let stream = lease
                    .open_bi(deadline)
                    .await
                    .inspect_err(|_| progress.fail(AdmissionExchangeFailure::ConnectionClosed))
                    .map_err(SpaceAdmissionTransportError::deferred_from)?;
                let ((mut send, mut receive), work) = stream.into_parts();
                progress.start_step(AdmissionExchangeStep::SendInitialHello);
                write_typed(
                    &mut send,
                    FrameKind::InitialHello,
                    &InitialHelloV2 {
                        protocol_version: SpaceAdmissionProtocolVersion::CURRENT.as_u16(),
                        admission_id: *admission_id.as_bytes(),
                        invitation_id: *invitation_id.as_bytes(),
                        joiner_peer_id: *local.as_bytes(),
                        attempt_started_at_ms: attempt_timeline.started_at_ms(),
                        attempt_expires_at_ms: attempt_timeline.expires_at_ms(),
                        ke1: ke1.encode_for_transport(),
                    },
                    AUTH_FRAME_LIMIT,
                )
                .await
                .inspect_err(|error| progress.fail(wire_failure(error)))
                .map_err(SpaceAdmissionTransportError::unavailable_from)?;
                progress.start_step(AdmissionExchangeStep::ReceiveOpaqueResponse);
                let response: OpaqueResponseV1 =
                    match read_typed(&mut receive, FrameKind::OpaqueResponse, AUTH_FRAME_LIMIT)
                        .await
                    {
                        Ok(response) => response,
                        Err(error) => {
                            // Classify transport interruption separately from explicit authentication rejection.
                            let close_code = application_close_code(&connection).await;
                            let failure = response_failure(&error, close_code);
                            read_failure = Some(failure);
                            progress.fail(response_step_failure(failure));
                            record_network_snapshot(
                                &connection,
                                AdmissionExchangeSide::Joiner,
                                AdmissionNetworkPoint::ExchangeFinished,
                            );
                            if failure == AdmissionResponseFailure::ResponseTimeout {
                                return Err(SpaceAdmissionTransportError::SponsorUnresponsive);
                            }
                            if matches!(
                                failure,
                                AdmissionResponseFailure::StreamReset
                                    | AdmissionResponseFailure::ConnectionLost
                            ) {
                                return Err(SpaceAdmissionTransportError::unavailable_from(error));
                            }
                            return Err(close_code
                                .and_then(map_initial_hello_close_code)
                                .unwrap_or(
                                    SpaceAdmissionTransportError::authentication_rejected(),
                                ));
                        }
                    };
                progress.start_step(AdmissionExchangeStep::ValidateReply);
                if response.sponsor_peer_id != *remote.as_bytes() {
                    progress.fail(AdmissionExchangeFailure::AuthenticationRejected);
                    return Err(SpaceAdmissionTransportError::authentication_rejected());
                }
                let ke2 = SpaceAdmissionKe2::decode_from_transport(&response.ke2)
                    .inspect_err(|_| progress.fail(AdmissionExchangeFailure::InvalidMessage))
                    .map_err(SpaceAdmissionTransportError::authentication_rejected_from)?;
                let (credential, ke3) = client
                    .finish(&context, ke2)
                    .inspect_err(|_| {
                        progress.fail(AdmissionExchangeFailure::AuthenticationRejected)
                    })
                    .map_err(SpaceAdmissionTransportError::authentication_rejected_from)?;
                let credential = credential
                    .into_core()
                    .inspect_err(|_| {
                        progress.fail(AdmissionExchangeFailure::AuthenticationRejected)
                    })
                    .map_err(SpaceAdmissionTransportError::authentication_rejected_from)?;
                progress.start_step(AdmissionExchangeStep::SendOpaqueFinish);
                write_typed(
                    &mut send,
                    FrameKind::OpaqueFinish,
                    &OpaqueFinishV1 {
                        ke3: ke3.encode_for_transport(),
                    },
                    AUTH_FRAME_LIMIT,
                )
                .await
                .inspect_err(|error| progress.fail(wire_failure(error)))
                .map_err(SpaceAdmissionTransportError::unavailable_from)?;
                if let Some(progress) = initial_progress.take() {
                    progress.end_steps();
                }
                let newly_established = copy_credential(&credential)?;
                Ok(Box::new(EstablishedExchange::new(
                    connection,
                    send,
                    receive,
                    work,
                    deadline,
                    admission_id,
                    binding,
                    credential,
                    Some(newly_established),
                ))
                    as Box<dyn AuthenticatedAdmissionExchangePort>)
            }
            .instrument(span.clone()),
        )
        .await
        .inspect_err(|_| {
            if let Some(progress) = initial_progress.as_mut() {
                progress.fail(AdmissionExchangeFailure::TimedOut);
            }
        })
        .map_err(SpaceAdmissionTransportError::unavailable_from)
        .and_then(|result| result);
        if let Some(progress) = initial_progress {
            progress.end_steps();
        }
        span.in_scope(|| {
            if let Some(failure) = connection_failure {
                complete_admission_connection_failure(failure, started.elapsed());
                return;
            }
            match read_failure {
                Some(failure) => complete_admission_response_failure(
                    failure,
                    client_completion(
                        DiagnosticOperation::SpaceAdmission,
                        started.elapsed(),
                        result.as_ref().err(),
                    ),
                ),
                None => record_client_completion(
                    DiagnosticOperation::SpaceAdmission,
                    started.elapsed(),
                    result.as_ref().err(),
                ),
            }
        });
        result
    }

    async fn resume(
        &self,
        admission_id: SpaceAdmissionId,
        route: &SpaceAdmissionRoute,
        binding: AdmissionPeerBinding,
        credential: &AdmissionContinuationCredential,
    ) -> Result<Box<dyn AuthenticatedAdmissionExchangePort>, SpaceAdmissionTransportError> {
        let started = Instant::now();
        let deadline = TokioInstant::now() + EXCHANGE_DEADLINE;
        let span = operation_span(OperationContext {
            domain: DiagnosticDomain::SpaceAdmission,
            operation: DiagnosticOperation::SpaceAdmission,
            role: DiagnosticRole::Joiner,
            kind: DiagnosticSpanKind::Client,
        });
        uc_observability_contract::diagnostics::describe_admission_connection(&span, true);
        let mut connection_failure = None;
        let result = timeout_at(
            deadline,
            async {
                let route = decode_route(route, false)?;
                let local = peer_id(self.endpoint.id().as_bytes())?;
                let remote = peer_id(route.endpoint_addr.id.as_bytes())?;
                if binding.local_peer_id() != local || binding.remote_peer_id() != remote {
                    return Err(SpaceAdmissionTransportError::authentication_rejected());
                }
                let (connection, lease) = acquire(&self.manager, route.endpoint_addr)
                    .await
                    .map_err(|error| {
                        connection_failure = Some(super::connection::dial_failure(&error));
                        SpaceAdmissionTransportError::deferred_from(error)
                    })?;
                let stream = lease
                    .open_bi(deadline)
                    .await
                    .map_err(SpaceAdmissionTransportError::deferred_from)?;
                let ((mut send, receive), work) = stream.into_parts();
                let nonce = random_nonce();
                let request_digest = [0u8; 32];
                let mac = calculate_mac(
                    credential,
                    b"resume",
                    admission_id,
                    local,
                    remote,
                    &nonce,
                    &request_digest,
                    None,
                )
                .map_err(SpaceAdmissionTransportError::authentication_rejected_from)?;
                write_typed(
                    &mut send,
                    FrameKind::ContinuationHello,
                    &ContinuationHelloV1 {
                        admission_id: *admission_id.as_bytes(),
                        local_peer_id: *local.as_bytes(),
                        remote_peer_id: *remote.as_bytes(),
                        nonce,
                        request_digest,
                        mac,
                    },
                    AUTH_FRAME_LIMIT,
                )
                .await
                .map_err(SpaceAdmissionTransportError::unavailable_from)?;
                Ok(Box::new(EstablishedExchange::new(
                    connection,
                    send,
                    receive,
                    work,
                    deadline,
                    admission_id,
                    binding,
                    copy_credential(credential)?,
                    None,
                ))
                    as Box<dyn AuthenticatedAdmissionExchangePort>)
            }
            .instrument(span.clone()),
        )
        .await
        .map_err(SpaceAdmissionTransportError::unavailable_from)
        .and_then(|result| result);
        span.in_scope(|| {
            if let Some(failure) = connection_failure {
                complete_admission_connection_failure(failure, started.elapsed());
                return;
            }
            record_client_completion(
                DiagnosticOperation::SpaceAdmission,
                started.elapsed(),
                result.as_ref().err(),
            )
        });
        result
    }
}
