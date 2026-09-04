//! Bounded membership-history exchange on authenticated Iroh connections.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh::{Endpoint, EndpointAddr};
use serde::{Deserialize, Serialize};
use tracing::Instrument;
use uc_application::deps::{
    RestrictedMembershipDelivery, RestrictedMembershipDeliveryError,
    RestrictedMembershipDeliveryPort,
};
use uc_core::ids::DeviceId;
use uc_core::membership::{
    MemberRepositoryPort, MembershipHistoryAckV3, MembershipHistoryExchangeEndpointPort,
    MembershipHistoryExchangeError, MembershipHistoryExchangePort, MembershipHistoryMessage,
    MAX_MEMBERSHIP_HISTORY_FRAME_SIZE,
};
use uc_core::ports::security::IdentityFingerprintFactoryPort;
use uc_core::ports::PeerAddressRepositoryPort;
use uc_observability_contract::diagnostics::{
    complete_operation, operation_span, DiagnosticDomain, DiagnosticErrorType, DiagnosticOperation,
    DiagnosticRole, DiagnosticSpanKind, OperationCompletion, OperationContext,
};

use super::connect_with_staggered_retry;
use super::peer_address_resolver::PeerAddressResolver;
use super::trace_context::{inject_current, set_remote_parent, WireTraceContext};

pub const MEMBERSHIP_HISTORY_EXCHANGE_ALPN: &[u8] = b"uniclipboard/membership-history/3";

const WIRE_VERSION: u8 = 3;
const REQUEST_LAYOUT_MARKER: &[u8; 4] = b"UCT1";
const IO_TIMEOUT: Duration = Duration::from_secs(10);
const ACCEPTED: u8 = 1;
const REJECTED: u8 = 2;

#[derive(Serialize, Deserialize)]
struct WireMembershipHistoryRequest {
    trace_context: Option<WireTraceContext>,
    message: MembershipHistoryMessage,
}

pub struct IrohMembershipHistoryExchangeAdapter {
    endpoint: Arc<Endpoint>,
    peer_address_resolver: PeerAddressResolver,
}

impl IrohMembershipHistoryExchangeAdapter {
    pub fn new(
        endpoint: Arc<Endpoint>,
        peer_addr_repo: Arc<dyn PeerAddressRepositoryPort>,
    ) -> Self {
        Self {
            endpoint,
            peer_address_resolver: PeerAddressResolver::new(peer_addr_repo),
        }
    }

    pub(crate) fn handler(
        &self,
        member_repo: Arc<dyn MemberRepositoryPort>,
        fingerprint_factory: Arc<dyn IdentityFingerprintFactoryPort>,
        endpoint: Arc<dyn MembershipHistoryExchangeEndpointPort>,
    ) -> IrohMembershipHistoryExchangeHandler {
        IrohMembershipHistoryExchangeHandler {
            state: Arc::new(HandlerState {
                member_repo,
                fingerprint_factory,
                endpoint,
            }),
        }
    }

    async fn resolve_addr(&self, recipient: &DeviceId) -> Option<EndpointAddr> {
        match self.peer_address_resolver.resolve(recipient).await {
            Ok(address) => address,
            Err(error) => {
                tracing::warn!(
                    error_kind = error.kind(),
                    "membership history address resolution failed"
                );
                None
            }
        }
    }
}

#[async_trait]
impl MembershipHistoryExchangePort for IrohMembershipHistoryExchangeAdapter {
    async fn exchange_membership_history(
        &self,
        recipient: &DeviceId,
        message: MembershipHistoryMessage,
    ) -> Result<MembershipHistoryMessage, MembershipHistoryExchangeError> {
        let payload = encode_request(message)?;
        let address = self
            .resolve_addr(recipient)
            .await
            .ok_or(MembershipHistoryExchangeError::Offline)?;
        let connection = connect_with_staggered_retry(
            Arc::clone(&self.endpoint),
            address,
            MEMBERSHIP_HISTORY_EXCHANGE_ALPN,
            "membership-history",
        )
        .await
        .map_err(|_| MembershipHistoryExchangeError::Offline)?;
        let (mut send, mut receive) = tokio::time::timeout(IO_TIMEOUT, connection.open_bi())
            .await
            .map_err(|_| MembershipHistoryExchangeError::Transport)?
            .map_err(|_| MembershipHistoryExchangeError::Transport)?;
        write_message(&mut send, &payload).await?;
        let accepted = read_byte(&mut receive).await?;
        if accepted == REJECTED {
            return Err(MembershipHistoryExchangeError::Rejected);
        }
        if accepted != ACCEPTED {
            return Err(MembershipHistoryExchangeError::Transport);
        }
        let response = read_message(&mut receive).await?;
        decode_message(&response)
    }
}

