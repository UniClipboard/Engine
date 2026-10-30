use std::collections::VecDeque;

use async_trait::async_trait;
use tokio::sync::{Mutex, Notify};
use uc_core::ports::clipboard::{SelfWriteAttribution, SelfWriteLedgerPort};
use uc_core::ClipboardChangeOrigin;

/// 一个 Engine 实例独占的系统剪贴板归因记录。
pub(crate) struct InMemorySelfWriteLedger {
    state: Mutex<IdentityStore>,
    changed: Notify,
}

struct IdentityStore {
    pending: usize,
    records: VecDeque<(Vec<u8>, SelfWriteAttribution)>,
}

const IDENTITY_RECORD_MAX: usize = 256;
const IDENTITY_BYTES_MAX: usize = 256;

impl InMemorySelfWriteLedger {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(IdentityStore {
                pending: 0,
                records: VecDeque::new(),
            }),
            changed: Notify::new(),
        }
    }
}

#[async_trait]
impl SelfWriteLedgerPort for InMemorySelfWriteLedger {
    async fn begin_identified_write(&self) {
        self.state.lock().await.pending += 1;
    }

    async fn finish_identified_write(
        &self,
        identity: Option<Vec<u8>>,
        attribution: SelfWriteAttribution,
    ) {
        let mut state = self.state.lock().await;
        if let Some(identity) =
            identity.filter(|value| !value.is_empty() && value.len() <= IDENTITY_BYTES_MAX)
        {
            state.records.retain(|(stored, _)| stored != &identity);
            state.records.push_back((identity, attribution));
            while state.records.len() > IDENTITY_RECORD_MAX {
                state.records.pop_front();
            }
        }
        state.pending = state.pending.saturating_sub(1);
        drop(state);
        self.changed.notify_waiters();
    }

    async fn attribute_identity(&self, identity: Option<&[u8]>) -> ClipboardChangeOrigin {
        let Some(identity) =
            identity.filter(|value| !value.is_empty() && value.len() <= IDENTITY_BYTES_MAX)
        else {
            return ClipboardChangeOrigin::LocalCapture;
        };
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let state = self.state.lock().await;
            if state.pending == 0 {
                return match state.records.iter().find(|(stored, _)| stored == identity) {
                    Some((_, SelfWriteAttribution::Local)) => ClipboardChangeOrigin::LocalRestore,
                    Some((_, SelfWriteAttribution::Remote)) => {
                        ClipboardChangeOrigin::remote_push_anonymous()
                    }
                    None => ClipboardChangeOrigin::LocalCapture,
                };
            }
            drop(state);
            notified.await;
        }
    }
}
