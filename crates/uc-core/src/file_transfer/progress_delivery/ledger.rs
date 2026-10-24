use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::Hash;
use std::time::Duration;

/// 待确认终态全局容量。
pub const TERMINAL_GLOBAL_CAPACITY: usize = 256;
/// 待确认终态每对端容量。
pub const TERMINAL_PEER_CAPACITY: usize = 64;

type Order = (Duration, u64);

/// 待确认终态的有界保留账本。
///
/// 相同键不重复占容量；某端满时只淘汰该端最早生成的项，全局满时淘汰全局最早项。
/// 被淘汰项由调用方停止补发，不改变真实本地事实。
#[derive(Debug)]
pub struct TerminalLedger<P, K> {
    order: BTreeMap<Order, (P, K)>,
    by_key: HashMap<(P, K), Order>,
    per_peer: HashMap<P, BTreeSet<Order>>,
    next_seq: u64,
}

impl<P, K> Default for TerminalLedger<P, K> {
    fn default() -> Self {
        Self {
            order: BTreeMap::new(),
            by_key: HashMap::new(),
            per_peer: HashMap::new(),
            next_seq: 0,
        }
    }
}

impl<P: Clone + Eq + Hash, K: Clone + Eq + Hash> TerminalLedger<P, K> {
    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    pub fn contains(&self, peer: &P, key: &K) -> bool {
        self.by_key.contains_key(&(peer.clone(), key.clone()))
    }

    /// 登记新终态并返回被容量淘汰的项；已存在的相同键不占用额外容量也不淘汰他项。
    pub fn admit(&mut self, peer: P, key: K, generated: Duration) -> Vec<(P, K)> {
        if self.contains(&peer, &key) {
            return Vec::new();
        }
        let order = (generated, self.next_seq);
        self.next_seq += 1;
        self.order.insert(order, (peer.clone(), key.clone()));
        self.by_key.insert((peer.clone(), key), order);
        self.per_peer.entry(peer.clone()).or_default().insert(order);

        let mut evicted = Vec::new();
        while self.per_peer.get(&peer).map_or(0, BTreeSet::len) > TERMINAL_PEER_CAPACITY {
            let earliest = self
                .per_peer
                .get(&peer)
                .and_then(|set| set.first().copied());
            evicted.extend(earliest.and_then(|order| self.remove_order(order)));
        }
        while self.order.len() > TERMINAL_GLOBAL_CAPACITY {
            let earliest = self.order.first_key_value().map(|(order, _)| *order);
            evicted.extend(earliest.and_then(|order| self.remove_order(order)));
        }
        evicted
    }

    /// 精确 ACK 或补发结束后释放。
    pub fn release(&mut self, peer: &P, key: &K) -> bool {
        let order = self.by_key.get(&(peer.clone(), key.clone())).copied();
        order.is_some_and(|order| self.remove_order(order).is_some())
    }

    /// 移除生成时间不晚于 `cutoff` 的全部项，用于保留期到期清理。
    pub fn expire_through(&mut self, cutoff: Duration) -> Vec<(P, K)> {
        let due: Vec<Order> = self
            .order
            .range(..=(cutoff, u64::MAX))
            .map(|(order, _)| *order)
            .collect();
        due.into_iter()
            .filter_map(|order| self.remove_order(order))
            .collect()
    }

    fn remove_order(&mut self, order: Order) -> Option<(P, K)> {
        let (peer, key) = self.order.remove(&order)?;
        self.by_key.remove(&(peer.clone(), key.clone()));
        if let Some(set) = self.per_peer.get_mut(&peer) {
            set.remove(&order);
            if set.is_empty() {
                self.per_peer.remove(&peer);
            }
        }
        Some((peer, key))
    }
}
