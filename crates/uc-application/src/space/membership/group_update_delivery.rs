use std::collections::HashSet;
use std::sync::Arc;
use uc_observability_contract::diagnostics::connectivity::{
    record_pending_group_updates, LocalWorkObservation, LocalWorkOutcome, LocalWorkStep,
};

use uc_core::ids::DeviceId;
use uc_core::membership::{
    GroupRevocationPort, GroupUpdateDeliveryStatus, GroupUpdateDispatchError,
    GroupUpdateDispatchPort, KeyEpochError, PendingGroupUpdate,
};
use uc_core::ports::{ClockPort, HostEvent, MembershipHostEvent};

use crate::support::host_event_bus::HostEventBus;

use super::{
    DeliverPendingGroupUpdatesPort, LoadSecurityDeviceUpdateStatusPort,
    MembershipMaintenanceStepOutcome, MembershipMaintenanceTrigger, QueryDeviceTrustError,
    SpaceDeviceUpdateProblem, SpaceDeviceUpdateRecovery, SpaceDeviceUpdateStatus,
};

const MAX_UPDATES_PER_ROUND: usize = 8;

/// 仍应接收设备组更新的收件人名单来源。
///
/// 名单只能由成员账本按已提交的成员历史导出；无法导出时返回 `None`，
/// 调用方必须保持队列原样，不得按空名单清空。
#[async_trait::async_trait]
pub(crate) trait RetainedGroupUpdateRecipientsPort: Send + Sync {
    async fn retained_group_update_recipients(&self) -> Option<Vec<DeviceId>>;
}

/// 待投递 Group Epoch 的唯一完整负责人。
///
/// 调用方只触发一轮维护；本类内部隐藏持久欠账、认证投递与确认删除的顺序。
pub(crate) struct DeliverPendingGroupUpdatesUseCase {
    store: Arc<dyn GroupRevocationPort>,
    dispatch: Arc<dyn GroupUpdateDispatchPort>,
    recipients: Arc<dyn RetainedGroupUpdateRecipientsPort>,
    host_events: Arc<HostEventBus>,
    clock: Arc<dyn ClockPort>,
}

#[async_trait::async_trait]
impl LoadSecurityDeviceUpdateStatusPort for DeliverPendingGroupUpdatesUseCase {
    async fn load_security_device_update_status(
        &self,
    ) -> Result<SpaceDeviceUpdateStatus, QueryDeviceTrustError> {
        let status = self
            .store
            .space_group_update_delivery_status()
            .await
            .map_err(map_query_error)?;
        Ok(match status {
            GroupUpdateDeliveryStatus::Completed => SpaceDeviceUpdateStatus::completed(),
            GroupUpdateDeliveryStatus::Pending { next_attempt_at_ms }
                if next_attempt_at_ms > self.clock.now_ms() =>
            {
                SpaceDeviceUpdateStatus::retryable_failure(next_attempt_at_ms)
            }
            GroupUpdateDeliveryStatus::Pending { .. } => SpaceDeviceUpdateStatus::updating(),
            GroupUpdateDeliveryStatus::Rejected => SpaceDeviceUpdateStatus::needs_attention(
                SpaceDeviceUpdateProblem::DeviceSecurityUpdateRejected,
                SpaceDeviceUpdateRecovery::ReviewDevices,
            ),
        })
    }
}

fn map_query_error(error: KeyEpochError) -> QueryDeviceTrustError {
    match error {
        KeyEpochError::Repository(_)
        | KeyEpochError::StateIssue(_)
        | KeyEpochError::SecurityState { .. }
        | KeyEpochError::SpaceNotReady => QueryDeviceTrustError::Dependency {
            source: anyhow::Error::new(error),
        },
        _ => QueryDeviceTrustError::RecoveryRequired,
    }
}

impl DeliverPendingGroupUpdatesUseCase {
    pub(crate) fn new(
        store: Arc<dyn GroupRevocationPort>,
        dispatch: Arc<dyn GroupUpdateDispatchPort>,
        recipients: Arc<dyn RetainedGroupUpdateRecipientsPort>,
        host_events: Arc<HostEventBus>,
        clock: Arc<dyn ClockPort>,
    ) -> Self {
        Self {
            store,
            dispatch,
            recipients,
            host_events,
            clock,
        }
    }

