use iroh::endpoint::{Connection, ConnectionError, ReadError, WriteError};
use std::io::{Error as IoError, ErrorKind};
use std::time::Duration;

use uc_application::deps::SpaceAdmissionTransportError;
use uc_observability_contract::diagnostics::connectivity::{
    record_admission_network_snapshot, AdmissionExchangeFailure, AdmissionExchangeSide,
    AdmissionNetworkPoint, AdmissionNetworkSnapshot, AuthenticationFailure, AuthenticationStage,
    CredentialFailure, IdentityCheck, NetworkPathKind, ProofFailure, ReadFailure,
};
use uc_observability_contract::diagnostics::{
    complete_operation, operation_span, DiagnosticDomain, DiagnosticErrorType, DiagnosticOperation,
    DiagnosticRole, DiagnosticSpanKind, OperationCompletion, OperationContext,
};

use super::super::space_admission_wire::WireError;
use super::errors::HandlerError;

pub(super) fn record_client_completion(
    operation: DiagnosticOperation,
    elapsed: Duration,
    error: Option<&SpaceAdmissionTransportError>,
) {
    complete_operation(client_completion(operation, elapsed, error));
}

pub(super) fn client_completion(
    operation: DiagnosticOperation,
    elapsed: Duration,
    error: Option<&SpaceAdmissionTransportError>,
) -> OperationCompletion {
    let completion = match error {
        None => OperationCompletion::succeeded(
            DiagnosticDomain::SpaceAdmission,
            operation,
            DiagnosticRole::Joiner,
            elapsed,
        ),
        Some(SpaceAdmissionTransportError::AuthenticationRejected) => OperationCompletion::failed(
            DiagnosticDomain::SpaceAdmission,
            operation,
            DiagnosticRole::Joiner,
            DiagnosticErrorType::AuthenticationFailed,
            elapsed,
        ),
        Some(SpaceAdmissionTransportError::PeerUpgradeRequired) => OperationCompletion::failed(
            DiagnosticDomain::SpaceAdmission,
            operation,
            DiagnosticRole::Joiner,
            DiagnosticErrorType::PeerIncompatible,
            elapsed,
        ),
        Some(SpaceAdmissionTransportError::ProtocolRejected) => OperationCompletion::failed(
            DiagnosticDomain::SpaceAdmission,
            operation,
            DiagnosticRole::Joiner,
            DiagnosticErrorType::DecodeFailed,
            elapsed,
        ),
        Some(SpaceAdmissionTransportError::Deferred) => OperationCompletion::deferred(
            DiagnosticDomain::SpaceAdmission,
            operation,
            DiagnosticRole::Joiner,
            elapsed,
        ),
        Some(
            SpaceAdmissionTransportError::InvitationUnavailable
            | SpaceAdmissionTransportError::Unavailable,
        ) => OperationCompletion::failed(
            DiagnosticDomain::SpaceAdmission,
            operation,
            DiagnosticRole::Joiner,
            DiagnosticErrorType::Unavailable,
            elapsed,
        ),
    };
    completion
}

pub(super) fn server_completion(
    elapsed: Duration,
    error: Option<&HandlerError>,
) -> OperationCompletion {
    let completion = match error {
        None => OperationCompletion::succeeded(
            DiagnosticDomain::SpaceAdmission,
            DiagnosticOperation::NetworkTransport,
            DiagnosticRole::Sponsor,
            elapsed,
        ),
        Some(error) => OperationCompletion::failed(
            DiagnosticDomain::SpaceAdmission,
            DiagnosticOperation::NetworkTransport,
            DiagnosticRole::Sponsor,
            server_error_type(error),
            elapsed,
        ),
    };
    completion
}

pub(super) fn wire_failure(error: &WireError) -> AdmissionExchangeFailure {
    match error {
        WireError::Timeout => AdmissionExchangeFailure::TimedOut,
        WireError::Io(source) => io_failure(source),
        WireError::UnsupportedLayout => AdmissionExchangeFailure::PeerUpgradeRequired,
        _ => AdmissionExchangeFailure::InvalidMessage,
    }
}

pub(super) fn io_failure(error: &IoError) -> AdmissionExchangeFailure {
    // QUIC 的超时经过 std::io 转换会变成 NotConnected，先读取原始固定分类。
    if let Some(source) = error.get_ref() {
        if matches!(
            source.downcast_ref::<ReadError>(),
            Some(ReadError::ConnectionLost(ConnectionError::TimedOut))
        ) || matches!(
            source.downcast_ref::<WriteError>(),
            Some(WriteError::ConnectionLost(ConnectionError::TimedOut))
        ) {
            return AdmissionExchangeFailure::TimedOut;
        }
    }
    match error.kind() {
        ErrorKind::UnexpectedEof
        | ErrorKind::ConnectionAborted
        | ErrorKind::ConnectionReset
        | ErrorKind::BrokenPipe
        | ErrorKind::NotConnected => AdmissionExchangeFailure::ConnectionClosed,
        ErrorKind::TimedOut => AdmissionExchangeFailure::TimedOut,
        _ => AdmissionExchangeFailure::IoFailed,
    }
}

