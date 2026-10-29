use super::super::connectivity::IrohPeerConnection;
use super::managed::{AdmissionEndpoint, AdmissionManager};
use iroh::endpoint::Connection;
use iroh::{Endpoint, EndpointAddr};
use std::error::Error;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use uc_application::deps::{ConnectionLease, ConnectivityError};
use uc_observability_contract::diagnostics::connectivity::{
    ConnectionFailurePhase, ConnectionFailureReason, ConnectionObservation, ConnectionOutcome,
    ConnectionPurpose, DialFailure,
};

use super::diagnostics::record_network_snapshot;
use super::SPACE_ADMISSION_ALPN;
use uc_observability_contract::diagnostics::connectivity::{
    AdmissionExchangeSide, AdmissionNetworkPoint,
};
use uc_observability_contract::diagnostics::within_network_scope;
use uc_sync_protocol::space_admission::IO_DEADLINE;

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
    let connection = within_network_scope(endpoint.connect(addr, SPACE_ADMISSION_ALPN));
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
        record_network_snapshot(
            connection,
            AdmissionExchangeSide::Joiner,
            AdmissionNetworkPoint::Connected,
        );
        attempt.connected(connection.stable_id() as u64);
        observation.connected(connection.stable_id() as u64);
    } else {
        attempt.finish(outcome);
        observation.finish(outcome);
    }
    result
}

pub(super) async fn acquire(
    manager: &AdmissionManager,
    addr: EndpointAddr,
) -> Result<(Connection, ConnectionLease<AdmissionEndpoint>), ConnectivityError> {
    let target = AdmissionEndpoint(*addr.id.as_bytes());
    let address = postcard::to_stdvec(&addr)
        .map_err(|source| ConnectivityError::AddressLookup(Box::new(source)))?;
    let request = manager.request(target).with_address_hint(address);
    let lease = request
        .connect(Instant::now() + IO_DEADLINE, &CancellationToken::new())
        .await?;
    let native = IrohPeerConnection::native(lease.transport())?;
    Ok((native, lease))
}

pub(super) fn dial_failure(error: &ConnectivityError) -> DialFailure {
    let mut source: Option<&(dyn Error + 'static)> = Some(error);
    while let Some(current) = source {
        if let Some(error) = current.downcast_ref::<AdmissionConnectError>() {
            return error.category();
        }
        if matches!(
            current.downcast_ref::<ConnectivityError>(),
            Some(ConnectivityError::TimedOut(_) | ConnectivityError::DeadlineExpired)
        ) {
            return DialFailure::TimedOut;
        }
        source = current.source();
    }
    DialFailure::TransportFailed
}
