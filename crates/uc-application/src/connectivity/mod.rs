//! 会话内完整连接获取与独立交换的负责人。
mod acquisition;
mod admission;
mod backend;
mod checks;
mod connection;
mod elapsed_clock;
mod error;
mod identity;
mod inbound;
mod lifetime;
mod manager;
mod pending;
mod request;
mod retention;
mod stream;
mod unidirectional;

pub use admission::{
    AdmissionAttempt, AdmissionGrant, InboundAdmissionFence, InboundConnectionError,
    InboundIdentityReservation, SessionConnectionAdmissionPort,
};
pub use backend::{
    ConnectionMode, ConnectionProtocol, ConnectivityBackend, PeerConnection,
    PeerUnidirectionalConnection,
};
pub use connection::ConnectionLease;
pub use elapsed_clock::{ElapsedClockError, ElapsedClockPort, ElapsedClockRefusal};
pub use error::{ConnectivityError, StreamCapabilityRefusal};
pub use identity::ConnectionKey;
pub use inbound::InboundAdmissionPolicy;
pub use manager::{ConnectivityManager, ConnectivityPolicy};
pub use pending::PendingInboundLease;
pub use request::ConnectionRequest;
pub use retention::IdleRetentionPolicy;
pub use stream::{PeerReceiveStream, PeerSendStream, StreamLease, StreamPair, StreamWorkLease};
pub use unidirectional::UniStreamLease;
#[cfg(test)]
mod tests;

pub(crate) use request::ConnectionRequestScope;
