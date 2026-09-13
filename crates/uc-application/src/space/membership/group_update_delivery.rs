use std::collections::HashSet;
use std::sync::Arc;
use uc_observability_contract::diagnostics::connectivity::{
    record_pending_group_updates, LocalWorkObservation, LocalWorkOutcome, LocalWorkStep,
};

use uc_core::membership::{
    GroupRevocationPort, GroupUpdateDispatchError, GroupUpdateDispatchPort, KeyEpochError,
};

use super::{
    DeliverPendingGroupUpdatesPort, MembershipMaintenanceStepOutcome, MembershipMaintenanceTrigger,
};

const MAX_UPDATES_PER_ROUND: usize = 8;

/// 待投递 Group Epoch 的唯一完整负责人。
///
/// 调用方只触发一轮维护；本类内部隐藏持久欠账、认证投递与确认删除的顺序。
pub(crate) struct DeliverPendingGroupUpdatesUseCase {
    store: Arc<dyn GroupRevocationPort>,
    dispatch: Arc<dyn GroupUpdateDispatchPort>,
    clock: Arc<dyn uc_core::ports::ClockPort>,
}

impl DeliverPendingGroupUpdatesUseCase {
    pub(crate) fn new(
        store: Arc<dyn GroupRevocationPort>,
        dispatch: Arc<dyn GroupUpdateDispatchPort>,
        clock: Arc<dyn uc_core::ports::ClockPort>,
    ) -> Self {
        Self {
            store,
            dispatch,
            clock,
        }
    }
}

#[async_trait::async_trait]
impl DeliverPendingGroupUpdatesPort for DeliverPendingGroupUpdatesUseCase {
    async fn deliver_pending_group_updates(
        &self,
        trigger: &MembershipMaintenanceTrigger,
    ) -> MembershipMaintenanceStepOutcome {
        let online_peer = match trigger {
            MembershipMaintenanceTrigger::PeerOnline(peer) => Some(*peer),
            _ => None,
        };
        let pending = match self
            .store
            .due_space_group_updates(self.clock.now_ms(), online_peer)
            .await
        {
            Ok(pending) => pending,
            Err(error) => return classify_store_error(&error),
        };
        let mut outcome = MembershipMaintenanceStepOutcome::Completed;
        let mut failures = Vec::new();
        let mut unavailable_peers = HashSet::new();
        record_pending_group_updates(pending.len(), pending.len().min(MAX_UPDATES_PER_ROUND));

        for update in pending.iter().take(MAX_UPDATES_PER_ROUND) {
            if unavailable_peers.contains(update.recipient()) {
                continue;
            }
            let observation =
                LocalWorkObservation::begin(LocalWorkStep::MaintenanceGroupUpdateDispatch);
            let dispatched = self.dispatch.dispatch_group_update(update).await;
            observation.finish(match &dispatched {
                Ok(()) => LocalWorkOutcome::Ok,
                Err(GroupUpdateDispatchError::Offline | GroupUpdateDispatchError::Transport) => {
                    LocalWorkOutcome::Deferred
                }
                Err(GroupUpdateDispatchError::Rejected) => LocalWorkOutcome::Rejected,
            });
            match dispatched {
                Ok(()) => match self
                    .store
                    .acknowledge_space_group_update(update.update_id(), self.clock.now_ms())
                    .await
                {
                    Ok(true) => {}
                    Ok(false) => outcome = MembershipMaintenanceStepOutcome::StableFailure,
                    Err(error) => return classify_store_error(&error),
                },
                Err(
                    error @ (GroupUpdateDispatchError::Offline
                    | GroupUpdateDispatchError::Transport),
                ) => {
                    failures.push((update.update_id().to_owned(), error));
                    unavailable_peers.insert(*update.recipient());
                    outcome = MembershipMaintenanceStepOutcome::Deferred;
                }
                Err(GroupUpdateDispatchError::Rejected) => {
                    failures.push((
                        update.update_id().to_owned(),
                        GroupUpdateDispatchError::Rejected,
                    ));
                    unavailable_peers.insert(*update.recipient());
                    if outcome != MembershipMaintenanceStepOutcome::Deferred {
                        outcome = MembershipMaintenanceStepOutcome::StableFailure;
                    }
                }
            }
        }

        if !failures.is_empty() {
            match self
                .store
                .record_space_group_update_failures(&failures, self.clock.now_ms())
                .await
            {
                Ok(deferred) if deferred == failures.len() => {}
                Ok(_) => outcome = MembershipMaintenanceStepOutcome::StableFailure,
                Err(error) => return classify_store_error(&error),
            }
        }

        if pending.len() > MAX_UPDATES_PER_ROUND {
            MembershipMaintenanceStepOutcome::Deferred
        } else {
            outcome
        }
    }
}

