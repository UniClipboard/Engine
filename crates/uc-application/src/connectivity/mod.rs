//! 会话内完整连接获取与独立交换的负责人。
mod acquisition;
mod admission;
mod backend;
mod connection;
mod error;
mod manager;
mod stream;

pub use admission::{
    AdmissionAttempt, AdmissionGrant, InboundConnectionError, SessionConnectionAdmissionPort,
};
pub use backend::{ConnectionMode, ConnectionProtocol, ConnectivityBackend, PeerConnection};
pub use connection::ConnectionLease;
pub use error::ConnectivityError;
pub use manager::{ConnectivityManager, ConnectivityPolicy};
pub use stream::{PeerReceiveStream, PeerSendStream, StreamLease, StreamPair};
#[cfg(test)]
mod tests;
