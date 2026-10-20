//! presence 完整准入、在线证据与保留连接的唯一负责人。
mod ports;
pub use ports::{
    PeerReachabilityEnvironmentPort, PeerReachabilityTransportPorts, PeerReachabilityWireError,
    PeerReachabilityWirePort,
};

mod admission;
mod error;
mod liveness;
mod model;
mod owner;
pub use error::PeerReachabilityTransportError;
pub use owner::PeerReachabilityTransport;

#[cfg(test)]
mod tests;
