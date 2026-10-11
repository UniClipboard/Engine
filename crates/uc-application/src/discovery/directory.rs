use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
};

use tokio::time::Instant;

use super::{
    ConnectionCandidate, DiscoveryError, DiscoveryEvent, DiscoveryPeerId, DiscoveryProviderId,
};

const MAX_IDENTITY_BYTES: usize = 512;
const MAX_LOCATOR_BYTES: usize = 16 * 1024;

/// 仅保存有界内存观察；同一 peer 的不同来源独立更新、丢失和过期。
pub struct PeerDirectory {
    observations: Mutex<HashMap<(DiscoveryPeerId, DiscoveryProviderId), ConnectionCandidate>>,
    capacity: usize,
    stopped: AtomicBool,
}

impl PeerDirectory {
    pub fn new(capacity: usize) -> Self {
        Self {
            observations: Mutex::new(HashMap::new()),
            capacity,
            stopped: AtomicBool::new(false),
        }
    }

    pub fn observe(
        &self,
        source: DiscoveryProviderId,
        event: DiscoveryEvent,
        now: Instant,
    ) -> Result<bool, DiscoveryError> {
        let mut entries = self
            .observations
            .lock()
            // discarded-source[lock-poisoned]: guard 不能作为跨线程错误来源保留。
            .map_err(|_| DiscoveryError::DirectoryPoisoned)?;
        if self.stopped.load(Ordering::Acquire) {
            return Err(DiscoveryError::Stopped);
        }
        entries.retain(|_, item| item.expires_at > now);
        let changed = match event {
            DiscoveryEvent::Lost { peer } => entries.remove(&(peer, source)).is_some(),
            DiscoveryEvent::Found(found) | DiscoveryEvent::Updated(found) => {
                if found.peer.identity().is_empty()
                    || found.peer.identity().len() > MAX_IDENTITY_BYTES
                    || found.locator.is_empty()
                    || found.locator.len() > MAX_LOCATOR_BYTES
                    || found.ttl.is_zero()
                    || self.capacity == 0
                {
                    return Err(DiscoveryError::InvalidObservation);
                }
                let expires_at = now
                    .checked_add(found.ttl)
                    .ok_or(DiscoveryError::InvalidObservation)?;
                let key = (found.peer.clone(), source);
                let changed = entries
                    .get(&key)
                    .is_none_or(|previous| previous.locator != found.locator);
                if !entries.contains_key(&key)
                    && entries
                        .values()
                        .filter(|item| item.source == source)
                        .count()
                        >= self.capacity
                {
                    if let Some(oldest) = entries
                        .iter()
                        .filter(|(_, item)| item.source == source)
                        .min_by_key(|(_, item)| item.observed_at)
                        .map(|(key, _)| key.clone())
                    {
                        entries.remove(&oldest);
                    }
                }
                entries.insert(
                    key,
                    ConnectionCandidate {
                        peer: found.peer,
                        locator: found.locator,
                        source,
                        observed_at: now,
                        expires_at,
                    },
                );
                changed
            }
        };
        Ok(changed)
    }

    pub fn candidates(
        &self,
        peer: &DiscoveryPeerId,
        now: Instant,
    ) -> Result<Vec<ConnectionCandidate>, DiscoveryError> {
        let mut entries = self
            .observations
            .lock()
            // discarded-source[lock-poisoned]: guard 不能作为跨线程错误来源保留。
            .map_err(|_| DiscoveryError::DirectoryPoisoned)?;
        entries.retain(|_, item| item.expires_at > now);
        Ok(entries
            .values()
            .filter(|item| &item.peer == peer)
            .cloned()
            .collect())
    }

    pub fn snapshot(&self, now: Instant) -> Result<Vec<ConnectionCandidate>, DiscoveryError> {
        let mut entries = self
            .observations
            .lock()
            // discarded-source[lock-poisoned]: guard 不能作为跨线程错误来源保留。
            .map_err(|_| DiscoveryError::DirectoryPoisoned)?;
        entries.retain(|_, item| item.expires_at > now);
        Ok(entries.values().cloned().collect())
    }

    pub fn remove_source(&self, source: DiscoveryProviderId) -> Result<(), DiscoveryError> {
        self.observations
            .lock()
            // discarded-source[lock-poisoned]: guard 不能作为跨线程错误来源保留。
            .map_err(|_| DiscoveryError::DirectoryPoisoned)?
            .retain(|(_, provider), _| *provider != source);
        Ok(())
    }

    pub(super) fn clear(&self) -> Result<(), DiscoveryError> {
        let mut entries = self
            .observations
            .lock()
            // discarded-source[lock-poisoned]: guard 不能作为跨线程错误来源保留。
            .map_err(|_| DiscoveryError::DirectoryPoisoned)?;
        self.stopped.store(true, Ordering::Release);
        entries.clear();
        Ok(())
    }
}