#[async_trait]
impl RestrictedMembershipDeliveryPort for IrohMembershipHistoryExchangeAdapter {
    async fn deliver_restricted_membership(
        &self,
        peer: &DeviceId,
        delivery: &RestrictedMembershipDelivery,
    ) -> Result<(), RestrictedMembershipDeliveryError> {
        let message = match delivery {
            RestrictedMembershipDelivery::Event(event) => {
                MembershipHistoryMessage::RestrictedEventV3(event.clone())
            }
            RestrictedMembershipDelivery::Decision(decision) => {
                MembershipHistoryMessage::RestrictedDecisionV3(decision.clone())
            }
        };
        match self.exchange_membership_history(peer, message).await {
            Ok(MembershipHistoryMessage::AckV3(
                MembershipHistoryAckV3::RestrictedConsistent
                | MembershipHistoryAckV3::RestrictedApplied,
            )) => Ok(()),
            Ok(MembershipHistoryMessage::AckV3(
                MembershipHistoryAckV3::Invalid | MembershipHistoryAckV3::Diverged,
            ))
            | Ok(MembershipHistoryMessage::SuffixPageV3(_))
            | Ok(MembershipHistoryMessage::SummaryV3(_))
            | Ok(MembershipHistoryMessage::RequestSuffixV3(_))
            | Ok(MembershipHistoryMessage::RequestConflictEvidenceV3(_))
            | Ok(MembershipHistoryMessage::ConflictEvidenceV3(_))
            | Ok(MembershipHistoryMessage::AckV3(
                MembershipHistoryAckV3::Continue { .. } | MembershipHistoryAckV3::Confirmed { .. },
            ))
            | Ok(MembershipHistoryMessage::RestrictedEventV3(_))
            | Ok(MembershipHistoryMessage::RestrictedDecisionV3(_)) => {
                Err(RestrictedMembershipDeliveryError::Rejected)
            }
            Err(MembershipHistoryExchangeError::Offline)
            | Err(MembershipHistoryExchangeError::Transport) => {
                Err(RestrictedMembershipDeliveryError::Deferred)
            }
            Err(MembershipHistoryExchangeError::Rejected) => {
                Err(RestrictedMembershipDeliveryError::Rejected)
            }
        }
    }
}

#[derive(Clone)]
pub struct IrohMembershipHistoryExchangeHandler {
    state: Arc<HandlerState>,
}

impl std::fmt::Debug for IrohMembershipHistoryExchangeHandler {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IrohMembershipHistoryExchangeHandler")
            .finish_non_exhaustive()
    }
}

impl ProtocolHandler for IrohMembershipHistoryExchangeHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let (mut send, mut receive) =
            match tokio::time::timeout(IO_TIMEOUT, connection.accept_bi()).await {
                Ok(Ok(streams)) => streams,
                _ => return Ok(()),
            };
        let message = match read_message(&mut receive).await {
            Ok(message) => message,
            Err(_) => {
                reject(&mut send).await;
                return Ok(());
            }
        };
        let request = match decode_request(&message) {
            Ok(request) => request,
            Err(_) => {
                reject(&mut send).await;
                return Ok(());
            }
        };
        let Some(source_device) = self
            .resolve_source_device(connection.remote_id().as_bytes(), &request.message)
            .await
        else {
            reject(&mut send).await;
            return Ok(());
        };
        let span = operation_span(OperationContext {
            domain: DiagnosticDomain::SpaceMembership,
            operation: DiagnosticOperation::MembershipHistorySync,
            role: DiagnosticRole::Member,
            kind: DiagnosticSpanKind::Server,
            flow: None,
        });
        let _ = set_remote_parent(&span, request.trace_context.as_ref());
        let started = Instant::now();
        let result = async {
            let response = self
                .state
                .endpoint
                .handle_membership_history_exchange(&source_device, request.message)
                .await
                .map_err(|error| history_endpoint_error_type(&error))?;
            let payload =
                encode_message(&response).map_err(|_| DiagnosticErrorType::DecodeFailed)?;
            send.write_all(&[ACCEPTED])
                .await
                .map_err(|_| DiagnosticErrorType::StreamFailed)?;
            write_message(&mut send, &payload)
                .await
                .map_err(|_| DiagnosticErrorType::StreamFailed)
        }
        .instrument(span.clone())
        .await;
        span.in_scope(|| record_server_completion(started.elapsed(), result.as_ref().err()));
        if result.is_err() {
            reject(&mut send).await;
        }
        let _ = connection.closed().await;
        Ok(())
    }
}

