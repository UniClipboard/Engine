use crate::connectivity::ConnectionLease;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Weak};
use tokio::sync::Mutex;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use uc_core::ids::DeviceId;
use uc_core::ports::ReachabilityState;

#[derive(Default)]
pub(super) struct PeerEpoch {
    pub check: Mutex<()>,
    pub revision: std::sync::atomic::AtomicU64,
}
#[derive(Clone)]
pub(super) struct Attempt {
    pub peer: DeviceId,
    pub epoch: Arc<PeerEpoch>,
    pub generation: u64,
    pub verification_revision: u64,
    pub verification_was_active: bool,
    pub success: Option<u64>,
    pub stop: CancellationToken,
}
#[derive(Clone)]
pub(super) struct Retained {
    pub lease: ConnectionLease,
    pub inbound: bool,
    pub promotion_order: u64,
}
pub(super) struct Evidence {
    pub accepting: bool,
    pub terminated: bool,
    pub generation: u64,
    pub stop: CancellationToken,
    pub epochs: HashMap<DeviceId, Weak<PeerEpoch>>,
    pub states: HashMap<DeviceId, ReachabilityState>,
    pub local_failures: HashSet<DeviceId>,
    pub successes: HashMap<DeviceId, u64>,
    pub revision: u64,
    pub verified: HashMap<usize, Instant>,
    pub connections: HashMap<usize, Retained>,
}
impl Default for Evidence {
    fn default() -> Self {
        Self {
            accepting: true,
            terminated: false,
            generation: 0,
            stop: CancellationToken::new(),
            epochs: HashMap::new(),
            states: HashMap::new(),
            local_failures: HashSet::new(),
            successes: HashMap::new(),
            revision: 0,
            verified: HashMap::new(),
            connections: HashMap::new(),
        }
    }
}
impl Evidence {
    pub fn begin(&mut self, peer: DeviceId) -> Attempt {
        self.epochs.retain(|_, epoch| epoch.strong_count() > 0);
        let epoch = self
            .epochs
            .get(&peer)
            .and_then(Weak::upgrade)
            .unwrap_or_else(|| {
                let epoch = Arc::new(PeerEpoch::default());
                self.epochs.insert(peer, Arc::downgrade(&epoch));
                epoch
            });
        let verification_was_active = epoch.check.try_lock().is_err();
        Attempt {
            peer,
            verification_revision: epoch.revision.load(std::sync::atomic::Ordering::Acquire),
            verification_was_active,
            epoch,
            generation: self.generation,
            success: self.successes.get(&peer).copied(),
            stop: self.stop.clone(),
        }
    }
    pub fn current(&self, attempt: &Attempt) -> bool {
        self.accepting
            && !self.terminated
            && self.generation == attempt.generation
            && self
                .epochs
                .get(&attempt.peer)
                .and_then(Weak::upgrade)
                .is_some_and(|epoch| Arc::ptr_eq(&epoch, &attempt.epoch))
    }
    pub fn succeeded(&mut self, peer: DeviceId) {
        self.revision = self.revision.wrapping_add(1);
        self.successes.insert(peer, self.revision);
    }
}
