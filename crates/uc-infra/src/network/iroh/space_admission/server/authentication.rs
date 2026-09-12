//! 完成身份与请求证明校验后，才允许交接请求并接纳远端关联。
use iroh::endpoint::Connection;
use uc_core::membership::{
    AdmissionChannelPeerId, AdmissionContinuationCredential, InvitationId, SpaceAdmissionId,
    SpaceAdmissionProtocolVersion,
};
use uc_observability_contract::diagnostics::connectivity::complete_admission_authentication_failure;

use super::super::super::space_admission_wire::{
    read_envelope, read_raw_with_limit, read_typed, write_typed, AuthenticatedEnvelopeV1,
    ContinuationHelloV1, FrameKind, InitialHelloV1, OpaqueFinishV1, OpaqueResponseV1,
    AUTH_FRAME_LIMIT, IO_DEADLINE,
};
use super::super::crypto::{peer_id, verify_mac};
use super::super::diagnostics::AuthenticationStep;
use super::super::errors::{map_request_wire_error, map_server_wire_error, HandlerError};
use super::IrohSpaceAdmissionHandler;
use crate::security::{
    SpaceAdmissionAuth, SpaceAdmissionAuthContext, SpaceAdmissionContinuationCredential,
    SpaceAdmissionKe1, SpaceAdmissionKe3,
};

pub(super) struct AuthenticatedRequest {
    pub(super) send: iroh::endpoint::SendStream,
    pub(super) receive: iroh::endpoint::RecvStream,
    pub(super) remote_peer_id: AdmissionChannelPeerId,
    pub(super) admission_id: SpaceAdmissionId,
    pub(super) credential: AdmissionContinuationCredential,
    pub(super) is_initial: bool,
    pub(super) wire: AuthenticatedEnvelopeV1,
    pub(super) envelope: uc_core::membership::SpaceAdmissionEnvelopeV1,
    pub(super) canonical_digest: [u8; 32],
}