fn history_endpoint_error_type(error: &MembershipHistoryExchangeError) -> DiagnosticErrorType {
    match error {
        MembershipHistoryExchangeError::Offline => DiagnosticErrorType::Unavailable,
        MembershipHistoryExchangeError::Rejected => DiagnosticErrorType::PeerRejected,
        MembershipHistoryExchangeError::Transport => DiagnosticErrorType::StreamFailed,
    }
}

fn encode_message(
    message: &MembershipHistoryMessage,
) -> Result<Vec<u8>, MembershipHistoryExchangeError> {
    let mut payload = vec![WIRE_VERSION];
    payload.extend(
        postcard::to_stdvec(message).map_err(|_| MembershipHistoryExchangeError::Transport)?,
    );
    if payload.len() > MAX_MEMBERSHIP_HISTORY_FRAME_SIZE {
        return Err(MembershipHistoryExchangeError::Transport);
    }
    Ok(payload)
}

fn encode_request(
    message: MembershipHistoryMessage,
) -> Result<Vec<u8>, MembershipHistoryExchangeError> {
    let mut payload = vec![WIRE_VERSION];
    payload.extend_from_slice(REQUEST_LAYOUT_MARKER);
    payload.extend(
        postcard::to_stdvec(&WireMembershipHistoryRequest {
            trace_context: inject_current(),
            message,
        })
        .map_err(|_| MembershipHistoryExchangeError::Transport)?,
    );
    if payload.len() > MAX_MEMBERSHIP_HISTORY_FRAME_SIZE {
        return Err(MembershipHistoryExchangeError::Transport);
    }
    Ok(payload)
}

fn decode_request(
    payload: &[u8],
) -> Result<WireMembershipHistoryRequest, MembershipHistoryExchangeError> {
    let Some((&version, body)) = payload.split_first() else {
        return Err(MembershipHistoryExchangeError::Transport);
    };
    let Some(body) = body.strip_prefix(REQUEST_LAYOUT_MARKER) else {
        return Err(MembershipHistoryExchangeError::Transport);
    };
    if version != WIRE_VERSION
        || body.is_empty()
        || payload.len() > MAX_MEMBERSHIP_HISTORY_FRAME_SIZE
    {
        return Err(MembershipHistoryExchangeError::Transport);
    }
    postcard::from_bytes(body).map_err(|_| MembershipHistoryExchangeError::Transport)
}

fn record_server_completion(elapsed: Duration, error: Option<&DiagnosticErrorType>) {
    let completion = match error {
        Some(error) => OperationCompletion::failed(
            DiagnosticDomain::SpaceMembership,
            DiagnosticOperation::MembershipHistorySync,
            DiagnosticRole::Member,
            *error,
            elapsed,
        ),
        None => OperationCompletion::succeeded(
            DiagnosticDomain::SpaceMembership,
            DiagnosticOperation::MembershipHistorySync,
            DiagnosticRole::Member,
            elapsed,
        ),
    };
    complete_operation(completion);
}

fn decode_message(
    payload: &[u8],
) -> Result<MembershipHistoryMessage, MembershipHistoryExchangeError> {
    let Some((&version, body)) = payload.split_first() else {
        return Err(MembershipHistoryExchangeError::Transport);
    };
    if version != WIRE_VERSION
        || body.is_empty()
        || payload.len() > MAX_MEMBERSHIP_HISTORY_FRAME_SIZE
    {
        return Err(MembershipHistoryExchangeError::Transport);
    }
    let message: MembershipHistoryMessage =
        postcard::from_bytes(body).map_err(|_| MembershipHistoryExchangeError::Transport)?;
    if matches!(
        message,
        MembershipHistoryMessage::SummaryV3(_)
            | MembershipHistoryMessage::RequestSuffixV3(_)
            | MembershipHistoryMessage::AckV3(_)
            | MembershipHistoryMessage::SuffixPageV3(_)
            | MembershipHistoryMessage::RestrictedEventV3(_)
            | MembershipHistoryMessage::RestrictedDecisionV3(_)
            | MembershipHistoryMessage::RequestConflictEvidenceV3(_)
            | MembershipHistoryMessage::ConflictEvidenceV3(_)
    ) {
        Ok(message)
    } else {
        Err(MembershipHistoryExchangeError::Transport)
    }
}

