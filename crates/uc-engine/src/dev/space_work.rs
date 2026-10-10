use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use tokio::sync::Notify;
use uc_application::deps::{
    AuthenticatedAdmissionExchangePort, AuthenticatedAdmissionReply, SpaceAdmissionTransportError,
    SpaceAdmissionTransportPort,
};
use uc_core::ids::DeviceId;
use uc_core::membership::{
    AdmissionAttemptTimeline, AdmissionContinuationCredential,
    AdmissionEncryptedPasswordEquivalent, AdmissionPeerBinding, GroupEpoch, GroupRevocationPort,
    GroupRevocationResult, GroupUpdateDispatchError, GroupUpdateDispatchPort,
    GroupUpdateRecipientDelivery, KeyEpochError, MembershipHistoryExchangeError,
    MembershipHistoryExchangePort, MembershipHistoryMessage, PendingGroupUpdate, RevocationId,
    SpaceAdmissionEnvelopeV1, SpaceAdmissionId, SpaceAdmissionMessageKind, SpaceAdmissionRoute,
};
use uc_observability_contract::diagnostics::connectivity::{
    ClassifiedGroupUpdateStorageFailure, GroupUpdateReason,
};

use super::{
    DevGroupUpdateReceiveFault, DevMembershipHistoryFailure, DevSpaceWorkEvent,
    DevSpaceWorkEventKind,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FinalConfirmationFailureState {
    Idle,
    ConnectionFailureArmed,
    SuccessReplyDropArmed,
    FailNextConnection,
    DropNextSuccessReply,
    AwaitingRetry,
    RetryInFlight,
}

struct State {
    final_confirmation: FinalConfirmationFailureState,
    membership_history_failure: Option<MembershipHistoryFailurePlan>,
    group_update_receive_fault: Option<DevGroupUpdateReceiveFault>,
    group_update_tamper_remaining: usize,
    next_sequence: u64,
    events: Vec<DevSpaceWorkEvent>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            final_confirmation: FinalConfirmationFailureState::Idle,
            membership_history_failure: None,
            group_update_receive_fault: None,
            group_update_tamper_remaining: 0,
            next_sequence: 1,
            events: Vec::new(),
        }
    }
}

/// 只在 `dev-tools` 构建中存在的单进程配对验收控制器。
///
/// 它不保存业务标识或消息材料，只记录测试关心的动作种类和严格递增序号。
#[derive(Default)]
pub(crate) struct SpaceWorkTestControl {
    state: Mutex<State>,
    changed: Notify,
}

impl SpaceWorkTestControl {
    pub(crate) fn arm_membership_history_failures(
        &self,
        failure: DevMembershipHistoryFailure,
        count: usize,
    ) -> Option<u64> {
        let mut state = self.lock_state();
        if count == 0 || state.membership_history_failure.is_some() {
            return None;
        }
        state.membership_history_failure = Some(MembershipHistoryFailurePlan {
            failure,
            remaining: count,
        });
        Some(state.next_sequence.saturating_sub(1))
    }

    pub(crate) fn set_group_update_receive_fault(&self, fault: Option<DevGroupUpdateReceiveFault>) {
        self.lock_state().group_update_receive_fault = fault;
    }

    pub(crate) fn arm_group_update_tamper(&self, count: usize) {
        self.lock_state().group_update_tamper_remaining = count;
    }

    fn group_update_receive_fault(&self) -> Option<DevGroupUpdateReceiveFault> {
        self.lock_state().group_update_receive_fault
    }

    fn take_group_update_tamper(&self) -> bool {
        let mut state = self.lock_state();
        if state.group_update_tamper_remaining == 0 {
            return false;
        }
        state.group_update_tamper_remaining -= 1;
        true
    }

    pub(crate) fn clear_membership_history_failures(&self) -> usize {
        let mut state = self.lock_state();
        state
            .membership_history_failure
            .take()
            .map_or(0, |plan| plan.remaining)
    }