    /// 结清收件人已失去资格的待投递项。
    ///
    /// 收件人不在当前生效成员中意味着投递理由可证明消失：它已无权持有新密钥，
    /// 因此立即结清而不设窗口。本机自己的冗余项不在保留名单中，由同一条规则结清。
    /// 名单无法导出时保持队列原样并推迟本轮，绝不按空名单清空。
    async fn settle_obsolete_updates(&self) -> Result<(), MembershipMaintenanceStepOutcome> {
        let Some(retained) = self.recipients.retained_group_update_recipients().await else {
            return Err(MembershipMaintenanceStepOutcome::Deferred);
        };
        let settled = self
            .store
            .settle_obsolete_space_group_updates(&retained, self.clock.now_ms())
            .await
            .map_err(|error| classify_store_error(&error))?;
        if settled > 0 {
            self.host_events.emit_or_warn(HostEvent::Membership(
                MembershipHostEvent::SpaceDeviceUpdateChanged,
            ));
        }
        Ok(())
    }

    /// 已到期的更新，加上发给刚确认成员历史的对端、仍在退避中的更新。对端刚完成已认证交换，
    /// 等待退避到期只会让它继续缺少新的组密钥。
    async fn deliverable_updates(
        &self,
        reachable_peers: &[DeviceId],
    ) -> Result<Vec<PendingGroupUpdate>, KeyEpochError> {
        let now_ms = self.clock.now_ms();
        let mut pending = self.store.due_space_group_updates(now_ms, None).await?;
        for peer in reachable_peers {
            let expedited = self
                .store
                .due_space_group_updates(now_ms, Some(*peer))
                .await?;
            for update in expedited {
                if update.recipient() == peer
                    && !pending
                        .iter()
                        .any(|known| known.update_id() == update.update_id())
                {
                    pending.push(update);
                }
            }
        }
        Ok(pending)
    }
}