struct HandlerState {
    member_repo: Arc<dyn MemberRepositoryPort>,
    fingerprint_factory: Arc<dyn IdentityFingerprintFactoryPort>,
    endpoint: Arc<dyn MembershipHistoryExchangeEndpointPort>,
}

impl IrohMembershipHistoryExchangeHandler {
    async fn resolve_source_device(
        &self,
        public_key: &[u8; 32],
        message: &MembershipHistoryMessage,
    ) -> Option<DeviceId> {
        let fingerprint = self
            .state
            .fingerprint_factory
            .from_public_key(public_key)
            .ok()?;
        let known = self
            .state
            .member_repo
            .list()
            .await
            .ok()?
            .into_iter()
            .find(|member| member.identity_fingerprint == fingerprint)
            .map(|member| member.device_id);
        if known.is_some() {
            return known;
        }

        introduced_device(message, &fingerprint)
    }
}

fn introduced_device(
    message: &MembershipHistoryMessage,
    fingerprint: &uc_core::security::IdentityFingerprint,
) -> Option<DeviceId> {
    let admission = match message {
        MembershipHistoryMessage::SummaryV3(summary) => &summary.sender_admission,
        MembershipHistoryMessage::SuffixPageV3(page) => page.sender_admission(),
        MembershipHistoryMessage::RequestSuffixV3(_)
        | MembershipHistoryMessage::RequestConflictEvidenceV3(_)
        | MembershipHistoryMessage::ConflictEvidenceV3(_)
        | MembershipHistoryMessage::AckV3(_)
        | MembershipHistoryMessage::RestrictedEventV3(_)
        | MembershipHistoryMessage::RestrictedDecisionV3(_) => return None,
    };
    (&admission.identity_fingerprint == fingerprint).then(|| admission.device_id.clone())
}

async fn write_message(
    send: &mut iroh::endpoint::SendStream,
    payload: &[u8],
) -> Result<(), MembershipHistoryExchangeError> {
    let length =
        u32::try_from(payload.len()).map_err(|_| MembershipHistoryExchangeError::Transport)?;
    send.write_all(&length.to_be_bytes())
        .await
        .map_err(|_| MembershipHistoryExchangeError::Transport)?;
    send.write_all(payload)
        .await
        .map_err(|_| MembershipHistoryExchangeError::Transport)?;
    send.finish()
        .map_err(|_| MembershipHistoryExchangeError::Transport)
}

async fn read_byte(
    receive: &mut iroh::endpoint::RecvStream,
) -> Result<u8, MembershipHistoryExchangeError> {
    let mut value = [0; 1];
    tokio::time::timeout(IO_TIMEOUT, receive.read_exact(&mut value))
        .await
        .map_err(|_| MembershipHistoryExchangeError::Transport)?
        .map_err(|_| MembershipHistoryExchangeError::Transport)?;
    Ok(value[0])
}

async fn read_message(
    receive: &mut iroh::endpoint::RecvStream,
) -> Result<Vec<u8>, MembershipHistoryExchangeError> {
    let mut length = [0; 4];
    tokio::time::timeout(IO_TIMEOUT, receive.read_exact(&mut length))
        .await
        .map_err(|_| MembershipHistoryExchangeError::Transport)?
        .map_err(|_| MembershipHistoryExchangeError::Transport)?;
    let length = checked_message_length(u32::from_be_bytes(length) as usize)?;
    let mut payload = vec![0; length];
    tokio::time::timeout(IO_TIMEOUT, receive.read_exact(&mut payload))
        .await
        .map_err(|_| MembershipHistoryExchangeError::Transport)?
        .map_err(|_| MembershipHistoryExchangeError::Transport)?;
    Ok(payload)
}

