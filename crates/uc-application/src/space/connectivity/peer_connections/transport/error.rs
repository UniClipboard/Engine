use super::PeerReachabilityWireError;
use crate::connectivity::ConnectivityError;
use uc_core::error_class::ErrorClass;
use uc_core::ids::DeviceId;
use uc_core::membership::PeerAdmissionError;
use uc_core::ports::PeerReachabilityError;

#[derive(Debug, thiserror::Error)]
pub enum PeerReachabilityTransportError {
    #[error("presence capability failed")]
    Connection(#[source] ConnectivityError),
    #[error("presence authority read failed")]
    Authority(#[source] PeerAdmissionError),
    #[error("presence authority deadline elapsed")]
    AuthorityTimeout(#[source] tokio::time::error::Elapsed),
    #[error("presence wire exchange failed")]
    Wire(#[source] PeerReachabilityWireError),
    #[error("presence task failed")]
    Task(#[source] std::sync::Arc<tokio::task::JoinError>),
    #[error("presence result interrupted")]
    Response(#[source] tokio::sync::oneshot::error::RecvError),
    #[error("presence attempt stopped")]
    Stopped,
}
impl From<ConnectivityError> for PeerReachabilityTransportError {
    fn from(source: ConnectivityError) -> Self {
        Self::Connection(source)
    }
}
impl From<PeerReachabilityWireError> for PeerReachabilityTransportError {
    fn from(source: PeerReachabilityWireError) -> Self {
        Self::Wire(source)
    }
}

impl PeerReachabilityTransportError {
    pub(super) fn into_contract(self, peer: DeviceId) -> PeerReachabilityError {
        match &self {
            Self::Connection(source) if source.is_address_missing() => {
                PeerReachabilityError::NoAddress(peer)
            }
            _ => PeerReachabilityError::internal(self),
        }
    }
}

impl ErrorClass for PeerReachabilityTransportError {
    fn class(&self) -> &'static str {
        match self {
            Self::Connection(source) => source.class(),
            Self::Authority(_) => "presence_authority_read_failed",
            Self::AuthorityTimeout(_) => "presence_authority_timeout",
            Self::Wire(source) => source.class(),
            Self::Task(_) => "presence_task_failed",
            Self::Response(_) => "presence_result_interrupted",
            Self::Stopped => "presence_stopped",
        }
    }
}