#[async_trait::async_trait]
impl DeliverPendingGroupUpdatesPort for DeliverPendingGroupUpdatesUseCase {
    async fn deliver_pending_group_updates(
        &self,
        _trigger: &MembershipMaintenanceTrigger,
        reachable_peers: &[DeviceId],
    ) -> MembershipMaintenanceStepOutcome {
        if let Err(outcome) = self.settle_obsolete_updates().await {
            return outcome;
        }
        let pending = match self.deliverable_updates(reachable_peers).await {
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
                Err(
                    GroupUpdateDispatchError::Offline { .. }
                    | GroupUpdateDispatchError::Transport { .. },
                ) => LocalWorkOutcome::Deferred,
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
                    error @ (GroupUpdateDispatchError::Offline { .. }
                    | GroupUpdateDispatchError::Transport { .. }),
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
        /// 仍在投递退避中的更新：只有查询时指明其收件人才返回，与持久存储的语义一致。
        backed_off: Mutex<Vec<PendingGroupUpdate>>,
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
            online_peer: Option<DeviceId>,
        ) -> Result<Vec<PendingGroupUpdate>, KeyEpochError> {
            let mut due = self.pending.lock().unwrap().clone();
            if let Some(peer) = online_peer {
                due.extend(
                    self.backed_off
                        .lock()
                        .unwrap()
                        .iter()
                        .filter(|update| *update.recipient() == peer)
                        .cloned(),
                );
            }
            Ok(due)
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

        async fn space_group_update_delivery_status(
            &self,
        ) -> Result<GroupUpdateDeliveryStatus, KeyEpochError> {
            Ok(GroupUpdateDeliveryStatus::Completed)
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

        async fn settle_obsolete_space_group_updates(
            &self,
            retained_recipients: &[DeviceId],
            _: i64,
        ) -> Result<usize, KeyEpochError> {
            let mut pending = self.pending.lock().unwrap();
            let before = pending.len();
            pending.retain(|update| retained_recipients.contains(update.recipient()));
            Ok(before - pending.len())
        }
    }

    /// 名单来源替身：`None` 表示成员历史不可用。
    struct FixedRecipients(Option<Vec<DeviceId>>);

    #[async_trait]
    impl RetainedGroupUpdateRecipientsPort for FixedRecipients {
        async fn retained_group_update_recipients(&self) -> Option<Vec<DeviceId>> {
            self.0.clone()
        }
    }

    /// 既有用例不针对结清，保留队列中当前全部收件人以维持原有行为。
    fn retaining_all(store: &RecordingStore) -> Arc<FixedRecipients> {
        Arc::new(FixedRecipients(Some(
            store
                .pending
                .lock()
                .unwrap()
                .iter()
                .map(|update| update.recipient().clone())
                .collect(),
        )))
    }

    fn retaining(recipients: &[&str]) -> Arc<FixedRecipients> {
        Arc::new(FixedRecipients(Some(
            recipients.iter().map(|id| DeviceId::new(id)).collect(),
        )))
    }

    fn refresh_events() -> (Arc<HostEventBus>, Arc<Mutex<Vec<HostEvent>>>) {
        struct Recorder(Arc<Mutex<Vec<HostEvent>>>);
        impl uc_core::ports::HostEventEmitterPort for Recorder {
            fn emit(&self, event: HostEvent) -> Result<(), uc_core::ports::EmitError> {
                self.0.lock().unwrap().push(event);
                Ok(())
            }
        }
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let bus = Arc::new(HostEventBus::new());
        bus.register("test", Arc::new(Recorder(Arc::clone(&recorded))));
        (bus, recorded)
    }

    fn store_with(pending: Vec<PendingGroupUpdate>) -> Arc<RecordingStore> {
        Arc::new(RecordingStore {
            pending: Mutex::new(pending),
            backed_off: Mutex::new(Vec::new()),
            acknowledged: Mutex::new(Vec::new()),
            deferred_batches: Mutex::new(Vec::new()),
        })
    }

    #[tokio::test]
    async fn update_for_removed_recipient_is_settled_without_dispatch() {
        let update = PendingGroupUpdate::persistent(DeviceId::new("peer-removed"), vec![1]);
        let store = store_with(vec![update]);
        let dispatch = dispatch_with([]);
        let (bus, recorded) = refresh_events();
        let use_case = DeliverPendingGroupUpdatesUseCase::new(
            store.clone(),
            dispatch.clone(),
            retaining(&["peer-still-member"]),
            bus,
            Arc::new(FixedClock),
        );

        let outcome = use_case
            .deliver_pending_group_updates(&MembershipMaintenanceTrigger::Periodic, &[])
            .await;

        assert_eq!(outcome, MembershipMaintenanceStepOutcome::Completed);
        assert!(store.pending.lock().unwrap().is_empty());
        assert!(dispatch.dispatched.lock().unwrap().is_empty());
        assert!(matches!(
            recorded.lock().unwrap().as_slice(),
            [HostEvent::Membership(
                MembershipHostEvent::SpaceDeviceUpdateChanged
            )]
        ));
    }

    #[tokio::test]
    async fn update_for_current_member_is_kept_and_dispatched() {
        let update = PendingGroupUpdate::persistent(DeviceId::new("peer-a"), vec![1]);
        let update_id = update.update_id().to_owned();
        let store = store_with(vec![update]);
        let dispatch = dispatch_with([Ok(())]);
        let (bus, recorded) = refresh_events();
        let use_case = DeliverPendingGroupUpdatesUseCase::new(
            store.clone(),
            dispatch.clone(),
            retaining(&["peer-a"]),
            bus,
            Arc::new(FixedClock),
        );

        let outcome = use_case
            .deliver_pending_group_updates(&MembershipMaintenanceTrigger::Periodic, &[])
            .await;

        assert_eq!(outcome, MembershipMaintenanceStepOutcome::Completed);
        assert_eq!(dispatch.dispatched.lock().unwrap().as_slice(), [update_id]);
        assert!(recorded.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn backed_off_update_is_delivered_once_its_recipient_confirms_history() {
        let returning = PendingGroupUpdate::persistent(DeviceId::new("peer-returning"), vec![1]);
        let returning_id = returning.update_id().to_owned();
        let still_offline = PendingGroupUpdate::persistent(DeviceId::new("peer-offline"), vec![2]);
        let store = store_with(Vec::new());
        store
            .backed_off
            .lock()
            .unwrap()
            .extend([returning, still_offline]);
        let dispatch = dispatch_with([Ok(())]);
        let (bus, _) = refresh_events();
        let use_case = DeliverPendingGroupUpdatesUseCase::new(
            store.clone(),
            dispatch.clone(),
            retaining(&["peer-returning", "peer-offline"]),
            bus,
            Arc::new(FixedClock),
        );

        let waiting = use_case
            .deliver_pending_group_updates(&MembershipMaintenanceTrigger::Periodic, &[])
            .await;
        assert_eq!(waiting, MembershipMaintenanceStepOutcome::Completed);
        assert!(dispatch.dispatched.lock().unwrap().is_empty());

        let outcome = use_case
            .deliver_pending_group_updates(
                &MembershipMaintenanceTrigger::StateChanged,
                &[DeviceId::new("peer-returning")],
            )
            .await;

        assert_eq!(outcome, MembershipMaintenanceStepOutcome::Completed);
        assert_eq!(
            dispatch.dispatched.lock().unwrap().as_slice(),
            [returning_id]
        );
    }

    #[tokio::test]
    async fn unavailable_membership_history_keeps_the_queue_intact() {
        let update = PendingGroupUpdate::persistent(DeviceId::new("peer-a"), vec![1]);
        let store = store_with(vec![update]);
        let dispatch = dispatch_with([]);
        let (bus, _) = refresh_events();
        let use_case = DeliverPendingGroupUpdatesUseCase::new(
            store.clone(),
            dispatch.clone(),
            Arc::new(FixedRecipients(None)),
            bus,
            Arc::new(FixedClock),
        );

        let outcome = use_case
            .deliver_pending_group_updates(&MembershipMaintenanceTrigger::Periodic, &[])
            .await;

        assert_eq!(outcome, MembershipMaintenanceStepOutcome::Deferred);
        assert_eq!(store.pending.lock().unwrap().len(), 1);
        assert!(dispatch.dispatched.lock().unwrap().is_empty());
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
            backed_off: Mutex::new(Vec::new()),
            acknowledged: Mutex::new(Vec::new()),
            deferred_batches: Mutex::new(Vec::new()),
        });
        let use_case = DeliverPendingGroupUpdatesUseCase::new(
            store.clone(),
            dispatch_with([Ok(())]),
            retaining_all(&store),
            refresh_events().0,
            Arc::new(FixedClock),
        );

        let outcome = use_case
            .deliver_pending_group_updates(&MembershipMaintenanceTrigger::Periodic, &[])
            .await;

        assert_eq!(outcome, MembershipMaintenanceStepOutcome::Completed);
        assert_eq!(store.acknowledged.lock().unwrap().as_slice(), &[update_id]);
    }

    #[tokio::test]
    async fn transient_delivery_failure_keeps_update_pending_for_retry() {
        let update = PendingGroupUpdate::persistent(DeviceId::new("peer-a"), vec![1]);
        let store = Arc::new(RecordingStore {
            pending: Mutex::new(vec![update]),
            backed_off: Mutex::new(Vec::new()),
            acknowledged: Mutex::new(Vec::new()),
            deferred_batches: Mutex::new(Vec::new()),
        });
        let use_case = DeliverPendingGroupUpdatesUseCase::new(
            store.clone(),
            dispatch_with([Err(GroupUpdateDispatchError::offline())]),
            retaining_all(&store),
            refresh_events().0,
            Arc::new(FixedClock),
        );

        let outcome = use_case
            .deliver_pending_group_updates(&MembershipMaintenanceTrigger::Periodic, &[])
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
            backed_off: Mutex::new(Vec::new()),
            acknowledged: Mutex::new(Vec::new()),
            deferred_batches: Mutex::new(Vec::new()),
        });
        let dispatch = dispatch_with((0..10).map(|_| Ok(())));
        let use_case = DeliverPendingGroupUpdatesUseCase::new(
            store.clone(),
            dispatch.clone(),
            retaining_all(&store),
            refresh_events().0,
            Arc::new(FixedClock),
        );

        let outcome = use_case
            .deliver_pending_group_updates(&MembershipMaintenanceTrigger::Periodic, &[])
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
            backed_off: Mutex::new(Vec::new()),
            acknowledged: Mutex::new(Vec::new()),
            deferred_batches: Mutex::new(Vec::new()),
        });
        let dispatch = dispatch_with(
            (0..MAX_UPDATES_PER_ROUND).map(|_| Err(GroupUpdateDispatchError::offline())),
        );
        let use_case = DeliverPendingGroupUpdatesUseCase::new(
            store.clone(),
            dispatch,
            retaining_all(&store),
            refresh_events().0,
            Arc::new(FixedClock),
        );

        assert_eq!(
            use_case
                .deliver_pending_group_updates(&MembershipMaintenanceTrigger::Periodic, &[])
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
                .deliver_pending_group_updates(&MembershipMaintenanceTrigger::Periodic, &[])
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