    fn take_membership_history_failure(&self) -> Option<DevMembershipHistoryFailure> {
        let mut state = self.lock_state();
        let plan = state.membership_history_failure.as_mut()?;
        let failure = plan.failure;
        plan.remaining = plan.remaining.saturating_sub(1);
        if plan.remaining == 0 {
            state.membership_history_failure = None;
        }
        Some(failure)
    }

    pub(crate) fn arm_final_confirmation_connection_failure(&self) -> Option<u64> {
        let mut state = self.lock_state();
        if state.final_confirmation != FinalConfirmationFailureState::Idle {
            return None;
        }
        state.final_confirmation = FinalConfirmationFailureState::ConnectionFailureArmed;
        Some(state.next_sequence.saturating_sub(1))
    }

    pub(crate) fn arm_final_confirmation_success_reply_drop(&self) -> Option<u64> {
        let mut state = self.lock_state();
        if state.final_confirmation != FinalConfirmationFailureState::Idle {
            return None;
        }
        state.final_confirmation = FinalConfirmationFailureState::SuccessReplyDropArmed;
        Some(state.next_sequence.saturating_sub(1))
    }

    pub(crate) fn final_confirmation_ready(&self) {
        let mut state = self.lock_state();
        state.final_confirmation = match state.final_confirmation {
            FinalConfirmationFailureState::ConnectionFailureArmed => {
                FinalConfirmationFailureState::FailNextConnection
            }
            FinalConfirmationFailureState::SuccessReplyDropArmed => {
                FinalConfirmationFailureState::DropNextSuccessReply
            }
            current => current,
        };
    }

    pub(crate) fn begin_continuation_connection(&self) -> ContinuationTestAction {
        let mut state = self.lock_state();
        match state.final_confirmation {
            FinalConfirmationFailureState::FailNextConnection => {
                push_event(
                    &mut state,
                    DevSpaceWorkEventKind::FinalConfirmationConnectionFailed,
                );
                state.final_confirmation = FinalConfirmationFailureState::AwaitingRetry;
                drop(state);
                self.changed.notify_waiters();
                ContinuationTestAction::Fail
            }
            FinalConfirmationFailureState::AwaitingRetry => {
                push_event(
                    &mut state,
                    DevSpaceWorkEventKind::FinalConfirmationRetryStarted,
                );
                state.final_confirmation = FinalConfirmationFailureState::RetryInFlight;
                drop(state);
                self.changed.notify_waiters();
                ContinuationTestAction::TrackRetry
            }
            FinalConfirmationFailureState::DropNextSuccessReply => {
                ContinuationTestAction::DropSuccessReply
            }
            _ => ContinuationTestAction::PassThrough,
        }
    }

    /// 只有邀请方确实提交了最终确认，才丢弃其成功回复并用掉这次安排；连接或交换失败时保持安排，
    /// 由下一次连接继续执行。
    fn drop_success_reply(&self, kind: SpaceAdmissionMessageKind, succeeded: bool) -> bool {
        let mut state = self.lock_state();
        if state.final_confirmation != FinalConfirmationFailureState::DropNextSuccessReply
            || kind != SpaceAdmissionMessageKind::CompleteAck
            || !succeeded
        {
            return false;
        }
        state.final_confirmation = FinalConfirmationFailureState::AwaitingRetry;
        push_event(
            &mut state,
            DevSpaceWorkEventKind::FinalConfirmationSponsorCommitted,
        );
        push_event(
            &mut state,
            DevSpaceWorkEventKind::FinalConfirmationSuccessReplyDropped,
        );
        drop(state);
        self.changed.notify_waiters();
        true
    }

    pub(crate) fn retry_connection_failed(&self) {
        let mut state = self.lock_state();
        if state.final_confirmation == FinalConfirmationFailureState::RetryInFlight {
            state.final_confirmation = FinalConfirmationFailureState::AwaitingRetry;
        }
    }

