use iroh::endpoint::{Connection, RecvStream, SendStream};
use iroh::{Endpoint, EndpointAddr};
use tracing::instrument::WithSubscriber;
use uc_observability_contract::diagnostics::connectivity::{
    ConnectionFailurePhase, ConnectionFailureReason, ConnectionObservation, ConnectionOutcome,
    ConnectionPurpose, DialFailure,
};

use super::super::space_admission_wire::IO_DEADLINE;
use super::SPACE_ADMISSION_ALPN;

#[derive(Debug, thiserror::Error)]
pub(super) enum AdmissionConnectError {
    #[error("admission connection timed out")]
    TimedOut(#[source] tokio::time::error::Elapsed),
    #[error("admission connection failed")]
    Transport(#[source] iroh::endpoint::ConnectError),
}

impl AdmissionConnectError {
    pub(super) fn category(&self) -> DialFailure {
        match self {
            Self::TimedOut(_) => DialFailure::TimedOut,
            Self::Transport(_) => DialFailure::TransportFailed,
        }
    }
}

pub(super) async fn connect(
    endpoint: &Endpoint,
    addr: EndpointAddr,
) -> Result<Connection, AdmissionConnectError> {
    let observation =
        ConnectionObservation::begin(ConnectionPurpose::Admission, *addr.id.as_bytes());
    let (summary, fingerprint) = super::super::connection_diagnostics::candidate_summary(&addr);
    observation.input_candidates(
        fingerprint,
        uc_observability_contract::diagnostics::connectivity::AddressInputSource::AdmissionRoute,
        summary,
    );
    let attempt = observation.attempts().begin(1, IO_DEADLINE);
    // 底层连接驱动不能延长业务 span，完整调用由外层负责人结算。
    let connection = endpoint
        .connect(addr, SPACE_ADMISSION_ALPN)
        .with_subscriber(tracing::Dispatch::new(
            tracing::subscriber::NoSubscriber::default(),
        ));
    let result = tokio::time::timeout(IO_DEADLINE, connection)
        .await
        .map_err(AdmissionConnectError::TimedOut)
        .and_then(|result| result.map_err(AdmissionConnectError::Transport));
    let outcome = match &result {
        Ok(_) => ConnectionOutcome::Connected,
        Err(AdmissionConnectError::TimedOut(_)) => ConnectionOutcome::Failed {
            phase: ConnectionFailurePhase::Establish,
            reason: ConnectionFailureReason::TimedOut,
        },
        Err(AdmissionConnectError::Transport(error)) => {
            super::super::connection_diagnostics::connect_failure(error)
        }
    };
    if let Ok(connection) = &result {
        attempt.connected(connection.stable_id() as u64);
        observation.connected(connection.stable_id() as u64);
    } else {
        attempt.finish(outcome);
        observation.finish(outcome);
    }
    result
}

pub(super) async fn open_stream(
    connection: &Connection,
) -> Result<(SendStream, RecvStream), uc_application::deps::SpaceAdmissionTransportError> {
    tokio::time::timeout(IO_DEADLINE, connection.open_bi())
        .await
        .map_err(|_| uc_application::deps::SpaceAdmissionTransportError::Deferred)?
        .map_err(|_| uc_application::deps::SpaceAdmissionTransportError::Deferred)
}
