//! 设备级发现：候选只提供可达性线索，不建立会话或授予设备资格。

mod candidate;
mod directory;
mod error;
mod manager;
mod provider;

pub use candidate::{
    BackendId, ConnectionCandidate, DiscoveredPeer, DiscoveryEvent, DiscoveryPeerId,
    DiscoveryProviderId,
};
pub use directory::PeerDirectory;
pub use error::DiscoveryError;
pub use manager::DiscoveryManager;
pub use provider::{DiscoveryProvider, DiscoveryStream};

#[cfg(test)]
mod tests;