    pub(crate) fn retry_exchange_finished(&self, kind: SpaceAdmissionMessageKind, succeeded: bool) {
        let mut state = self.lock_state();
        if state.final_confirmation != FinalConfirmationFailureState::RetryInFlight {
            return;
        }
        if kind == SpaceAdmissionMessageKind::CompleteAck && succeeded {
            push_event(
                &mut state,
                DevSpaceWorkEventKind::FinalConfirmationReplyReceived,
            );
            state.final_confirmation = FinalConfirmationFailureState::Idle;
            drop(state);
            self.changed.notify_waiters();
        } else {
            state.final_confirmation = FinalConfirmationFailureState::AwaitingRetry;
        }
    }

    pub(crate) fn record(&self, kind: DevSpaceWorkEventKind) {
        let mut state = self.lock_state();
        push_event(&mut state, kind);
        drop(state);
        self.changed.notify_waiters();
    }

    pub(crate) fn events(&self) -> Vec<DevSpaceWorkEvent> {
        let state = self.lock_state();
        let mut events = Vec::with_capacity(state.events.len());
        events.extend(state.events.iter().cloned());
        events
    }

    pub(crate) async fn wait_for_event(
        &self,
        after_sequence: u64,
        kind: DevSpaceWorkEventKind,
    ) -> DevSpaceWorkEvent {
        loop {
            let notified = self.changed.notified();
            if let Some(event) = self.lock_state().events.iter().find_map(|event| {
                (event.sequence > after_sequence && event.kind == kind).then(|| event.clone())
            }) {
                return event;
            }
            notified.await;
        }
    }

