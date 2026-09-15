use async_trait::async_trait;
use iroh::endpoint::{Connection, RecvStream, SendStream};
use sha2::{Digest, Sha256};
use tracing::Instrument;
use uc_application::deps::{
    AuthenticatedAdmissionExchangePort, AuthenticatedAdmissionReply, SpaceAdmissionTransportError,
};
use uc_core::membership::{
    AdmissionContinuationCredential, AdmissionPeerBinding, SpaceAdmissionEnvelopeV1,
    SpaceAdmissionId, SpaceAdmissionProtocolVersion,
};
use uc_observability_contract::diagnostics::connectivity::{
    AdmissionExchangeFailure, AdmissionExchangeObservation, AdmissionExchangeSide,
    AdmissionExchangeStep, AdmissionNetworkPoint,
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
use super::diagnostics::{client_completion, io_failure, record_network_snapshot, wire_failure};
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
        let span = operation_span(OperationContext {
            domain: DiagnosticDomain::SpaceAdmission,
            operation: DiagnosticOperation::NetworkTransport,
            role: DiagnosticRole::Joiner,
            kind: DiagnosticSpanKind::Client,
        });
        let started = std::time::Instant::now();
        let mut progress =
            span.in_scope(|| AdmissionExchangeObservation::begin(AdmissionExchangeSide::Joiner));
        let result = async {
            record_network_snapshot(
                &self.connection,
                AdmissionExchangeSide::Joiner,
                AdmissionNetworkPoint::ExchangeStarted,
            );
            progress.start_step(AdmissionExchangeStep::PrepareRequest);
            if request.header().protocol_version() != SpaceAdmissionProtocolVersion::V2 {
                progress.fail(AdmissionExchangeFailure::PeerUpgradeRequired);
                return Err(SpaceAdmissionTransportError::PeerUpgradeRequired);
            }
            let canonical = request
                .encode_canonical_v1()
                .inspect_err(|_| progress.fail(AdmissionExchangeFailure::InvalidMessage))
                .map_err(|_| SpaceAdmissionTransportError::ProtocolRejected)?;
            let digest: [u8; 32] = Sha256::digest(&canonical).into();
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
            .inspect_err(|_| progress.fail(AdmissionExchangeFailure::AuthenticationRejected))
            .map_err(|_| SpaceAdmissionTransportError::AuthenticationRejected)?;
            progress.start_step(AdmissionExchangeStep::SendRequest);
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
            .inspect_err(|error| progress.fail(wire_failure(error)))
            .map_err(|_| SpaceAdmissionTransportError::Unavailable)?;
            progress.start_step(AdmissionExchangeStep::ReceiveReply);
            let (wire, reply, reply_digest) =
                read_authenticated_reply(&mut self.receive, &self.connection, &mut progress)
                    .await?;
            progress.start_step(AdmissionExchangeStep::ValidateReply);
            if reply.header().protocol_version() != request.header().protocol_version() {
                progress.fail(AdmissionExchangeFailure::PeerUpgradeRequired);
                return Err(SpaceAdmissionTransportError::PeerUpgradeRequired);
            }
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
            .inspect_err(|_| progress.fail(AdmissionExchangeFailure::AuthenticationRejected))
            .map_err(|_| SpaceAdmissionTransportError::AuthenticationRejected)?;
            progress.start_step(AdmissionExchangeStep::SendAcknowledgement);
            write_typed(&mut self.send, FrameKind::Ack, &1u8, AUTH_FRAME_LIMIT)
                .await
                .inspect_err(|error| progress.fail(wire_failure(error)))
                .map_err(|_| SpaceAdmissionTransportError::Unavailable)?;
            progress.start_step(AdmissionExchangeStep::FinishSend);
            self.send
                .finish()
                .inspect_err(|_| progress.fail(AdmissionExchangeFailure::ConnectionClosed))
                .map_err(|_| SpaceAdmissionTransportError::Unavailable)?;
            progress.start_step(AdmissionExchangeStep::WaitPeerFinish);
            let mut trailing = [0_u8; 1];
            let trailing_length = tokio::time::timeout(
                IO_DEADLINE,
                tokio::io::AsyncReadExt::read(&mut self.receive, &mut trailing),
            )
            .await
            .inspect_err(|_| progress.fail(AdmissionExchangeFailure::TimedOut))
            .map_err(|_| SpaceAdmissionTransportError::Unavailable)?
            .inspect_err(|error| progress.fail(io_failure(error)))
            .map_err(|_| SpaceAdmissionTransportError::Unavailable)?;
            if trailing_length != 0 {
                progress.fail(AdmissionExchangeFailure::InvalidMessage);
                return Err(SpaceAdmissionTransportError::ProtocolRejected);
            }
            AuthenticatedAdmissionReply::new(reply, reply_digest).ok_or_else(|| {
                progress.fail(AdmissionExchangeFailure::InvalidMessage);
                SpaceAdmissionTransportError::ProtocolRejected
            })
        }
        .instrument(span.clone())
        .await;
        span.in_scope(|| {
            record_network_snapshot(
                &self.connection,
                AdmissionExchangeSide::Joiner,
                AdmissionNetworkPoint::ExchangeFinished,
            )
        });
        progress.finish(client_completion(
            DiagnosticOperation::NetworkTransport,
            started.elapsed(),
            result.as_ref().err(),
        ));
        result
    }
}

async fn read_authenticated_reply(
    receive: &mut RecvStream,
    connection: &Connection,
    progress: &mut AdmissionExchangeObservation,
) -> Result<
    (AuthenticatedEnvelopeV1, SpaceAdmissionEnvelopeV1, [u8; 32]),
    SpaceAdmissionTransportError,
> {
    match read_envelope(receive, FrameKind::Reply).await {
        Ok(reply) => Ok(reply),
        Err(error @ WireError::UnsupportedLayout) => {
            progress.fail(wire_failure(&error));
            Err(map_reply_wire_error(error))
        }
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
                        Some(mapped) => {
                            match mapped {
                                SpaceAdmissionTransportError::AuthenticationRejected => {
                                    progress.fail(AdmissionExchangeFailure::AuthenticationRejected)
                                }
                                SpaceAdmissionTransportError::PeerUpgradeRequired => {
                                    progress.fail(AdmissionExchangeFailure::PeerUpgradeRequired)
                                }
                                _ => progress.fail(AdmissionExchangeFailure::ConnectionClosed),
                            }
                            Err(mapped)
                        }
                        None => {
                            progress.fail(wire_failure(&error));
                            Err(map_reply_wire_error(error))
                        }
                    }
                }
                _ => {
                    progress.fail(wire_failure(&error));
                    Err(map_reply_wire_error(error))
                }
            }
        }
    }
}
