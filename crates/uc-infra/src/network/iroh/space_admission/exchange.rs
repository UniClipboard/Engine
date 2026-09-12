use async_trait::async_trait;
use iroh::endpoint::{Connection, RecvStream, SendStream};
use sha2::{Digest, Sha256};
use tracing::Instrument;
use uc_application::deps::{
    AuthenticatedAdmissionExchangePort, AuthenticatedAdmissionReply, SpaceAdmissionTransportError,
};
use uc_core::membership::{
    AdmissionContinuationCredential, AdmissionPeerBinding, SpaceAdmissionEnvelopeV1,
    SpaceAdmissionId,
};
use uc_observability_contract::diagnostics::{
    operation_span, DiagnosticDomain, DiagnosticOperation, DiagnosticRole, DiagnosticSpanKind,
    OperationContext,
};

use super::super::space_admission_wire::{read_envelope, WireError};
use super::super::space_admission_wire::{
    write_envelope, write_typed, AuthenticatedEnvelopeV1, FrameKind, AUTH_FRAME_LIMIT, IO_DEADLINE,
};
use super::super::trace_context::inject_current;
use super::crypto::{calculate_mac, random_nonce, verify_mac};
use super::diagnostics::record_client_completion;
use super::errors::{map_application_close_code, map_reply_wire_error};
use std::time::Duration;

pub(super) struct EstablishedExchange {
    connection: Connection,
    send: SendStream,
    receive: RecvStream,
    admission_id: SpaceAdmissionId,
    binding: AdmissionPeerBinding,
    credential: AdmissionContinuationCredential,
    newly_established: Option<AdmissionContinuationCredential>,
}

impl EstablishedExchange {
    pub(super) fn new(
        connection: Connection,
        send: SendStream,
        receive: RecvStream,
        admission_id: SpaceAdmissionId,
        binding: AdmissionPeerBinding,
        credential: AdmissionContinuationCredential,
        newly_established: Option<AdmissionContinuationCredential>,
    ) -> Self {
        Self {
            connection,
            send,
            receive,
            admission_id,
            binding,
            credential,
            newly_established,
        }
    }
}

#[async_trait]
impl AuthenticatedAdmissionExchangePort for EstablishedExchange {
    fn peer_binding(&self) -> AdmissionPeerBinding {
        self.binding
    }

    fn take_newly_established_continuation(&mut self) -> Option<AdmissionContinuationCredential> {
        self.newly_established.take()
    }

    async fn exchange(
        mut self: Box<Self>,
        request: &SpaceAdmissionEnvelopeV1,
    ) -> Result<AuthenticatedAdmissionReply, SpaceAdmissionTransportError> {
        let canonical = request
            .encode_canonical_v1()
            .map_err(|_| SpaceAdmissionTransportError::ProtocolRejected)?;
        let digest: [u8; 32] = Sha256::digest(&canonical).into();
        let span = operation_span(OperationContext {
            domain: DiagnosticDomain::SpaceAdmission,
            operation: DiagnosticOperation::NetworkTransport,
            role: DiagnosticRole::Joiner,
            kind: DiagnosticSpanKind::Client,
        });
        let started = std::time::Instant::now();
        let result = async {
            let trace_context = inject_current();
            let nonce = random_nonce();
            let mac = calculate_mac(
                &self.credential,
                b"request",
                self.admission_id,
                self.binding.local_peer_id(),
                self.binding.remote_peer_id(),
                &nonce,
                &digest,
                trace_context.as_ref(),
            )
            .map_err(|_| SpaceAdmissionTransportError::AuthenticationRejected)?;
            write_envelope(
                &mut self.send,
                FrameKind::Request,
                &AuthenticatedEnvelopeV1 {
                    nonce,
                    canonical_envelope: canonical,
                    trace_context,
                    mac,
                },
            )
            .await
            .map_err(|_| SpaceAdmissionTransportError::Unavailable)?;
            let (wire, reply, reply_digest) =
                read_authenticated_reply(&mut self.receive, &self.connection).await?;
            verify_mac(
                &self.credential,
                b"reply",
                self.admission_id,
                self.binding.remote_peer_id(),
                self.binding.local_peer_id(),
                &wire.nonce,
                &reply_digest,
                wire.trace_context.as_ref(),
                &wire.mac,
            )
            .map_err(|_| SpaceAdmissionTransportError::AuthenticationRejected)?;
            write_typed(&mut self.send, FrameKind::Ack, &1u8, AUTH_FRAME_LIMIT)
                .await
                .map_err(|_| SpaceAdmissionTransportError::Unavailable)?;
            self.send
                .finish()
                .map_err(|_| SpaceAdmissionTransportError::Unavailable)?;
            let mut trailing = [0_u8; 1];
            let trailing_length = tokio::time::timeout(
                IO_DEADLINE,
                tokio::io::AsyncReadExt::read(&mut self.receive, &mut trailing),
            )
            .await
            .map_err(|_| SpaceAdmissionTransportError::Unavailable)?
            .map_err(|_| SpaceAdmissionTransportError::Unavailable)?;
            if trailing_length != 0 {
                return Err(SpaceAdmissionTransportError::ProtocolRejected);
            }
            AuthenticatedAdmissionReply::new(reply, reply_digest)
                .ok_or(SpaceAdmissionTransportError::ProtocolRejected)
        }
        .instrument(span.clone())
        .await;
        span.in_scope(|| {
            record_client_completion(
                DiagnosticOperation::NetworkTransport,
                started.elapsed(),
                result.as_ref().err(),
            )
        });
        result
    }
}

async fn read_authenticated_reply(
    receive: &mut RecvStream,
    connection: &Connection,
) -> Result<
    (AuthenticatedEnvelopeV1, SpaceAdmissionEnvelopeV1, [u8; 32]),
    SpaceAdmissionTransportError,
> {
    match read_envelope(receive, FrameKind::Reply).await {
        Ok(reply) => Ok(reply),
        Err(error @ WireError::UnsupportedLayout) => Err(map_reply_wire_error(error)),
        Err(error) => {
            let close_reason = match connection.close_reason() {
                Some(reason) => Some(reason),
                None => tokio::time::timeout(Duration::from_millis(100), connection.closed())
                    .await
                    .ok(),
            };
            match close_reason {
                Some(iroh::endpoint::ConnectionError::ApplicationClosed(close)) => {
                    match map_application_close_code(close.error_code.into_inner()) {
                        Some(mapped) => Err(mapped),
                        None => Err(map_reply_wire_error(error)),
                    }
                }
                _ => Err(map_reply_wire_error(error)),
            }
        }
    }
}