fn checked_message_length(length: usize) -> Result<usize, MembershipHistoryExchangeError> {
    if length == 0 || length > MAX_MEMBERSHIP_HISTORY_FRAME_SIZE {
        return Err(MembershipHistoryExchangeError::Transport);
    }
    Ok(length)
}

async fn reject(send: &mut iroh::endpoint::SendStream) {
    let _ = send.write_all(&[REJECTED]).await;
    let _ = send.finish();
}

#[cfg(test)]
mod tests {
    use opentelemetry::trace::TracerProvider as _;
    use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
    use tracing_subscriber::layer::SubscriberExt;
    use uc_core::ids::DeviceId;
    use uc_core::membership::{
        AdmissionChangeFacts, MembershipCredential, MembershipHistoryAckV3,
        MembershipHistoryMessage, ED25519_SIGNATURE_ALGORITHM_V1,
        MAX_MEMBERSHIP_HISTORY_FRAME_SIZE,
    };
    use uc_core::security::IdentityFingerprint;

    use super::{
        checked_message_length, decode_message, decode_request, encode_message, encode_request,
        introduced_device, MEMBERSHIP_HISTORY_EXCHANGE_ALPN,
    };

    #[test]
    fn history_frame_length_accepts_the_boundary_and_rejects_oversize_before_allocation() {
        assert_eq!(checked_message_length(1), Ok(1));
        assert_eq!(
            checked_message_length(MAX_MEMBERSHIP_HISTORY_FRAME_SIZE),
            Ok(MAX_MEMBERSHIP_HISTORY_FRAME_SIZE)
        );
        assert!(checked_message_length(0).is_err());
        assert!(checked_message_length(MAX_MEMBERSHIP_HISTORY_FRAME_SIZE + 1).is_err());
    }

    #[test]
    fn history_v3_wire_checks_version_before_decoding_the_body() {
        assert_eq!(
            MEMBERSHIP_HISTORY_EXCHANGE_ALPN,
            b"uniclipboard/membership-history/3"
        );
        let message = MembershipHistoryMessage::AckV3(MembershipHistoryAckV3::Invalid);
        let encoded = encode_message(&message).unwrap();
        assert_eq!(encoded[0], 3);
        assert_eq!(decode_message(&encoded).unwrap(), message);

        let mut old_version_with_invalid_body = vec![1];
        old_version_with_invalid_body.extend([0xff; 32]);
        assert!(decode_message(&old_version_with_invalid_body).is_err());
    }

    #[test]
    fn history_request_layout_is_explicit_and_keeps_context_private() {
        let message = MembershipHistoryMessage::AckV3(MembershipHistoryAckV3::Invalid);
        let encoded = encode_request(message.clone()).expect("request encodes");

        assert_eq!(&encoded[..5], b"\x03UCT1");
        let decoded = decode_request(&encoded).expect("request decodes");
        assert_eq!(decoded.message, message);
        assert!(decoded.trace_context.is_none());
        assert!(decode_request(&encode_message(&message).expect("response encodes")).is_err());
    }

    #[test]
    fn history_request_creates_a_real_server_parent_after_peer_resolution() {
        let exporter = InMemorySpanExporter::default();
        let provider = SdkTracerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        let subscriber = tracing_subscriber::registry().with(
            tracing_opentelemetry::layer()
                .with_tracer(provider.tracer("membership-history-trace-test"))
                .with_context_activation(true),
        );

        tracing::subscriber::with_default(subscriber, || {
            let client = uc_observability_contract::diagnostics::operation_span(
                uc_observability_contract::diagnostics::OperationContext {
                    domain: uc_observability_contract::diagnostics::DiagnosticDomain::SpaceMembership,
                    operation: uc_observability_contract::diagnostics::DiagnosticOperation::MembershipHistorySync,
                    role: uc_observability_contract::diagnostics::DiagnosticRole::Member,
                    kind: uc_observability_contract::diagnostics::DiagnosticSpanKind::Client,
                    flow: None,
                },
            );
            let _client_entered = client.enter();
            let encoded = encode_request(MembershipHistoryMessage::AckV3(
                MembershipHistoryAckV3::Invalid,
            ))
            .expect("request encodes");
            let request = decode_request(&encoded).expect("request decodes");
            let server = uc_observability_contract::diagnostics::operation_span(
                uc_observability_contract::diagnostics::OperationContext {
                    domain: uc_observability_contract::diagnostics::DiagnosticDomain::SpaceMembership,
                    operation: uc_observability_contract::diagnostics::DiagnosticOperation::MembershipHistorySync,
                    role: uc_observability_contract::diagnostics::DiagnosticRole::Member,
                    kind: uc_observability_contract::diagnostics::DiagnosticSpanKind::Server,
                    flow: None,
                },
            );
            assert!(super::set_remote_parent(
                &server,
                request.trace_context.as_ref()
            ));
            let _server_entered = server.enter();
        });
        provider.force_flush().expect("trace flush");

        let spans = exporter.get_finished_spans().expect("finished spans");
        assert_eq!(spans.len(), 2);
        let client = spans
            .iter()
            .find(|span| span.parent_span_id == opentelemetry::trace::SpanId::INVALID)
            .expect("client span");
        let server = spans
            .iter()
            .find(|span| span.parent_span_id == client.span_context.span_id())
            .expect("server span");
        assert_eq!(
            server.span_context.trace_id(),
            client.span_context.trace_id()
        );
    }

