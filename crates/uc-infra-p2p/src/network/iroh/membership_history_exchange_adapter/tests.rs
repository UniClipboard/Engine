#[tokio::test]
async fn invalid_reply_keeps_decode_reason_on_the_completed_span() {
    use tracing::Instrument;
    use uc_observability_contract::diagnostics::*;
    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let subscriber = tracing_subscriber::registry().with(
        tracing_opentelemetry::layer().with_tracer(provider.tracer("membership-failure-test")),
    );
    let _subscriber = tracing::subscriber::set_default(subscriber);
    scope_operation_diagnostics(async {
        let span = operation_span(OperationContext {
            domain: DiagnosticDomain::SpaceMembership,
            operation: DiagnosticOperation::MembershipHistorySync,
            role: DiagnosticRole::Member,
            kind: DiagnosticSpanKind::Client,
        });
        async {
            describe_membership_exchange(MembershipExchangePurpose::CompareSummary, false);
            assert!(decode_message(&[3, 255]).is_err());
            complete_operation(OperationCompletion::failed(
                DiagnosticDomain::SpaceMembership,
                DiagnosticOperation::MembershipHistorySync,
                DiagnosticRole::Member,
                DiagnosticErrorType::StreamFailed,
                std::time::Duration::ZERO,
            ));
        }
        .instrument(span)
        .await;
    })
    .await;
    provider.force_flush().expect("flush");
    let spans = exporter.get_finished_spans().expect("spans");
    assert_eq!(spans.len(), 1);
    for (key, value) in [
        ("error.type", "decode_failed"),
        ("uc.outcome", "error"),
        ("uc.display.name", "membership.compare_summary.exchange"),
    ] {
        assert!(
            spans[0]
                .attributes
                .iter()
                .any(|field| field.key.as_str() == key && field.value.as_str() == value),
            "missing {key}"
        );
    }
}

use super::super::inbound_peer::PeerIdentityResolver;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
use std::sync::Arc;
use tracing_subscriber::layer::SubscriberExt;
use uc_core::ids::DeviceId;
use uc_core::membership::{
    AdmissionChangeFacts, MembershipCredential, MembershipHistoryAckV3, MembershipHistoryMessage,
    ED25519_SIGNATURE_ALGORITHM_V1, MAX_MEMBERSHIP_HISTORY_FRAME_SIZE,
};
use uc_core::ports::security::IdentityFingerprintFactoryPort;
use uc_core::security::IdentityFingerprint;

use super::super::trace_context::{inject_current, set_remote_parent};
use super::admission::introduced_device;
use uc_observability_contract::diagnostics::{describe_operation_failure, DiagnosticErrorType};
use uc_sync_protocol::membership_history::{
    checked_message_length, decode_message as decode_wire_message, decode_request, encode_message,
    encode_request as encode_wire_request, MembershipHistoryWireError,
    MEMBERSHIP_HISTORY_EXCHANGE_ALPN,
};
fn encode_request(
    message: MembershipHistoryMessage,
) -> Result<Vec<u8>, MembershipHistoryWireError> {
    encode_wire_request(message, inject_current())
}
fn decode_message(bytes: &[u8]) -> Result<MembershipHistoryMessage, MembershipHistoryWireError> {
    decode_wire_message(bytes)
        .inspect_err(|_| describe_operation_failure(DiagnosticErrorType::DecodeFailed))
}

#[test]
fn history_frame_length_accepts_the_boundary_and_rejects_oversize_before_allocation() {
    assert!(matches!(checked_message_length(1), Ok(1)));
    assert!(matches!(
        checked_message_length(MAX_MEMBERSHIP_HISTORY_FRAME_SIZE),
        Ok(MAX_MEMBERSHIP_HISTORY_FRAME_SIZE)
    ));
    assert!(checked_message_length(0).is_err());
    assert!(checked_message_length(MAX_MEMBERSHIP_HISTORY_FRAME_SIZE + 1).is_err());
}

#[test]
fn history_v4_wire_checks_version_before_decoding_the_body() {
    assert_eq!(
        MEMBERSHIP_HISTORY_EXCHANGE_ALPN,
        b"uniclipboard/membership-history/4"
    );
    let message = MembershipHistoryMessage::AckV3(MembershipHistoryAckV3::Invalid);
    let encoded = encode_message(&message).unwrap();
    assert_eq!(encoded[0], 4);
    assert_eq!(decode_message(&encoded).unwrap(), message);

    let mut old_version_with_invalid_body = vec![1];
    old_version_with_invalid_body.extend([0xff; 32]);
    assert!(decode_message(&old_version_with_invalid_body).is_err());
}

#[test]
fn history_request_layout_is_explicit_and_keeps_context_private() {
    let message = MembershipHistoryMessage::AckV3(MembershipHistoryAckV3::Invalid);
    let encoded = encode_request(message.clone()).expect("request encodes");

    assert_eq!(&encoded[..5], b"\x04UCT1");
    let decoded = decode_request(&encoded).expect("request decodes");
    assert_eq!(decoded.message, message);
    assert!(decoded.trace_context.is_none());
    let mut old_version = encoded.clone();
    old_version[0] = 3;
    assert!(decode_request(&old_version).is_err());
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
                },
            );
        assert!(set_remote_parent(&server, request.trace_context.as_ref()));
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

struct FixedFingerprint(IdentityFingerprint);

impl IdentityFingerprintFactoryPort for FixedFingerprint {
    fn from_public_key(&self, _: &[u8]) -> anyhow::Result<IdentityFingerprint> {
        Ok(self.0.clone())
    }
}

struct UnusedMembers;

crate::network::iroh::inbound_peer::member_table_identity_directory!(UnusedMembers);
#[async_trait::async_trait]
impl uc_core::membership::MemberRepositoryPort for UnusedMembers {
    async fn get(
        &self,
        _: &DeviceId,
    ) -> Result<Option<uc_core::membership::SpaceMember>, uc_core::membership::MembershipError>
    {
        panic!("unused")
    }
    async fn list(
        &self,
    ) -> Result<Vec<uc_core::membership::SpaceMember>, uc_core::membership::MembershipError> {
        panic!("unused")
    }
    async fn save(
        &self,
        _: &uc_core::membership::SpaceMember,
    ) -> Result<(), uc_core::membership::MembershipError> {
        panic!("unused")
    }
    async fn remove(&self, _: &DeviceId) -> Result<bool, uc_core::membership::MembershipError> {
        panic!("unused")
    }
}

fn resolver(fingerprint: IdentityFingerprint) -> PeerIdentityResolver {
    PeerIdentityResolver::new(
        Arc::new(UnusedMembers),
        Arc::new(FixedFingerprint(fingerprint)),
    )
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

    assert_eq!(
        introduced_device(&message, &resolver(fingerprint()), &[0; 32]),
        None
    );
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

    assert_eq!(
        introduced_device(&message, &resolver(fingerprint()), &[0; 32]),
        Some(expected)
    );
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

    assert_eq!(
        introduced_device(&message, &resolver(other), &[0; 32]),
        None
    );
}
