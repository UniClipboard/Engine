use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use tokio::sync::Notify;
use uc_application::deps::{
    ApplyGroupUpdatePort, AuthenticatedAdmissionExchangePort, AuthenticatedAdmissionReply,
    ConnectionLease, ConnectionProtocol, ConnectivityError, GroupUpdateApplyError,
    GroupUpdateIncoming, GroupUpdateReceiveOutcome, GroupUpdateWirePort, PreparedGroupUpdate,
    SpaceAdmissionTransportError, SpaceAdmissionTransportPort, StreamLease,
    VerifiedHistoryGroupUpdate,
};
use uc_core::ids::DeviceId;
use uc_core::membership::{
    AdmissionAttemptTimeline, AdmissionContinuationCredential,
    AdmissionEncryptedPasswordEquivalent, AdmissionPeerBinding, GroupEpoch,
    GroupUpdateDispatchError, GroupUpdateDispatchPort, GroupUpdateRejection, KeyEpochError,
    KeyEpochStateIssue, MembershipHistoryExchangeEndpointPort, MembershipHistoryExchangeError,
    MembershipHistoryExchangePort, MembershipHistoryMessage, PendingGroupUpdate,
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
    history_held: bool,
    legacy_group_protocol: bool,
    history_held_peer: Option<DeviceId>,
    legacy_rejection_remaining: usize,
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
            history_held: false,
            legacy_group_protocol: false,
            history_held_peer: None,
            legacy_rejection_remaining: 0,
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

    pub(crate) fn set_legacy_group_protocol(&self, legacy: bool) {
        self.lock_state().legacy_group_protocol = legacy;
    }
    pub(crate) fn uses_legacy_group_protocol(&self) -> bool {
        self.lock_state().legacy_group_protocol
    }
    fn record_group_wire(&self, protocol: ConnectionProtocol, acknowledged: bool) {
        let kind = match (protocol, acknowledged) {
            (ConnectionProtocol::LegacyGroupUpdate, false) => {
                DevSpaceWorkEventKind::GroupUpdateLegacyWireSent
            }
            (ConnectionProtocol::GroupUpdate, false) => {
                DevSpaceWorkEventKind::GroupUpdateCurrentWireSent
            }
            (ConnectionProtocol::LegacyGroupUpdate, true) => {
                DevSpaceWorkEventKind::GroupUpdateLegacyWireAcknowledged
            }
            (ConnectionProtocol::GroupUpdate, true) => {
                DevSpaceWorkEventKind::GroupUpdateCurrentWireAcknowledged
            }
            _ => return,
        };
        self.record(kind);
    }
    pub(crate) fn set_history_held(&self, held: bool, peer: Option<DeviceId>) {
        let mut state = self.lock_state();
        state.history_held = held;
        state.history_held_peer = peer;
    }
    fn history_is_held(&self, peer: &DeviceId, message: &MembershipHistoryMessage) -> bool {
        let state = self.lock_state();
        state.history_held
            && state
                .history_held_peer
                .is_none_or(|held_peer| held_peer == *peer)
            && !matches!(
                message,
                MembershipHistoryMessage::SummaryV3(_) | MembershipHistoryMessage::AckV3(_)
            )
    }

    pub(crate) fn arm_group_update_tamper(&self, count: usize) {
        self.lock_state().group_update_tamper_remaining = count;
    }

    pub(crate) fn arm_legacy_receiver_rejection(&self, count: usize) {
        self.lock_state().legacy_rejection_remaining = count;
    }

    fn take_legacy_receiver_rejection(&self) -> bool {
        let mut state = self.lock_state();
        if state.legacy_rejection_remaining == 0 {
            return false;
        }
        state.legacy_rejection_remaining -= 1;
        true
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
        if self.control.take_legacy_receiver_rejection() {
            self.control
                .record(DevSpaceWorkEventKind::LegacyReceiverRejectionInjected);
            return Err(GroupUpdateDispatchError::Rejected(
                GroupUpdateRejection::Unclassified,
            ));
        }
        self.inner.dispatch_group_update(update).await
    }
}

/// 只改线上副本的 MLS 提交字节：接收方的真实校验会拒绝它，保存的原件不变。
fn tampered_payload(payload: &[u8]) -> Option<Vec<u8>> {
    let mut body: serde_json::Value = serde_json::from_slice(payload).ok()?;
    let commit = body.get_mut("commit")?.as_array_mut()?;
    let last = commit.last_mut()?;
    *last = serde_json::Value::from(last.as_u64()? ^ 0xff);
    serde_json::to_vec(&body).ok()
}