    #[test]
    fn history_v3_summary_is_accepted_by_the_decode_allowlist() {
        let message =
            MembershipHistoryMessage::SummaryV3(uc_core::membership::MembershipHistorySummaryV3 {
                lineage_id: "space-a".to_owned(),
                current_position: uc_core::membership::BaseMembershipHistoryPosition {
                    event_id: None,
                    depth: 0,
                    history_digest: [7; 32],
                },
                transfer_id: [8; 32],
                sender_admission: admission_facts("device-a", fingerprint()),
            });

        let encoded = encode_message(&message).unwrap();

        assert_eq!(decode_message(&encoded).unwrap(), message);
    }

    fn fingerprint() -> IdentityFingerprint {
        IdentityFingerprint::from_display_string("ABCD-EFGH-IJKL-MNOP")
            .unwrap_or_else(|_| panic!("test fingerprint must be valid"))
    }

    fn admission_facts(
        device: &str,
        identity_fingerprint: IdentityFingerprint,
    ) -> AdmissionChangeFacts {
        let device_id = DeviceId::new(device);
        let credential = MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0x41; 32]);
        AdmissionChangeFacts {
            member_instance: credential.member_instance_id(&device_id),
            device_id,
            device_name: device.to_owned(),
            identity_fingerprint,
            transport_public_key: vec![1],
            transport_address_blob: vec![2],
            identity_signature: vec![3],
        }
    }

    #[test]
    fn unknown_member_cannot_introduce_itself_with_a_regular_history_message() {
        let message = MembershipHistoryMessage::AckV3(MembershipHistoryAckV3::Invalid);

        assert_eq!(introduced_device(&message, &fingerprint()), None);
    }

    #[test]
    fn unknown_member_is_identified_from_a_fingerprint_bound_summary() {
        let facts = admission_facts("device-c", fingerprint());
        let expected = facts.device_id.clone();
        let message =
            MembershipHistoryMessage::SummaryV3(uc_core::membership::MembershipHistorySummaryV3 {
                lineage_id: "space-a".to_owned(),
                current_position: uc_core::membership::BaseMembershipHistoryPosition {
                    event_id: None,
                    depth: 2,
                    history_digest: [7; 32],
                },
                transfer_id: [7; 32],
                sender_admission: facts,
            });

        assert_eq!(introduced_device(&message, &fingerprint()), Some(expected));
    }

    #[test]
    fn unknown_member_claim_is_rejected_when_connection_fingerprint_differs() {
        let facts = admission_facts("device-c", fingerprint());
        let message =
            MembershipHistoryMessage::SummaryV3(uc_core::membership::MembershipHistorySummaryV3 {
                lineage_id: "space-a".to_owned(),
                current_position: uc_core::membership::BaseMembershipHistoryPosition {
                    event_id: None,
                    depth: 2,
                    history_digest: [7; 32],
                },
                transfer_id: [7; 32],
                sender_admission: facts,
            });
        let other = IdentityFingerprint::from_display_string("QRST-UVWX-YZAB-CDEF")
            .unwrap_or_else(|_| panic!("test fingerprint must be valid"));

        assert_eq!(introduced_device(&message, &other), None);
    }
}
