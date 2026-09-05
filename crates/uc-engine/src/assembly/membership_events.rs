use std::sync::Arc;

use async_trait::async_trait;
use uc_application::deps::{
    CommitMembershipLedgerPort, LoadedMembershipLedger, MembershipLedgerError,
    MembershipLedgerMutation,
};
use uc_application::facade::HostEventBus;
use uc_core::ports::{HostEvent, MembershipHostEvent};

pub(crate) fn publish_membership_commit_events(
    inner: Arc<dyn CommitMembershipLedgerPort>,
    host_events: Arc<HostEventBus>,
) -> Arc<dyn CommitMembershipLedgerPort> {
    Arc::new(PublishingMembershipCommit { inner, host_events })
}

struct PublishingMembershipCommit {
    inner: Arc<dyn CommitMembershipLedgerPort>,
    host_events: Arc<HostEventBus>,
}

#[async_trait]
impl CommitMembershipLedgerPort for PublishingMembershipCommit {
    async fn compare_and_commit(
        &self,
        mutation: MembershipLedgerMutation,
    ) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        let result = self.inner.compare_and_commit(mutation).await;
        if let Ok(committed) = &result {
            self.host_events.emit_or_warn(HostEvent::Membership(
                MembershipHostEvent::LedgerCommitted {
                    revision: committed.revision,
                },
            ));
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use uc_application::deps::{
        CommitMembershipLedgerPort, LoadedMembershipLedger, MembershipLedgerError,
        MembershipLedgerMutation,
    };
    use uc_application::facade::HostEventBus;
    use uc_core::ports::{EmitError, HostEvent, HostEventEmitterPort, MembershipHostEvent};

    use super::publish_membership_commit_events;

    struct SuccessfulCommit;

    #[async_trait]
    impl CommitMembershipLedgerPort for SuccessfulCommit {
        async fn compare_and_commit(
            &self,
            mutation: MembershipLedgerMutation,
        ) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
            Ok(mutation.replacement)
        }
    }

    struct FailingCommit;

    #[async_trait]
    impl CommitMembershipLedgerPort for FailingCommit {
        async fn compare_and_commit(
            &self,
            _mutation: MembershipLedgerMutation,
        ) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
            Err(MembershipLedgerError::Conflict)
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

    fn mutation(revision: u64) -> MembershipLedgerMutation {
        let mut replacement = LoadedMembershipLedger::no_current_space();
        replacement.revision = revision;
        MembershipLedgerMutation {
            expected_revision: revision.saturating_sub(1),
            expected_history_digest: None,
            replacement,
        }
    }

    #[tokio::test]
    async fn successful_commit_publishes_the_revision_once() {
        let recorder = Arc::new(HostEventRecorder::default());
        let host_events = Arc::new(HostEventBus::new());
        host_events.register("test", recorder.clone());
        let publisher = publish_membership_commit_events(Arc::new(SuccessfulCommit), host_events);

        let committed = publisher
            .compare_and_commit(mutation(7))
            .await
            .expect("membership commit");

        assert_eq!(committed.revision, 7);
        let events = recorder.events.lock().expect("host events lock");
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events.first(),
            Some(HostEvent::Membership(
                MembershipHostEvent::LedgerCommitted { revision: 7 }
            ))
        ));
    }

    #[tokio::test]
    async fn failed_commit_does_not_publish_an_event() {
        let recorder = Arc::new(HostEventRecorder::default());
        let host_events = Arc::new(HostEventBus::new());
        host_events.register("test", recorder.clone());
        let publisher = publish_membership_commit_events(Arc::new(FailingCommit), host_events);

        let result = publisher.compare_and_commit(mutation(1)).await;

        assert!(matches!(result, Err(MembershipLedgerError::Conflict)));
        assert!(recorder.events.lock().expect("host events lock").is_empty());
    }
}
