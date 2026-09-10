use iroh::endpoint::{Connection, RecvStream, SendStream};
use iroh::{Endpoint, EndpointAddr};
use tracing::instrument::WithSubscriber;
use uc_observability_contract::diagnostics::connectivity::DialFailure;

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
    // 底层连接驱动不能延长业务 span，完整调用由外层负责人结算。
    let connection = endpoint
        .connect(addr, SPACE_ADMISSION_ALPN)
        .with_subscriber(tracing::Dispatch::new(
            tracing::subscriber::NoSubscriber::default(),
        ));
    tokio::time::timeout(IO_DEADLINE, connection)
        .await
        .map_err(AdmissionConnectError::TimedOut)?
        .map_err(AdmissionConnectError::Transport)
}

pub(super) async fn open_stream(
    connection: &Connection,
) -> Result<(SendStream, RecvStream), uc_application::deps::SpaceAdmissionTransportError> {
    tokio::time::timeout(IO_DEADLINE, connection.open_bi())
        .await
        .map_err(|_| uc_application::deps::SpaceAdmissionTransportError::Deferred)?
        .map_err(|_| uc_application::deps::SpaceAdmissionTransportError::Deferred)
}