impl IrohSpaceAdmissionHandler {
    pub(super) async fn authenticate(
        &self,
        connection: &Connection,
        deadline: tokio::time::Instant,
        connection_started: std::time::Instant,
    ) -> Result<AuthenticatedRequest, HandlerError> {
        let mut diagnostic_stage = AuthenticationStep::ReceiveHello;
        let authenticated = tokio::time::timeout_at(deadline, async {
            let remote_peer_id = peer_id(connection.remote_id().as_bytes()).map_err(|source| {
                HandlerError::AuthenticationProof {
                    source: anyhow::Error::new(source),
                }
            })?;
            let (mut send, mut receive) = tokio::time::timeout(IO_DEADLINE, connection.accept_bi())
                .await
                .map_err(|_| HandlerError::Timeout)?
                .map_err(|source| HandlerError::Transport {
                    source: anyhow::Error::new(source),
                })?;
            let (kind, payload) = read_raw_with_limit(&mut receive, AUTH_FRAME_LIMIT)
                .await
                .map_err(map_server_wire_error)?;
            let (admission_id, credential, is_initial) = match kind {
                FrameKind::InitialHello => {
                    let hello: InitialHelloV1 =
                        postcard::from_bytes(&payload).map_err(|_| HandlerError::Protocol)?;
                    let admission_id = SpaceAdmissionId::from_bytes(hello.admission_id)
                        .ok_or(HandlerError::Protocol)?;
                    let invitation_id = InvitationId::from_bytes(hello.invitation_id)
                        .ok_or(HandlerError::Protocol)?;
                    diagnostic_stage = AuthenticationStep::InitialVersion;
                    if hello.protocol_version != SpaceAdmissionProtocolVersion::V1.as_u16() {
                        return Err(HandlerError::Authentication);
                    }
                    diagnostic_stage = AuthenticationStep::InitialIdentity;
                    if hello.joiner_peer_id != *remote_peer_id.as_bytes() {
                        return Err(HandlerError::Authentication);
                    }
                    diagnostic_stage = AuthenticationStep::InitialCredential;
                    let material = self
                        .credentials
                        .resolve_initial(invitation_id, admission_id)
                        .await
                        .map_err(HandlerError::Credential)?;
                    diagnostic_stage = AuthenticationStep::InitialProof;
                    let context = SpaceAdmissionAuthContext::new(
                        SpaceAdmissionProtocolVersion::V1,
                        admission_id,
                        invitation_id,
                        remote_peer_id,
                        self.local_peer_id,
                    );
                    let ke1 =
                        SpaceAdmissionKe1::decode_from_transport(&hello.ke1).map_err(|source| {
                            HandlerError::AuthenticationProof {
                                source: anyhow::Error::new(source),
                            }
                        })?;
                    let (server, ke2) = SpaceAdmissionAuth::start_server(
                        &material.server_setup,
                        &material.registration,
                        &context,
                        ke1,
                    )
                    .map_err(|source| HandlerError::AuthenticationProof {
                        source: anyhow::Error::new(source),
                    })?;
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
                    .map_err(map_server_wire_error)?;
                    let finish: OpaqueFinishV1 =
                        read_typed(&mut receive, FrameKind::OpaqueFinish, AUTH_FRAME_LIMIT)
                            .await
                            .map_err(map_server_wire_error)?;
                    let ke3 = SpaceAdmissionKe3::decode_from_transport(&finish.ke3).map_err(
                        |source| HandlerError::AuthenticationProof {
                            source: anyhow::Error::new(source),
                        },
                    )?;
                    let credential = server
                        .finish(&context, ke3)
                        .and_then(SpaceAdmissionContinuationCredential::into_core)
                        .map_err(|source| HandlerError::AuthenticationProof {
                            source: anyhow::Error::new(source),
                        })?;
                    (admission_id, credential, true)
                }
                FrameKind::ContinuationHello => {
                    let hello: ContinuationHelloV1 =
                        postcard::from_bytes(&payload).map_err(|_| HandlerError::Protocol)?;
                    let admission_id = SpaceAdmissionId::from_bytes(hello.admission_id)
                        .ok_or(HandlerError::Protocol)?;
                    diagnostic_stage = AuthenticationStep::ContinuationIdentity;
                    if hello.local_peer_id != *remote_peer_id.as_bytes()
                        || hello.remote_peer_id != *self.local_peer_id.as_bytes()
                    {
                        return Err(HandlerError::Authentication);
                    }
                    diagnostic_stage = AuthenticationStep::ContinuationCredential;
                    let credential = self
                        .credentials
                        .load_continuation(admission_id)
                        .await
                        .map_err(HandlerError::Credential)?;
                    diagnostic_stage = AuthenticationStep::ContinuationProof;
                    verify_mac(
                        &credential,
                        b"resume",
                        admission_id,
                        remote_peer_id,
                        self.local_peer_id,
                        &hello.nonce,
                        &hello.request_digest,
                        None,
                        &hello.mac,
                    )?;
                    (admission_id, credential, false)
                }
                _ => return Err(HandlerError::Protocol),
            };

            diagnostic_stage = AuthenticationStep::ReceiveRequest;
            let (wire, envelope, canonical_digest) =
                read_envelope(&mut receive, FrameKind::Request)
                    .await
                    .map_err(map_request_wire_error)?;
            diagnostic_stage = AuthenticationStep::RequestIdentity;
            if envelope.header().admission_id() != admission_id {
                return Err(HandlerError::Authentication);
            }
            diagnostic_stage = AuthenticationStep::RequestProof;
            verify_mac(
                &credential,
                b"request",
                admission_id,
                remote_peer_id,
                self.local_peer_id,
                &wire.nonce,
                &canonical_digest,
                wire.trace_context.as_ref(),
                &wire.mac,
            )?;
            Ok::<_, HandlerError>((
                send,
                receive,
                remote_peer_id,
                admission_id,
                credential,
                is_initial,
                wire,
                envelope,
                canonical_digest,
            ))
        })
        .await;
        let authenticated = match authenticated {
            Ok(Ok(authenticated)) => authenticated,
            Ok(Err(error)) => {
                complete_admission_authentication_failure(
                    diagnostic_stage.failure(&error),
                    connection_started.elapsed(),
                );
                return Err(error);
            }
            Err(_) => {
                let error = HandlerError::Timeout;
                complete_admission_authentication_failure(
                    diagnostic_stage.failure(&error),
                    connection_started.elapsed(),
                );
                return Err(error);
            }
        };
        let (
            send,
            receive,
            remote_peer_id,
            admission_id,
            credential,
            is_initial,
            wire,
            envelope,
            canonical_digest,
        ) = authenticated;
        Ok(AuthenticatedRequest {
            send,
            receive,
            remote_peer_id,
            admission_id,
            credential,
            is_initial,
            wire,
            envelope,
            canonical_digest,
        })
    }
}
