use iroh::endpoint::{Connection, RecvStream, SendStream};
use iroh::{Endpoint, EndpointAddr};
use tracing::instrument::WithSubscriber;
use uc_observability_contract::diagnostics::connectivity::{
    ConnectionFailurePhase, ConnectionFailureReason, ConnectionObservation, ConnectionOutcome,
    ConnectionPurpose, DialFailure,
};

use super::super::space_admission_wire::IO_DEADLINE;
use super::diagnostics::record_network_snapshot;
use super::SPACE_ADMISSION_ALPN;
use uc_observability_contract::diagnostics::connectivity::{
    AdmissionExchangeSide, AdmissionNetworkPoint,
};

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

/// 上一次成功交换后仍然打开的准入连接；下一轮交换优先复用，失效或对端已关闭时由调用方重新建立。
#[derive(Clone, Default)]
pub(super) struct ReusableConnection(std::sync::Arc<std::sync::Mutex<Option<Connection>>>);

impl ReusableConnection {
    pub(super) fn store(&self, connection: Connection) {
        *self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(connection);
    }

    /// 取走指向同一对端且仍未关闭的连接；其余情况返回 `None`，旧连接随之丢弃。
    pub(super) fn take_open(&self, remote: iroh::EndpointId) -> Option<Connection> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
            .filter(|connection| {
                connection.remote_id() == remote && connection.close_reason().is_none()
            })
    }
}

pub(super) async fn open_stream(
    connection: &Connection,
) -> Result<(SendStream, RecvStream), uc_application::deps::SpaceAdmissionTransportError> {
    tokio::time::timeout(IO_DEADLINE, connection.open_bi())
        .await
        // discarded-source[timeout]: `Elapsed`: the timeout itself is the classification
        .map_err(|_| uc_application::deps::SpaceAdmissionTransportError::deferred())?
        .map_err(uc_application::deps::SpaceAdmissionTransportError::deferred_from)
}