pub(crate) struct FaultableGroupUpdateWire {
    inner: Arc<dyn GroupUpdateWirePort>,
    control: Arc<SpaceWorkTestControl>,
}
impl FaultableGroupUpdateWire {
    pub(crate) fn new(
        inner: Arc<dyn GroupUpdateWirePort>,
        control: Arc<SpaceWorkTestControl>,
    ) -> Self {
        Self { inner, control }
    }
}
#[async_trait]
impl GroupUpdateWirePort for FaultableGroupUpdateWire {
    fn is_supported(&self, protocol: ConnectionProtocol) -> Result<(), ConnectivityError> {
        self.inner.is_supported(protocol)
    }
    async fn read(&self, stream: &mut StreamLease) -> Result<GroupUpdateIncoming, anyhow::Error> {
        self.inner.read(stream).await
    }
    async fn acknowledge(
        &self,
        stream: &mut StreamLease,
        protocol: ConnectionProtocol,
        outcome: &GroupUpdateReceiveOutcome,
    ) -> Result<bool, anyhow::Error> {
        let acknowledged = self.inner.acknowledge(stream, protocol, outcome).await?;
        if acknowledged {
            self.control.record_group_wire(protocol, true);
        }
        Ok(acknowledged)
    }
    async fn send(
        &self,
        stream: &mut StreamLease,
        protocol: ConnectionProtocol,
        payload: &[u8],
    ) -> Result<(), GroupUpdateDispatchError> {
        // 此端口只在 Uni 已授权并取得真实 stream 后调用，拨号失败不消费一次性篡改。
        if self.control.take_group_update_tamper() {
            if let Some(tampered) = tampered_payload(payload) {
                self.control
                    .record(DevSpaceWorkEventKind::GroupUpdateTampered);
                return self.inner.send(stream, protocol, &tampered).await;
            }
        }
        let result = self.inner.send(stream, protocol, payload).await;
        if result.is_ok() {
            self.control.record_group_wire(protocol, false);
        }
        result
    }
}

/// 在经过完整接收 owner 的直接入站应用注入失败，历史效果不消费接收故障。
pub(crate) struct FaultableGroupUpdateReceiver {
    inner: Arc<dyn ApplyGroupUpdatePort>,
    control: Arc<SpaceWorkTestControl>,
}

impl FaultableGroupUpdateReceiver {
    pub(crate) fn new(
        inner: Arc<dyn ApplyGroupUpdatePort>,
        control: Arc<SpaceWorkTestControl>,
    ) -> Self {
        Self { inner, control }
    }
}

#[async_trait]
impl ApplyGroupUpdatePort for FaultableGroupUpdateReceiver {
    async fn prepare_removal(
        &self,
        target: &DeviceId,
        retained: &[DeviceId],
        now_ms: i64,
    ) -> Result<Option<PreparedGroupUpdate>, anyhow::Error> {
        self.inner.prepare_removal(target, retained, now_ms).await
    }
    async fn apply(
        &self,
        payload: &[u8],
        proof: Option<&VerifiedHistoryGroupUpdate>,
        connection: Option<&ConnectionLease>,
    ) -> Result<GroupEpoch, GroupUpdateApplyError> {
        if connection.is_some() {
            if let Some(fault) = self.control.group_update_receive_fault() {
                self.control
                    .record(DevSpaceWorkEventKind::GroupUpdateReceiveFaultInjected);
                let source = match fault {
                    DevGroupUpdateReceiveFault::NotReady => KeyEpochError::SpaceNotReady,
                    DevGroupUpdateReceiveFault::StorageUnavailable => KeyEpochError::Repository(
                        anyhow::Error::new(ClassifiedGroupUpdateStorageFailure::new(
                            GroupUpdateReason::Unavailable,
                            std::io::Error::other("injected storage failure"),
                        )),
                    ),
                };
                return Err(GroupUpdateApplyError::Unavailable(source));
            }
        }
        let result = self.inner.apply(payload, proof, connection).await;
        if connection.is_some()
            && matches!(
                &result,
                Err(GroupUpdateApplyError::Unavailable(
                    KeyEpochError::StateIssue(KeyEpochStateIssue::MissingPredecessor)
                ))
            )
        {
            self.control
                .record(DevSpaceWorkEventKind::GroupUpdateMissingPredecessorObserved);
        }
        if result.is_ok() && connection.is_some() {
            self.control
                .record(DevSpaceWorkEventKind::GroupUpdateApplied);
        }
        result
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
        if self.control.history_is_held(recipient, &message) {
            return Err(MembershipHistoryExchangeError::offline());
        }
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

pub(crate) struct GatedMembershipHistoryEndpoint {
    inner: Arc<dyn MembershipHistoryExchangeEndpointPort>,
    control: Arc<SpaceWorkTestControl>,
}
impl GatedMembershipHistoryEndpoint {
    pub(crate) fn new(
        inner: Arc<dyn MembershipHistoryExchangeEndpointPort>,
        control: Arc<SpaceWorkTestControl>,
    ) -> Self {
        Self { inner, control }
    }
}
#[async_trait]
impl MembershipHistoryExchangeEndpointPort for GatedMembershipHistoryEndpoint {
    async fn handle_membership_history_exchange(
        &self,
        peer: &DeviceId,
        message: MembershipHistoryMessage,
    ) -> Result<MembershipHistoryMessage, MembershipHistoryExchangeError> {
        if self.control.history_is_held(peer, &message) {
            return Err(MembershipHistoryExchangeError::offline());
        }
        self.inner
            .handle_membership_history_exchange(peer, message)
            .await
    }
}
