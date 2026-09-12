//! Joiner 负责建立初次或恢复连接，并交付已认证交换。
use super::super::space_admission_wire::{
    read_typed, write_typed, ContinuationHelloV1, FrameKind, InitialHelloV1, OpaqueFinishV1,
    OpaqueResponseV1, AUTH_FRAME_LIMIT,
};
use super::connection::{connect, open_stream};
use super::crypto::{calculate_mac, copy_credential, peer_id, random_nonce};
use super::diagnostics::record_client_completion;
use super::exchange::EstablishedExchange;
use super::route::decode_route;
use crate::security::{SpaceAdmissionAuth, SpaceAdmissionAuthContext, SpaceAdmissionKe2};
use async_trait::async_trait;
use iroh::Endpoint;
use std::sync::Arc;
use std::time::Instant;
use tracing::Instrument;
use uc_application::deps::{
    AuthenticatedAdmissionExchangePort, SpaceAdmissionTransportError, SpaceAdmissionTransportPort,
};
use uc_core::membership::{
    AdmissionContinuationCredential, AdmissionEncryptedPasswordEquivalent, AdmissionPeerBinding,
    InvitationId, SpaceAdmissionId, SpaceAdmissionProtocolVersion, SpaceAdmissionRoute,
};
use uc_observability_contract::diagnostics::connectivity::complete_admission_connection_failure;
use uc_observability_contract::diagnostics::{
    operation_span, DiagnosticDomain, DiagnosticOperation, DiagnosticRole, DiagnosticSpanKind,
    OperationContext,
};

pub struct IrohSpaceAdmissionTransport {
    endpoint: Arc<Endpoint>,
}

impl IrohSpaceAdmissionTransport {
    pub fn new(endpoint: Arc<Endpoint>) -> Self {
        Self { endpoint }
    }
}

#[async_trait]
impl SpaceAdmissionTransportPort for IrohSpaceAdmissionTransport {
    async fn establish_initial(
        &self,
        admission_id: SpaceAdmissionId,
        route: &SpaceAdmissionRoute,
        password: &AdmissionEncryptedPasswordEquivalent,
    ) -> Result<Box<dyn AuthenticatedAdmissionExchangePort>, SpaceAdmissionTransportError> {
        let started = Instant::now();
        let span = operation_span(OperationContext {
            domain: DiagnosticDomain::SpaceAdmission,
            operation: DiagnosticOperation::SpaceAdmission,
            role: DiagnosticRole::Joiner,
            kind: DiagnosticSpanKind::Client,
        });
        let mut connection_failure = None;
        let result = async {
            let route = decode_route(route, true)?;
            let invitation_id = route
                .invitation_id
                .and_then(InvitationId::from_bytes)
                .ok_or(SpaceAdmissionTransportError::InvitationUnavailable)?;
            let local = peer_id(self.endpoint.id().as_bytes())?;
            let remote = peer_id(route.endpoint_addr.id.as_bytes())?;
            let binding = AdmissionPeerBinding::new(local, remote)
                .ok_or(SpaceAdmissionTransportError::AuthenticationRejected)?;
            let context = SpaceAdmissionAuthContext::new(
                SpaceAdmissionProtocolVersion::V1,
                admission_id,
                invitation_id,
                local,
                remote,
            );
            let (client, ke1) =
                SpaceAdmissionAuth::start_client_with_password_equivalent(password, &context)
                    .map_err(|_| SpaceAdmissionTransportError::AuthenticationRejected)?;
            let connection =
                connect(&self.endpoint, route.endpoint_addr)
                    .await
                    .map_err(|error| {
                        connection_failure = Some(error.category());
                        SpaceAdmissionTransportError::Deferred
                    })?;
            let (mut send, mut receive) = open_stream(&connection).await?;
            write_typed(
                &mut send,
                FrameKind::InitialHello,
                &InitialHelloV1 {
                    protocol_version: SpaceAdmissionProtocolVersion::V1.as_u16(),
                    admission_id: *admission_id.as_bytes(),
                    invitation_id: *invitation_id.as_bytes(),
                    joiner_peer_id: *local.as_bytes(),
                    ke1: ke1.encode_for_transport(),
                },
                AUTH_FRAME_LIMIT,
            )
            .await
            .map_err(|_| SpaceAdmissionTransportError::Unavailable)?;
            let response: OpaqueResponseV1 =
                read_typed(&mut receive, FrameKind::OpaqueResponse, AUTH_FRAME_LIMIT)
                    .await
                    .map_err(|_| SpaceAdmissionTransportError::AuthenticationRejected)?;
            if response.sponsor_peer_id != *remote.as_bytes() {
                return Err(SpaceAdmissionTransportError::AuthenticationRejected);
            }
            let ke2 = SpaceAdmissionKe2::decode_from_transport(&response.ke2)
                .map_err(|_| SpaceAdmissionTransportError::AuthenticationRejected)?;
            let (credential, ke3) = client
                .finish(&context, ke2)
                .map_err(|_| SpaceAdmissionTransportError::AuthenticationRejected)?;
            let credential = credential
                .into_core()
                .map_err(|_| SpaceAdmissionTransportError::AuthenticationRejected)?;
            write_typed(
                &mut send,
                FrameKind::OpaqueFinish,
                &OpaqueFinishV1 {
                    ke3: ke3.encode_for_transport(),
                },
                AUTH_FRAME_LIMIT,
            )
            .await
            .map_err(|_| SpaceAdmissionTransportError::Unavailable)?;
            let newly_established = copy_credential(&credential)?;
            Ok(Box::new(EstablishedExchange::new(
                connection,
                send,
                receive,
                admission_id,
                binding,
                credential,
                Some(newly_established),
            ))
                as Box<dyn AuthenticatedAdmissionExchangePort>)
        }
        .instrument(span.clone())
        .await;
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

    async fn resume(
        &self,
        admission_id: SpaceAdmissionId,
        route: &SpaceAdmissionRoute,
        binding: AdmissionPeerBinding,
        credential: &AdmissionContinuationCredential,
    ) -> Result<Box<dyn AuthenticatedAdmissionExchangePort>, SpaceAdmissionTransportError> {
        let started = Instant::now();
        let span = operation_span(OperationContext {
            domain: DiagnosticDomain::SpaceAdmission,
            operation: DiagnosticOperation::SpaceAdmission,
            role: DiagnosticRole::Joiner,
            kind: DiagnosticSpanKind::Client,
        });
        uc_observability_contract::diagnostics::describe_admission_connection(&span, true);
        let mut connection_failure = None;
        let result = async {
            let route = decode_route(route, false)?;
            let local = peer_id(self.endpoint.id().as_bytes())?;
            let remote = peer_id(route.endpoint_addr.id.as_bytes())?;
            if binding.local_peer_id() != local || binding.remote_peer_id() != remote {
                return Err(SpaceAdmissionTransportError::AuthenticationRejected);
            }
            let connection =
                connect(&self.endpoint, route.endpoint_addr)
                    .await
                    .map_err(|error| {
                        connection_failure = Some(error.category());
                        SpaceAdmissionTransportError::Deferred
                    })?;
            let (mut send, receive) = open_stream(&connection).await?;
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
            .map_err(|_| SpaceAdmissionTransportError::AuthenticationRejected)?;
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
            .map_err(|_| SpaceAdmissionTransportError::Unavailable)?;
            Ok(Box::new(EstablishedExchange::new(
                connection,
                send,
                receive,
                admission_id,
                binding,
                copy_credential(credential)?,
                None,
            ))
                as Box<dyn AuthenticatedAdmissionExchangePort>)
        }
        .instrument(span.clone())
        .await;
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
