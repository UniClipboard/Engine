use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use uc_application::deps::{
    CommitMembershipLedgerPort, LoadMembershipLedgerPort, LoadedMembershipLedger,
    MembershipLedgerError, MembershipLedgerMutation,
};
use uc_application::facade::HostEventBus;
use uc_core::ports::{HostEvent, MembershipHostEvent};

const UNKNOWN_REVISION: u64 = u64::MAX;

/// 让同一 Engine 会话中的所有账本读写共享版本，并只发布真实的设备分组变化。
pub(crate) struct MembershipLedgerAccess {
    loader: Arc<dyn LoadMembershipLedgerPort>,
    committer: Arc<dyn CommitMembershipLedgerPort>,
    host_events: Arc<HostEventBus>,
    revision: AtomicU64,
}

impl MembershipLedgerAccess {
    pub(crate) fn new(
        loader: Arc<dyn LoadMembershipLedgerPort>,
        committer: Arc<dyn CommitMembershipLedgerPort>,
        host_events: Arc<HostEventBus>,
    ) -> Self {
        Self {
            loader,
            committer,
            host_events,
            revision: AtomicU64::new(UNKNOWN_REVISION),
        }
    }

    fn remember(&self, revision: u64) {
        self.revision.store(revision, Ordering::Release);
    }
}

#[async_trait]
impl LoadMembershipLedgerPort for MembershipLedgerAccess {
    async fn load(&self) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        let loaded = self.loader.load().await?;
        self.remember(loaded.revision);
        Ok(loaded)
    }

    fn current_revision(&self) -> Option<u64> {
        match self.revision.load(Ordering::Acquire) {
            UNKNOWN_REVISION => None,
            revision => Some(revision),
        }
    }
}

#[async_trait]
impl CommitMembershipLedgerPort for MembershipLedgerAccess {
    async fn compare_and_commit(
        &self,
        mutation: MembershipLedgerMutation,
    ) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        let device_trust_changed = mutation.device_trust_changed;
        let committed = self.committer.compare_and_commit(mutation).await?;
        self.remember(committed.revision);
        if device_trust_changed {
            self.host_events.emit_or_warn(HostEvent::Membership(
                MembershipHostEvent::LedgerCommitted {
                    revision: committed.revision,
                },
            ));
        }
        Ok(committed)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use uc_application::deps::{
        CommitMembershipLedgerPort, LoadMembershipLedgerPort, LoadedMembershipLedger,
        MembershipLedgerError, MembershipLedgerMutation,
    };
    use uc_application::facade::HostEventBus;
    use uc_core::ports::{EmitError, HostEvent, HostEventEmitterPort, MembershipHostEvent};

    use super::MembershipLedgerAccess;

    struct MemoryLedger(Mutex<LoadedMembershipLedger>);

    impl Default for MemoryLedger {
        fn default() -> Self {
            Self(Mutex::new(LoadedMembershipLedger::no_current_space()))
        }
    }

    #[async_trait]
    impl LoadMembershipLedgerPort for MemoryLedger {
        async fn load(&self) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
            Ok(self.0.lock().expect("membership ledger lock").clone())
        }
    }

    #[async_trait]
    impl CommitMembershipLedgerPort for MemoryLedger {
        async fn compare_and_commit(
            &self,
            mutation: MembershipLedgerMutation,
        ) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
            if mutation.expected_revision == 99 {
                return Err(MembershipLedgerError::Conflict);
            }
            *self.0.lock().expect("membership ledger lock") = mutation.replacement.clone();
            Ok(mutation.replacement)
        }
    }

    #[derive(Default)]
    struct HostEventRecorder {
        events: Mutex<Vec<HostEvent>>,
    }

    impl HostEventEmitterPort for HostEventRecorder {
        fn emit(&self, event: HostEvent) -> Result<(), EmitError> {
            self.events.lock().expect("host events lock").push(event);
            Ok(())
        }
    }

    fn mutation(revision: u64, device_trust_changed: bool) -> MembershipLedgerMutation {
        let mut replacement = LoadedMembershipLedger::no_current_space();
        replacement.revision = revision;
        MembershipLedgerMutation {
            expected_revision: revision.saturating_sub(1),
            expected_history_digest: None,
            device_trust_changed,
            replacement,
        }
    }

    fn access() -> (Arc<MembershipLedgerAccess>, Arc<HostEventRecorder>) {
        let repository = Arc::new(MemoryLedger::default());
        let recorder = Arc::new(HostEventRecorder::default());
        let host_events = Arc::new(HostEventBus::new());
        host_events.register("test", recorder.clone());
        (
            Arc::new(MembershipLedgerAccess::new(
                repository.clone(),
                repository,
                host_events,
            )),
            recorder,
        )
    }

    #[tokio::test]
    async fn visible_commit_updates_revision_and_publishes_once() {
        let (access, recorder) = access();
        let committed = access
            .compare_and_commit(mutation(7, true))
            .await
            .expect("membership commit");

        assert_eq!(committed.revision, 7);
        assert_eq!(access.current_revision(), Some(7));
        let events = recorder.events.lock().expect("host events lock");
        assert!(matches!(
            events.as_slice(),
            [HostEvent::Membership(
                MembershipHostEvent::LedgerCommitted { revision: 7 }
            )]
        ));
    }

    #[tokio::test]
    async fn maintenance_commit_updates_revision_without_publishing() {
        let (access, recorder) = access();
        access
            .compare_and_commit(mutation(3, false))
            .await
            .expect("maintenance commit");

        assert_eq!(access.current_revision(), Some(3));
        assert!(recorder.events.lock().expect("host events lock").is_empty());
    }

    #[tokio::test]
    async fn failed_commit_changes_neither_revision_nor_events() {
        let (access, recorder) = access();
        let result = access.compare_and_commit(mutation(100, true)).await;

        assert!(matches!(result, Err(MembershipLedgerError::Conflict)));
        assert_eq!(access.current_revision(), None);
        assert!(recorder.events.lock().expect("host events lock").is_empty());
    }
}