fn classify_store_error(error: &KeyEpochError) -> MembershipMaintenanceStepOutcome {
    match error {
        KeyEpochError::Repository(_)
        | KeyEpochError::StateIssue(_)
        | KeyEpochError::SecurityState { .. }
        | KeyEpochError::SpaceNotReady => MembershipMaintenanceStepOutcome::Deferred,
        _ => MembershipMaintenanceStepOutcome::Corrupt,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use uc_core::ids::DeviceId;
    use uc_core::membership::*;

    use super::*;

    struct FixedClock;

    impl uc_core::ports::ClockPort for FixedClock {
        fn now_ms(&self) -> i64 {
            42
        }
    }

    struct RecordingStore {
        pending: Mutex<Vec<PendingGroupUpdate>>,
        acknowledged: Mutex<Vec<String>>,
        deferred_batches: Mutex<Vec<Vec<String>>>,
    }

    #[async_trait]
    impl GroupRevocationPort for RecordingStore {
        async fn revoke_group_member(
            &self,
            _: &DeviceId,
            _: &[DeviceId],
            _: i64,
        ) -> Result<GroupRevocationResult, KeyEpochError> {
            unreachable!()
        }
        async fn acknowledge_group_update(
            &self,
            _: &RevocationId,
            _: &DeviceId,
            _: i64,
        ) -> Result<GroupRevocationResult, KeyEpochError> {
            unreachable!()
        }
        async fn apply_group_epoch_update(&self, _: &[u8]) -> Result<GroupEpoch, KeyEpochError> {
            unreachable!()
        }
        async fn pending_group_updates(
            &self,
            _: &RevocationId,
        ) -> Result<Vec<PendingGroupUpdate>, KeyEpochError> {
            unreachable!()
        }
        async fn query_group_revocation(
            &self,
            _: &RevocationId,
        ) -> Result<Option<GroupRevocationResult>, KeyEpochError> {
            unreachable!()
        }
        async fn resume_group_revocations(
            &self,
            _: i64,
        ) -> Result<Vec<GroupRevocationResult>, KeyEpochError> {
            unreachable!()
        }

        async fn due_space_group_updates(
            &self,
            _: i64,
            _: Option<DeviceId>,
        ) -> Result<Vec<PendingGroupUpdate>, KeyEpochError> {
            Ok(self.pending.lock().unwrap().clone())
        }

        async fn record_space_group_update_failures(
            &self,
            failures: &[(String, GroupUpdateDispatchError)],
            _: i64,
        ) -> Result<usize, KeyEpochError> {
            let update_ids = failures
                .iter()
                .map(|(update_id, _)| update_id.clone())
                .collect::<Vec<_>>();
            self.deferred_batches
                .lock()
                .unwrap()
                .push(update_ids.clone());
            let mut pending = self.pending.lock().unwrap();
            let mut deferred = Vec::new();
            pending.retain(|update| {
                if update_ids.iter().any(|id| id == update.update_id()) {
                    deferred.push(update.clone());
                    false
                } else {
                    true
                }
            });
            let count = deferred.len();
            pending.extend(deferred);
            Ok(count)
        }

        async fn acknowledge_space_group_update(
            &self,
            update_id: &str,
            _: i64,
        ) -> Result<bool, KeyEpochError> {
            self.acknowledged.lock().unwrap().push(update_id.to_owned());
            self.pending
                .lock()
                .unwrap()
                .retain(|update| update.update_id() != update_id);
            Ok(true)
        }
    }

    struct RecordingDispatch {
        outcomes: Mutex<VecDeque<Result<(), GroupUpdateDispatchError>>>,
        dispatched: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl GroupUpdateDispatchPort for RecordingDispatch {
        async fn dispatch_group_update(
            &self,
            update: &PendingGroupUpdate,
        ) -> Result<(), GroupUpdateDispatchError> {
            self.dispatched
                .lock()
                .unwrap()
                .push(update.update_id().to_owned());
            self.outcomes.lock().unwrap().pop_front().unwrap_or(Ok(()))
        }
    }

    fn dispatch_with(
        outcomes: impl IntoIterator<Item = Result<(), GroupUpdateDispatchError>>,
    ) -> Arc<RecordingDispatch> {
        Arc::new(RecordingDispatch {
            outcomes: Mutex::new(outcomes.into_iter().collect()),
            dispatched: Mutex::new(Vec::new()),
        })
    }

    #[tokio::test]
    async fn accepted_update_is_acknowledged_in_durable_store() {
        let update = PendingGroupUpdate::persistent(DeviceId::new("peer-a"), vec![1, 2, 3]);
        let update_id = update.update_id().to_owned();
        let store = Arc::new(RecordingStore {
            pending: Mutex::new(vec![update]),
            acknowledged: Mutex::new(Vec::new()),
            deferred_batches: Mutex::new(Vec::new()),
        });
        let use_case = DeliverPendingGroupUpdatesUseCase::new(
            store.clone(),
            dispatch_with([Ok(())]),
            Arc::new(FixedClock),
        );

        let outcome = use_case
            .deliver_pending_group_updates(&MembershipMaintenanceTrigger::Periodic)
            .await;

        assert_eq!(outcome, MembershipMaintenanceStepOutcome::Completed);
        assert_eq!(store.acknowledged.lock().unwrap().as_slice(), &[update_id]);
    }

    #[tokio::test]
    async fn transient_delivery_failure_keeps_update_pending_for_retry() {
        let update = PendingGroupUpdate::persistent(DeviceId::new("peer-a"), vec![1]);
        let store = Arc::new(RecordingStore {
            pending: Mutex::new(vec![update]),
            acknowledged: Mutex::new(Vec::new()),
            deferred_batches: Mutex::new(Vec::new()),
        });
        let use_case = DeliverPendingGroupUpdatesUseCase::new(
            store.clone(),
            dispatch_with([Err(GroupUpdateDispatchError::Offline)]),
            Arc::new(FixedClock),
        );

        let outcome = use_case
            .deliver_pending_group_updates(&MembershipMaintenanceTrigger::Periodic)
            .await;

        assert_eq!(outcome, MembershipMaintenanceStepOutcome::Deferred);
        assert!(store.acknowledged.lock().unwrap().is_empty());
        assert_eq!(store.deferred_batches.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn one_round_is_bounded_and_leaves_overflow_pending() {
        let pending: Vec<_> = (0..10)
            .map(|index| {
                PendingGroupUpdate::persistent(DeviceId::new(format!("peer-{index}")), vec![1])
            })
            .collect();
        let store = Arc::new(RecordingStore {
            pending: Mutex::new(pending),
            acknowledged: Mutex::new(Vec::new()),
            deferred_batches: Mutex::new(Vec::new()),
        });
        let dispatch = dispatch_with(std::iter::repeat_n(Ok(()), 10));
        let use_case = DeliverPendingGroupUpdatesUseCase::new(
            store.clone(),
            dispatch.clone(),
            Arc::new(FixedClock),
        );

        let outcome = use_case
            .deliver_pending_group_updates(&MembershipMaintenanceTrigger::Periodic)
            .await;

        assert_eq!(outcome, MembershipMaintenanceStepOutcome::Deferred);
        assert_eq!(
            dispatch.dispatched.lock().unwrap().len(),
            MAX_UPDATES_PER_ROUND
        );
        assert_eq!(
            store.acknowledged.lock().unwrap().len(),
            MAX_UPDATES_PER_ROUND
        );
    }

    #[tokio::test]
    async fn deferred_updates_rotate_durably_so_later_recipients_are_not_starved() {
        let pending: Vec<_> = (0..10)
            .map(|index| {
                PendingGroupUpdate::persistent(DeviceId::new(format!("peer-{index}")), vec![1])
            })
            .collect();
        let later_update_ids = pending[MAX_UPDATES_PER_ROUND..]
            .iter()
            .map(|update| update.update_id().to_owned())
            .collect::<Vec<_>>();
        let store = Arc::new(RecordingStore {
            pending: Mutex::new(pending),
            acknowledged: Mutex::new(Vec::new()),
            deferred_batches: Mutex::new(Vec::new()),
        });
        let dispatch = dispatch_with(std::iter::repeat_n(
            Err(GroupUpdateDispatchError::Offline),
            MAX_UPDATES_PER_ROUND,
        ));
        let use_case =
            DeliverPendingGroupUpdatesUseCase::new(store.clone(), dispatch, Arc::new(FixedClock));

        assert_eq!(
            use_case
                .deliver_pending_group_updates(&MembershipMaintenanceTrigger::Periodic)
                .await,
            MembershipMaintenanceStepOutcome::Deferred
        );
        assert_eq!(store.deferred_batches.lock().unwrap().len(), 1);
        assert_eq!(
            store.deferred_batches.lock().unwrap()[0].len(),
            MAX_UPDATES_PER_ROUND
        );
        assert_eq!(
            use_case
                .deliver_pending_group_updates(&MembershipMaintenanceTrigger::Periodic)
                .await,
            MembershipMaintenanceStepOutcome::Deferred
        );

        let acknowledged = store.acknowledged.lock().unwrap();
        assert!(later_update_ids
            .iter()
            .all(|update_id| acknowledged.contains(update_id)));
        assert_eq!(acknowledged.len(), MAX_UPDATES_PER_ROUND);
    }
}