    fn lock_state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[derive(Clone, Copy)]
struct MembershipHistoryFailurePlan {
    failure: DevMembershipHistoryFailure,
    remaining: usize,
}

fn push_event(state: &mut State, kind: DevSpaceWorkEventKind) {
    let sequence = state.next_sequence;
    state.next_sequence = state.next_sequence.saturating_add(1);
    state.events.push(DevSpaceWorkEvent { sequence, kind });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContinuationTestAction {
    PassThrough,
    Fail,
    TrackRetry,
    DropSuccessReply,
}

pub(crate) struct ControlledSpaceAdmissionTransport {
    inner: Arc<dyn SpaceAdmissionTransportPort>,
    control: Arc<SpaceWorkTestControl>,
}

impl ControlledSpaceAdmissionTransport {
    pub(crate) fn new(
        inner: Arc<dyn SpaceAdmissionTransportPort>,
        control: Arc<SpaceWorkTestControl>,
    ) -> Self {
        Self { inner, control }
    }
}

#[async_trait]
impl SpaceAdmissionTransportPort for ControlledSpaceAdmissionTransport {
    async fn establish_initial(
        &self,
        admission_id: SpaceAdmissionId,
        attempt_timeline: AdmissionAttemptTimeline,
        route: &SpaceAdmissionRoute,
        encrypted_password_equivalent: &AdmissionEncryptedPasswordEquivalent,
    ) -> Result<Box<dyn AuthenticatedAdmissionExchangePort>, SpaceAdmissionTransportError> {
        self.inner
            .establish_initial(
                admission_id,
                attempt_timeline,
                route,
                encrypted_password_equivalent,
            )
            .await
    }

    async fn resume(
        &self,
        admission_id: SpaceAdmissionId,
        route: &SpaceAdmissionRoute,
        peer_binding: AdmissionPeerBinding,
        continuation_credential: &AdmissionContinuationCredential,
    ) -> Result<Box<dyn AuthenticatedAdmissionExchangePort>, SpaceAdmissionTransportError> {
        let action = self.control.begin_continuation_connection();
        if action == ContinuationTestAction::Fail {
            return Err(SpaceAdmissionTransportError::deferred());
        }
        let exchange = self
            .inner
            .resume(admission_id, route, peer_binding, continuation_credential)
            .await;
        match exchange {
            Ok(exchange)
                if matches!(
                    action,
                    ContinuationTestAction::TrackRetry | ContinuationTestAction::DropSuccessReply
                ) =>
            {
                Ok(Box::new(TrackedAdmissionExchange {
                    inner: exchange,
                    control: Arc::clone(&self.control),
                    action,
                }))
            }
            Ok(exchange) => Ok(exchange),
            Err(error) => {
                if action == ContinuationTestAction::TrackRetry {
                    self.control.retry_connection_failed();
                }
                Err(error)
            }
        }
    }
}

struct TrackedAdmissionExchange {
    inner: Box<dyn AuthenticatedAdmissionExchangePort>,
    control: Arc<SpaceWorkTestControl>,
    action: ContinuationTestAction,
}

#[async_trait]
impl AuthenticatedAdmissionExchangePort for TrackedAdmissionExchange {
    fn peer_binding(&self) -> AdmissionPeerBinding {
        self.inner.peer_binding()
    }

    fn take_newly_established_continuation(&mut self) -> Option<AdmissionContinuationCredential> {
        self.inner.take_newly_established_continuation()
    }

    async fn exchange(
        self: Box<Self>,
        request: &SpaceAdmissionEnvelopeV1,
    ) -> Result<AuthenticatedAdmissionReply, SpaceAdmissionTransportError> {
        let kind = request.kind();
        let TrackedAdmissionExchange {
            inner,
            control,
            action,
        } = *self;
        let result = inner.exchange(request).await;
        if action == ContinuationTestAction::DropSuccessReply
            && control.drop_success_reply(kind, result.is_ok())
        {
            return Err(SpaceAdmissionTransportError::deferred());
        }
        if action == ContinuationTestAction::TrackRetry {
            control.retry_exchange_finished(kind, result.is_ok());
        }
        result
    }
}

pub(crate) struct RecordedGroupUpdateDispatch {
    inner: Arc<dyn GroupUpdateDispatchPort>,
    control: Arc<SpaceWorkTestControl>,
}

impl RecordedGroupUpdateDispatch {
    pub(crate) fn new(
        inner: Arc<dyn GroupUpdateDispatchPort>,
        control: Arc<SpaceWorkTestControl>,
    ) -> Self {
        Self { inner, control }
    }
}

#[async_trait]
impl GroupUpdateDispatchPort for RecordedGroupUpdateDispatch {
    async fn dispatch_group_update(
        &self,
        update: &PendingGroupUpdate,
    ) -> Result<(), GroupUpdateDispatchError> {
        self.control
            .record(DevSpaceWorkEventKind::OrdinaryMemberUpdateStarted);
        if self.control.take_group_update_tamper() {
            if let Some(tampered) = tampered_update(update) {
                self.control
                    .record(DevSpaceWorkEventKind::GroupUpdateTampered);
                return self.inner.dispatch_group_update(&tampered).await;
            }
        }
        self.inner.dispatch_group_update(update).await
    }
}

/// 只改线上副本的 MLS 提交字节：接收方的真实校验会拒绝它，保存的原件不变。
fn tampered_update(update: &PendingGroupUpdate) -> Option<PendingGroupUpdate> {
    let mut body: serde_json::Value = serde_json::from_slice(update.payload()).ok()?;
    let commit = body.get_mut("commit")?.as_array_mut()?;
    let last = commit.last_mut()?;
    *last = serde_json::Value::from(last.as_u64()? ^ 0xff);
    let payload = serde_json::to_vec(&body).ok()?;
    let mut record = serde_json::to_value(update).ok()?;
    record["payload"] = serde_json::to_value(payload).ok()?;
    serde_json::from_value(record).ok()
}

/// 包装接收方使用的组密钥端口：只在 `apply_group_epoch_update` 注入本地失败，其余原样转交。
pub(crate) struct FaultableGroupUpdateReceiver {
    inner: Arc<dyn GroupRevocationPort>,
    control: Arc<SpaceWorkTestControl>,
}

impl FaultableGroupUpdateReceiver {
    pub(crate) fn new(
        inner: Arc<dyn GroupRevocationPort>,
        control: Arc<SpaceWorkTestControl>,
    ) -> Self {
        Self { inner, control }
    }
}

#[async_trait]
impl GroupRevocationPort for FaultableGroupUpdateReceiver {
    async fn revoke_group_member(
        &self,
        target: &DeviceId,
        retained_recipients: &[DeviceId],
        now_ms: i64,
    ) -> Result<GroupRevocationResult, KeyEpochError> {
        self.inner
            .revoke_group_member(target, retained_recipients, now_ms)
            .await
    }

    async fn acknowledge_group_update(
        &self,
        revocation_id: &RevocationId,
        recipient: &DeviceId,
        now_ms: i64,
    ) -> Result<GroupRevocationResult, KeyEpochError> {
        self.inner
            .acknowledge_group_update(revocation_id, recipient, now_ms)
            .await
    }

    async fn apply_group_epoch_update(&self, payload: &[u8]) -> Result<GroupEpoch, KeyEpochError> {
        if let Some(fault) = self.control.group_update_receive_fault() {
            self.control
                .record(DevSpaceWorkEventKind::GroupUpdateReceiveFaultInjected);
            return Err(match fault {
                DevGroupUpdateReceiveFault::NotReady => KeyEpochError::SpaceNotReady,
                DevGroupUpdateReceiveFault::StorageUnavailable => KeyEpochError::Repository(
                    anyhow::Error::new(ClassifiedGroupUpdateStorageFailure::new(
                        GroupUpdateReason::Unavailable,
                        std::io::Error::other("injected storage failure"),
                    )),
                ),
            });
        }
        let applied = self.inner.apply_group_epoch_update(payload).await;
        if applied.is_ok() {
            self.control
                .record(DevSpaceWorkEventKind::GroupUpdateApplied);
        }
        applied
    }

    async fn pending_group_updates(
        &self,
        revocation_id: &RevocationId,
    ) -> Result<Vec<PendingGroupUpdate>, KeyEpochError> {
        self.inner.pending_group_updates(revocation_id).await
    }

    async fn query_group_revocation(
        &self,
        revocation_id: &RevocationId,
    ) -> Result<Option<GroupRevocationResult>, KeyEpochError> {
        self.inner.query_group_revocation(revocation_id).await
    }

    async fn current_group_revocation(
        &self,
    ) -> Result<Option<GroupRevocationResult>, KeyEpochError> {
        self.inner.current_group_revocation().await
    }

    async fn continue_group_revocation(
        &self,
        revocation_id: &RevocationId,
        permanently_lost_device_ids: &[DeviceId],
        now_ms: i64,
    ) -> Result<GroupRevocationResult, KeyEpochError> {
        self.inner
            .continue_group_revocation(revocation_id, permanently_lost_device_ids, now_ms)
            .await
    }

    async fn resume_group_revocations(
        &self,
        now_ms: i64,
    ) -> Result<Vec<GroupRevocationResult>, KeyEpochError> {
        self.inner.resume_group_revocations(now_ms).await
    }

    async fn due_space_group_updates(
        &self,
        now_ms: i64,
        online_peer: Option<DeviceId>,
    ) -> Result<Vec<PendingGroupUpdate>, KeyEpochError> {
        self.inner
            .due_space_group_updates(now_ms, online_peer)
            .await
    }

    async fn record_space_group_update_failures(
        &self,
        failures: &[(String, GroupUpdateDispatchError)],
        now_ms: i64,
    ) -> Result<usize, KeyEpochError> {
        self.inner
            .record_space_group_update_failures(failures, now_ms)
            .await
    }

    async fn space_group_update_deliveries(
        &self,
    ) -> Result<Vec<GroupUpdateRecipientDelivery>, KeyEpochError> {
        self.inner.space_group_update_deliveries().await
    }

    async fn acknowledge_space_group_update(
        &self,
        update_id: &str,
        now_ms: i64,
    ) -> Result<bool, KeyEpochError> {
        self.inner
            .acknowledge_space_group_update(update_id, now_ms)
            .await
    }

    async fn settle_obsolete_space_group_updates(
        &self,
        retained_recipients: &[DeviceId],
        now_ms: i64,
    ) -> Result<usize, KeyEpochError> {
        self.inner
            .settle_obsolete_space_group_updates(retained_recipients, now_ms)
            .await
    }
}

pub(crate) struct RecordedMembershipHistoryExchange {
    inner: Arc<dyn MembershipHistoryExchangePort>,
    control: Arc<SpaceWorkTestControl>,
}

impl RecordedMembershipHistoryExchange {
    pub(crate) fn new(
        inner: Arc<dyn MembershipHistoryExchangePort>,
        control: Arc<SpaceWorkTestControl>,
    ) -> Self {
        Self { inner, control }
    }
}

#[async_trait]
impl MembershipHistoryExchangePort for RecordedMembershipHistoryExchange {
    async fn exchange_membership_history(
        &self,
        recipient: &DeviceId,
        message: MembershipHistoryMessage,
    ) -> Result<MembershipHistoryMessage, MembershipHistoryExchangeError> {
        self.control
            .record(DevSpaceWorkEventKind::MembershipHistorySyncStarted);
        if let Some(failure) = self.control.take_membership_history_failure() {
            return match failure {
                DevMembershipHistoryFailure::Retryable => {
                    self.control
                        .record(DevSpaceWorkEventKind::MembershipHistorySyncRetryableFailure);
                    Err(MembershipHistoryExchangeError::offline())
                }
                DevMembershipHistoryFailure::NeedsAttention => {
                    self.control
                        .record(DevSpaceWorkEventKind::MembershipHistorySyncNeedsAttention);
                    Err(MembershipHistoryExchangeError::Rejected)
                }
            };
        }
        let result = self
            .inner
            .exchange_membership_history(recipient, message)
            .await;
        if result.is_ok() {
            self.control
                .record(DevSpaceWorkEventKind::MembershipHistorySyncReplyReceived);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn one_failure_is_followed_by_an_observable_retry() {
        let control = SpaceWorkTestControl::default();
        let baseline = control
            .arm_final_confirmation_connection_failure()
            .expect("test failure can be armed");
        control.final_confirmation_ready();

        assert_eq!(
            control.begin_continuation_connection(),
            ContinuationTestAction::Fail
        );
        assert_eq!(
            control.begin_continuation_connection(),
            ContinuationTestAction::TrackRetry
        );
        control.retry_exchange_finished(SpaceAdmissionMessageKind::CompleteAck, true);

        let events = control.events();
        assert_eq!(
            events
                .iter()
                .filter(|event| {
                    event.kind == DevSpaceWorkEventKind::FinalConfirmationConnectionFailed
                })
                .count(),
            1
        );
        assert_eq!(
            events.iter().map(|event| event.kind).collect::<Vec<_>>(),
            vec![
                DevSpaceWorkEventKind::FinalConfirmationConnectionFailed,
                DevSpaceWorkEventKind::FinalConfirmationRetryStarted,
                DevSpaceWorkEventKind::FinalConfirmationReplyReceived,
            ]
        );
        assert_eq!(
            control
                .wait_for_event(
                    baseline,
                    DevSpaceWorkEventKind::FinalConfirmationReplyReceived,
                )
                .await,
            events[2]
        );
    }

    #[test]
    fn failed_attempts_keep_the_success_reply_drop_armed() {
        let control = SpaceWorkTestControl::default();
        control
            .arm_final_confirmation_success_reply_drop()
            .expect("test reply drop can be armed");
        control.final_confirmation_ready();

        // 连接被拒时交换从未发生，安排留给下一次连接。
        assert_eq!(
            control.begin_continuation_connection(),
            ContinuationTestAction::DropSuccessReply
        );
        assert_eq!(
            control.begin_continuation_connection(),
            ContinuationTestAction::DropSuccessReply
        );
        assert!(!control.drop_success_reply(SpaceAdmissionMessageKind::CompleteAck, false));
        assert_eq!(
            control.begin_continuation_connection(),
            ContinuationTestAction::DropSuccessReply
        );
        assert!(control.drop_success_reply(SpaceAdmissionMessageKind::CompleteAck, true));
        assert_eq!(
            control.begin_continuation_connection(),
            ContinuationTestAction::TrackRetry
        );
    }

    #[tokio::test]
    async fn one_success_reply_is_dropped_only_after_the_sponsor_commit() {
        let control = SpaceWorkTestControl::default();
        let baseline = control
            .arm_final_confirmation_success_reply_drop()
            .expect("test reply drop can be armed");
        control.final_confirmation_ready();

        assert_eq!(
            control.begin_continuation_connection(),
            ContinuationTestAction::DropSuccessReply
        );
        assert!(control.drop_success_reply(SpaceAdmissionMessageKind::CompleteAck, true));
        assert_eq!(
            control.begin_continuation_connection(),
            ContinuationTestAction::TrackRetry
        );
        control.retry_exchange_finished(SpaceAdmissionMessageKind::CompleteAck, true);

        let events = control.events();
        assert_eq!(
            events.iter().map(|event| event.kind).collect::<Vec<_>>(),
            vec![
                DevSpaceWorkEventKind::FinalConfirmationSponsorCommitted,
                DevSpaceWorkEventKind::FinalConfirmationSuccessReplyDropped,
                DevSpaceWorkEventKind::FinalConfirmationRetryStarted,
                DevSpaceWorkEventKind::FinalConfirmationReplyReceived,
            ]
        );
        assert_eq!(
            control
                .wait_for_event(
                    baseline,
                    DevSpaceWorkEventKind::FinalConfirmationSuccessReplyDropped,
                )
                .await,
            events[1]
        );
    }

    #[test]
    fn failure_cannot_be_armed_twice() {
        let control = SpaceWorkTestControl::default();
        assert!(control
            .arm_final_confirmation_connection_failure()
            .is_some());
        assert!(control
            .arm_final_confirmation_connection_failure()
            .is_none());
    }

    #[test]
    fn membership_history_failures_are_counted_and_then_recover() {
        let control = SpaceWorkTestControl::default();
        let baseline = control
            .arm_membership_history_failures(DevMembershipHistoryFailure::Retryable, 2)
            .expect("test failures can be armed");

        assert_eq!(
            control.take_membership_history_failure(),
            Some(DevMembershipHistoryFailure::Retryable)
        );
        assert!(control
            .arm_membership_history_failures(DevMembershipHistoryFailure::NeedsAttention, 1)
            .is_none());
        assert_eq!(
            control.take_membership_history_failure(),
            Some(DevMembershipHistoryFailure::Retryable)
        );
        assert_eq!(control.take_membership_history_failure(), None);
        assert_eq!(baseline, 0);
        assert!(control
            .arm_membership_history_failures(DevMembershipHistoryFailure::NeedsAttention, 1)
            .is_some());
        assert_eq!(control.clear_membership_history_failures(), 1);
        assert_eq!(control.clear_membership_history_failures(), 0);
    }

    #[test]
    fn membership_history_failure_count_must_be_positive() {
        let control = SpaceWorkTestControl::default();
        assert!(control
            .arm_membership_history_failures(DevMembershipHistoryFailure::Retryable, 0)
            .is_none());
    }
}
