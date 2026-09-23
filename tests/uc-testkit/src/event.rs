use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use serde::Serialize;
use tokio::sync::watch;

use crate::budget::lock;

const EVENT_CAPACITY: usize = 128;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ScenarioEvent {
    pub sequence: u64,
    pub kind: String,
}

impl ScenarioEvent {
    pub fn kind(&self) -> &str {
        &self.kind
    }
}

pub(crate) struct EventLog {
    events: Mutex<VecDeque<ScenarioEvent>>,
    next_sequence: AtomicU64,
    revision: watch::Sender<u64>,
}

impl EventLog {
    pub(crate) fn new() -> Arc<Self> {
        let (revision, _) = watch::channel(0);
        Arc::new(Self {
            events: Mutex::new(VecDeque::with_capacity(EVENT_CAPACITY)),
            next_sequence: AtomicU64::new(1),
            revision,
        })
    }

    fn record(&self, kind: &'static str) {
        let sequence = self.next_sequence.fetch_add(1, Ordering::Relaxed);
        let mut events = lock(&self.events);
        if events.len() == EVENT_CAPACITY {
            events.pop_front();
        }
        events.push_back(ScenarioEvent {
            sequence,
            kind: kind.to_owned(),
        });
        drop(events);
        self.revision.send_replace(sequence);
    }

    pub(crate) fn snapshot(&self) -> Vec<ScenarioEvent> {
        lock(&self.events).iter().cloned().collect()
    }

    pub(crate) fn last(&self) -> Option<ScenarioEvent> {
        lock(&self.events).back().cloned()
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<u64> {
        self.revision.subscribe()
    }
}

#[derive(Clone)]
pub struct EventRecorder {
    log: Arc<EventLog>,
}

impl EventRecorder {
    pub(crate) fn new(log: Arc<EventLog>) -> Self {
        Self { log }
    }

    pub fn record(&self, kind: &'static str) {
        self.log.record(kind);
    }
}
