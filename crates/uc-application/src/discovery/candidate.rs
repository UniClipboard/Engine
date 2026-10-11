use std::{fmt, time::Duration};

use tokio::time::Instant;

/// 实际理解 locator 的后端；与发现来源相互独立。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct BackendId(pub &'static str);

/// 提供观察的来源，一个来源只替换自己的观察。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct DiscoveryProviderId(pub &'static str);

/// 未经授权的不透明传输身份。可以属于陌生设备，不能当作逻辑 DeviceId。
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct DiscoveryPeerId {
    pub backend: BackendId,
    identity: Vec<u8>,
}

impl DiscoveryPeerId {
    pub fn new(backend: BackendId, identity: Vec<u8>) -> Self {
        Self { backend, identity }
    }

    pub fn identity(&self) -> &[u8] {
        &self.identity
    }
}

impl fmt::Debug for DiscoveryPeerId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DiscoveryPeerId")
            .field("backend", &self.backend)
            .finish_non_exhaustive()
    }
}

/// 一次完整的来源观察；locator 的编码及身份校验属于对应后端。
pub struct DiscoveredPeer {
    pub peer: DiscoveryPeerId,
    pub locator: Vec<u8>,
    pub ttl: Duration,
}

pub enum DiscoveryEvent {
    Found(DiscoveredPeer),
    Updated(DiscoveredPeer),
    Lost { peer: DiscoveryPeerId },
}

/// 目录返回带来源与期限的候选，不返回授权或在线证明。
#[derive(Clone)]
pub struct ConnectionCandidate {
    pub peer: DiscoveryPeerId,
    pub locator: Vec<u8>,
    pub source: DiscoveryProviderId,
    pub observed_at: Instant,
    pub expires_at: Instant,
}

impl fmt::Debug for ConnectionCandidate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConnectionCandidate")
            .field("peer", &self.peer)
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}
