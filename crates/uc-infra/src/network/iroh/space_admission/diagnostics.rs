use std::time::Duration;

use uc_application::deps::SpaceAdmissionTransportError;
use uc_observability_contract::diagnostics::connectivity::{
    AuthenticationFailure, AuthenticationStage, CredentialFailure, IdentityCheck, ProofFailure,
    ReadFailure,
};
use uc_observability_contract::diagnostics::{
    complete_operation, operation_span, DiagnosticDomain, DiagnosticErrorType, DiagnosticOperation,
    DiagnosticRole, DiagnosticSpanKind, OperationCompletion, OperationContext,
};

use super::errors::HandlerError;

pub(super) fn record_client_completion(
    operation: DiagnosticOperation,
    elapsed: Duration,
    error: Option<&SpaceAdmissionTransportError>,
) {
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
    complete_operation(completion);
}

pub(super) fn record_server_completion(elapsed: Duration, error: Option<&HandlerError>) {
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
    complete_operation(completion);
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