pub(super) fn record_network_snapshot(
    connection: &Connection,
    side: AdmissionExchangeSide,
    point: AdmissionNetworkPoint,
) {
    let paths = connection.paths();
    let selected = paths.iter().find(|path| path.is_selected());
    let path = selected.as_ref().map_or(NetworkPathKind::Unknown, |path| {
        if path.is_ip() {
            NetworkPathKind::Direct
        } else if path.is_relay() {
            NetworkPathKind::Relay
        } else {
            NetworkPathKind::Other
        }
    });
    let stats = connection.stats();
    record_admission_network_snapshot(
        side,
        point,
        AdmissionNetworkSnapshot {
            path,
            rtt_us: selected
                .and_then(|path| connection.rtt(path.id()))
                .map(|rtt| u64::try_from(rtt.as_micros()).unwrap_or(u64::MAX)),
            lost_packets_total: stats.lost_packets,
            sent_datagrams_total: stats.udp_tx.datagrams,
            received_datagrams_total: stats.udp_rx.datagrams,
        },
    );
}

pub(super) fn handler_failure(error: &HandlerError) -> AdmissionExchangeFailure {
    match error {
        HandlerError::Timeout => AdmissionExchangeFailure::TimedOut,
        HandlerError::Protocol => AdmissionExchangeFailure::InvalidMessage,
        HandlerError::Acknowledgement => AdmissionExchangeFailure::ConnectionClosed,
        HandlerError::Transport { .. } => AdmissionExchangeFailure::IoFailed,
        HandlerError::PeerUpgradeRequired => AdmissionExchangeFailure::PeerUpgradeRequired,
        HandlerError::Application => AdmissionExchangeFailure::Internal,
        _ => AdmissionExchangeFailure::AuthenticationRejected,
    }
}

pub(super) fn server_operation_span() -> tracing::Span {
    operation_span(OperationContext {
        domain: DiagnosticDomain::SpaceAdmission,
        operation: DiagnosticOperation::NetworkTransport,
        role: DiagnosticRole::Sponsor,
        kind: DiagnosticSpanKind::Server,
    })
}

#[derive(Clone, Copy)]
pub(super) enum AuthenticationStep {
    ReceiveHello,
    InitialVersion,
    InitialIdentity,
    InitialCredential,
    InitialProof,
    ContinuationIdentity,
    ContinuationCredential,
    ContinuationProof,
    ReceiveRequest,
    RequestIdentity,
    RequestProof,
}

impl AuthenticationStep {
    pub(super) fn failure(self, error: &HandlerError) -> AuthenticationFailure {
        let credential = match error {
            HandlerError::Credential(source) => source.diagnostic_failure(),
            _ => CredentialFailure::Unavailable,
        };
        let read = match error {
            HandlerError::Transport { .. } => ReadFailure::TransportFailed,
            HandlerError::Timeout => ReadFailure::TimedOut,
            HandlerError::PeerUpgradeRequired => ReadFailure::PeerUpgradeRequired,
            _ => ReadFailure::InvalidMessage,
        };
        let proof = if matches!(
            error,
            HandlerError::Authentication | HandlerError::AuthenticationProof { .. }
        ) {
            ProofFailure::Rejected
        } else {
            ProofFailure::Exchange(read)
        };
        match self {
            Self::ReceiveHello => AuthenticationFailure::ReadHello(read),
            Self::ReceiveRequest => AuthenticationFailure::ReadRequest(read),
            Self::InitialVersion => {
                AuthenticationFailure::IdentityMismatch(IdentityCheck::InitialVersion)
            }
            Self::InitialIdentity => {
                AuthenticationFailure::IdentityMismatch(IdentityCheck::InitialPeer)
            }
            Self::ContinuationIdentity => {
                AuthenticationFailure::IdentityMismatch(IdentityCheck::ContinuationPeer)
            }
            Self::RequestIdentity => {
                AuthenticationFailure::IdentityMismatch(IdentityCheck::RequestBinding)
            }
            Self::InitialCredential if matches!(error, HandlerError::Timeout) => {
                AuthenticationFailure::DeadlineExceeded(AuthenticationStage::InitialCredential)
            }
            Self::ContinuationCredential if matches!(error, HandlerError::Timeout) => {
                AuthenticationFailure::DeadlineExceeded(AuthenticationStage::ContinuationCredential)
            }
            Self::InitialCredential => AuthenticationFailure::InitialCredential(credential),
            Self::ContinuationCredential => {
                AuthenticationFailure::ContinuationCredential(credential)
            }
            Self::InitialProof => AuthenticationFailure::InitialProof(proof),
            Self::ContinuationProof => AuthenticationFailure::ContinuationProof(proof),
            Self::RequestProof => AuthenticationFailure::RequestProof(proof),
        }
    }
}

pub(super) fn server_error_type(error: &HandlerError) -> DiagnosticErrorType {
    match error {
        HandlerError::Authentication
        | HandlerError::Credential(_)
        | HandlerError::AuthenticationProof { .. } => DiagnosticErrorType::AuthenticationFailed,
        HandlerError::Protocol => DiagnosticErrorType::DecodeFailed,
        HandlerError::Transport { .. } => DiagnosticErrorType::StreamFailed,
        HandlerError::PeerUpgradeRequired => DiagnosticErrorType::PeerIncompatible,
        HandlerError::Application => DiagnosticErrorType::Internal,
        HandlerError::Acknowledgement => DiagnosticErrorType::ChannelClosed,
        HandlerError::Timeout => DiagnosticErrorType::Timeout,
    }
}
