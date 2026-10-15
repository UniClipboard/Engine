//! 节点连接普查：只持有弱连接句柄，周期性汇总连接与路径数量，用于定位连接/路径无界增长。
//!
//! 记录只含计数；对端身份仅在一次快照内部用于分组，不进入输出。
use iroh::endpoint::WeakConnectionHandle;
use iroh::TransportAddr;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::task::JoinHandle;
use uc_observability_contract::uc_info;

/// 登记表容量；超出时拒绝登记，避免普查自身成为无界容器。
const MAX_TRACKED: usize = 4096;
const CADENCE: Duration = Duration::from_secs(300);

#[derive(Clone, Default)]
pub(super) struct ConnectionCensus {
    tracked: Arc<Mutex<Vec<(WeakConnectionHandle, [u8; 32])>>>,
}

/// 一次快照的计数结果。
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct CensusSnapshot {
    pub(super) connections: u64,
    pub(super) per_peer_max: u64,
    pub(super) paths_total: u64,
    pub(super) paths_max: u64,
    pub(super) paths_direct: u64,
    pub(super) paths_relay: u64,
}

impl ConnectionCensus {
    pub(super) fn register(&self, handle: WeakConnectionHandle, peer: [u8; 32]) {
        let mut tracked = self
            .tracked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if tracked.len() >= MAX_TRACKED {
            tracked.retain(|(handle, _)| handle.upgrade().is_some());
        }
        if tracked.len() < MAX_TRACKED {
            tracked.push((handle, peer));
        }
    }

    pub(super) fn snapshot(&self) -> CensusSnapshot {
        let live: Vec<_> = {
            let mut tracked = self
                .tracked
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut live = Vec::with_capacity(tracked.len());
            tracked.retain(|(handle, peer)| match handle.upgrade() {
                Some(connection) => {
                    live.push((connection, *peer));
                    true
                }
                None => false,
            });
            live
        };
        let mut per_peer: HashMap<[u8; 32], u64> = HashMap::new();
        let mut snapshot = CensusSnapshot::default();
        for (connection, peer) in &live {
            snapshot.connections += 1;
            *per_peer.entry(*peer).or_default() += 1;
            let mut paths = 0u64;
            for path in connection.paths().iter() {
                paths += 1;
                match path.remote_addr() {
                    TransportAddr::Ip(_) => snapshot.paths_direct += 1,
                    TransportAddr::Relay(_) => snapshot.paths_relay += 1,
                    _ => {}
                }
            }
            snapshot.paths_total += paths;
            snapshot.paths_max = snapshot.paths_max.max(paths);
        }
        snapshot.per_peer_max = per_peer.values().copied().max().unwrap_or(0);
        snapshot
    }

    fn emit(&self) {
        let snapshot = self.snapshot();
        uc_info!(
            conn_total = snapshot.connections,
            conn_per_peer_max = snapshot.per_peer_max,
            paths_total = snapshot.paths_total,
            paths_max = snapshot.paths_max,
            paths_direct = snapshot.paths_direct,
            paths_relay = snapshot.paths_relay,
            "network connection census"
        );
    }

    /// 周期任务由节点持有并在关闭时 abort；首条记录在一个周期之后。
    pub(super) fn spawn(&self) -> JoinHandle<()> {
        let census = self.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(CADENCE);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            ticker.tick().await;
            loop {
                ticker.tick().await;
                census.emit();
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::{Endpoint, RelayMode};

    async fn pair() -> (
        Endpoint,
        Endpoint,
        iroh::endpoint::Connection,
        iroh::endpoint::Connection,
    ) {
        let server = Endpoint::builder(iroh::endpoint::presets::N0)
            .relay_mode(RelayMode::Disabled)
            .alpns(vec![b"census-test".to_vec()])
            .bind()
            .await
            .unwrap();
        let client = Endpoint::builder(iroh::endpoint::presets::N0)
            .relay_mode(RelayMode::Disabled)
            .bind()
            .await
            .unwrap();
        for _ in 0..100 {
            if !server.addr().addrs.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let (incoming, outgoing) = tokio::join!(
            async { server.accept().await.unwrap().await.unwrap() },
            client.connect(server.addr(), b"census-test")
        );
        (server, client, incoming, outgoing.unwrap())
    }

    #[tokio::test]
    async fn counts_live_connections_and_forgets_dropped_ones() {
        let (server, client, incoming, outgoing) = pair().await;
        let census = ConnectionCensus::default();
        census.register(outgoing.weak_handle(), [1; 32]);
        census.register(incoming.weak_handle(), [1; 32]);

        let live = census.snapshot();
        assert_eq!(live.connections, 2);
        assert_eq!(live.per_peer_max, 2);
        assert!(live.paths_total >= 2);
        assert!(live.paths_direct >= 2);

        drop(outgoing);
        drop(incoming);
        client.close().await;
        server.close().await;
        assert_eq!(census.snapshot(), CensusSnapshot::default());
    }

    #[tokio::test]
    async fn registry_holds_no_strong_reference() {
        let (server, client, incoming, outgoing) = pair().await;
        let census = ConnectionCensus::default();
        census.register(outgoing.weak_handle(), [2; 32]);
        drop(outgoing);
        drop(incoming);
        client.close().await;
        server.close().await;
        // 没有任何强引用时，快照不应把连接保活。
        assert_eq!(census.snapshot().connections, 0);
        assert!(census.tracked.lock().unwrap().is_empty());
    }
}
