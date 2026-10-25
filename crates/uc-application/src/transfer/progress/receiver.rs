//! 单个发送意图的完整登记、活动与终态采用负责人；网络等待不持有投影锁。
use super::clock::{Clock, Sample};
use super::{
    CurrentProgressTaskPort, ElapsedClockError, ElapsedClockPort, ProgressBound,
    ProgressContactNotice, ProgressProjectionSnapshot, ProgressReceiveError, ProgressReceiverPort,
    ProgressTaskDescriptor, ProgressTerminalNotice, ProgressUpdateNotice,
};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::Notify;
use uc_core::file_transfer::{
    summarize_progress, valid_progress_plan, ProgressFreshness, ProgressFreshnessEffect,
    ProgressFreshnessInput, ProgressFreshnessView, ProgressIdentity, ProgressItem,
    ProgressItemEffect, ProgressItemInput, ProgressItemProjection, ProgressItemState,
    ProgressRegistration, RegistrationEffect, RegistrationInput,
};
use uc_core::ids::DeviceId;
use uuid::Uuid;

pub struct TransferProgressReceiver {
    peer: DeviceId,
    source: ProgressIdentity,
    query_intent: ProgressIdentity,
    state: Mutex<State>,
    clock: Arc<Clock>,
}
struct State {
    closed: bool,
    plan: Vec<ProgressItem>,
    generation: u64,
    registration: ProgressRegistration<ProgressIdentity>,
    bound: Option<ProgressBound>,
    queried_scope: Option<ProgressTaskDescriptor>,
    snapshot_revision: Option<u64>,
    snapshot_content: Option<Vec<ProgressItemState>>,
    items: HashMap<ProgressItem, ProgressItemProjection>,
    version: u64,
    freshness: ProgressFreshness,
    epoch: Option<u64>,
    clock_fault: Option<Arc<ElapsedClockError>>,
    view: ProgressFreshnessView,
}
impl TransferProgressReceiver {
    /// 由原始发送意图创建；身份与逻辑计划不由第一条远端进度授权。
    pub fn new(
        peer: DeviceId,
        source: ProgressIdentity,
        plan: Vec<ProgressItem>,
        clock: Arc<dyn ElapsedClockPort>,
    ) -> Result<Self, ProgressReceiveError> {
        if !valid_progress_plan(&plan) {
            return Err(ProgressReceiveError::InvalidPlan);
        }
        let clock = Arc::new(Clock::new(clock, Arc::new(Notify::new())));
        let sample = clock.sample();
        let (accepted, epoch, clock_fault) = match sample {
            Ok(sample) => (Some(sample.now), Some(sample.epoch), None),
            Err(source) => (None, None, Some(Arc::new(source))),
        };
        let freshness = ProgressFreshness::new(accepted);
        let view = freshness.view(accepted);
        Ok(Self {
            clock,
            peer,
            source,
            query_intent: ProgressIdentity::new(*Uuid::new_v4().as_bytes()),
            state: Mutex::new(State {
                plan,
                closed: false,
                generation: 0,
                registration: ProgressRegistration::new(source),
                bound: None,
                queried_scope: None,
                snapshot_revision: None,
                snapshot_content: None,
                items: HashMap::new(),
                version: 0,
                freshness,
                epoch,
                clock_fault,
                view,
            }),
        })
    }
    pub(super) fn matches_plan(&self, plan: &[ProgressItem]) -> bool {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        !state.closed && state.plan == plan
    }
    pub(super) fn retire(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.closed = true;
        state.plan.clear();
        state.plan.shrink_to_fit();
        state.items.clear();
        state.items.shrink_to_fit();
        state.bound = None;
        state.queried_scope = None;
        state.snapshot_revision = None;
        state.snapshot_content = None;
        state
            .freshness
            .apply(ProgressFreshnessInput::ContinuityLost);
    }
    pub(super) fn needs_tick(&self) -> bool {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        !state.closed
            && matches!(
                state.view,
                ProgressFreshnessView::Waiting | ProgressFreshnessView::Live
            )
    }
    pub(super) fn revision(&self) -> u64 {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        refresh_view(&self.clock, &mut state);
        state.version
    }
    pub fn snapshot(&self) -> ProgressProjectionSnapshot {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let view = refresh_view(&self.clock, &mut state);
        let freshness = if view == ProgressFreshnessView::Settled {
            Ok(view)
        } else {
            match &state.clock_fault {
                Some(source) => Err(source.clone()),
                None => Ok(view),
            }
        };
        let items: Vec<_> = state
            .plan
            .iter()
            .map(|item| {
                (
                    *item,
                    state
                        .items
                        .get(item)
                        .and_then(ProgressItemProjection::state),
                )
            })
            .collect();
        let summary = summarize_progress(items.iter().map(|(_, value)| *value), view);
        ProgressProjectionSnapshot {
            freshness,
            summary,
            version: state.version,
            bound: state.bound,
            task: state.task_descriptor(),
            items,
        }
    }
}
#[async_trait]
impl ProgressReceiverPort for TransferProgressReceiver {
    async fn adopt_contact(
        &self,
        peer: &DeviceId,
        generation: u64,
        notice: ProgressContactNotice,
    ) -> Result<(), ProgressReceiveError> {
        if peer != &self.peer || notice.source != self.source {
            return Err(ProgressReceiveError::UnknownTask);
        }
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.closed {
            return Err(ProgressReceiveError::UnknownTask);
        }
        if state.registration.apply(RegistrationInput::Notification {
            generation,
            binding: notice.binding,
            source: notice.source,
            attempt: notice.attempt,
            revision: notice.revision,
        }) != RegistrationEffect::CheckNotification
        {
            return Err(ProgressReceiveError::Stale);
        }
        if state.freshness.view(None) == ProgressFreshnessView::Settled {
            return Err(ProgressReceiveError::Stale);
        }
        let now = read_clock(&self.clock, &mut state);
        // 无时钟仍消费已验证的新联系序号，但保持未知，恢复不能令重复消息续期。
        let effect = state.freshness.apply(ProgressFreshnessInput::Contact {
            sequence: notice.sequence,
            now: now.unwrap_or(Duration::ZERO),
        });
        if now.is_none() {
            state
                .freshness
                .apply(ProgressFreshnessInput::ContinuityLost);
        } else if effect == ProgressFreshnessEffect::Advanced {
            state.clock_fault = None;
        }
        let view = state.freshness.view(now);
        if state.view != view {
            state.view = view;
            state.version = state.version.saturating_add(1);
        }
        Ok(())
    }

    async fn adopt_update(
        &self,
        peer: &DeviceId,
        generation: u64,
        notice: ProgressUpdateNotice,
    ) -> Result<(), ProgressReceiveError> {
        if peer != &self.peer || notice.source != self.source {
            return Err(ProgressReceiveError::UnknownTask);
        }
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.closed {
            return Err(ProgressReceiveError::UnknownTask);
        }
        if !state.plan.contains(&notice.update.item) {
            return Err(ProgressReceiveError::InvalidPlan);
        }
        if state.registration.apply(RegistrationInput::Notification {
            generation,
            binding: notice.binding,
            source: notice.source,
            attempt: notice.attempt,
            revision: notice.revision,
        }) != RegistrationEffect::CheckNotification
        {
            return Err(ProgressReceiveError::Stale);
        }
        match state
            .items
            .entry(notice.update.item)
            .or_default()
            .apply(ProgressItemInput::Update(notice.update))
        {
            ProgressItemEffect::Adopted => {
                fresh_notification(&self.clock, &mut state);
                state.version = state.version.saturating_add(1);
            }
            ProgressItemEffect::Duplicate => {}
            ProgressItemEffect::Stale => return Err(ProgressReceiveError::Stale),
            ProgressItemEffect::Conflict => return Err(ProgressReceiveError::Conflict),
            ProgressItemEffect::Invalid => return Err(ProgressReceiveError::InvalidPlan),
        }
        Ok(())
    }
    async fn register(
        &self,
        peer: &DeviceId,
        generation: u64,
        source: ProgressIdentity,
        exchange: &mut dyn CurrentProgressTaskPort,
    ) -> Result<ProgressBound, ProgressReceiveError> {
        if peer != &self.peer || source != self.source {
            return Err(ProgressReceiveError::UnknownTask);
        }
        let nonce = ProgressIdentity::new(*Uuid::new_v4().as_bytes());
        {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            if state.closed {
                return Err(ProgressReceiveError::UnknownTask);
            }
            if generation > state.generation {
                if state
                    .registration
                    .apply(RegistrationInput::ReplaceConnection { generation })
                    != RegistrationEffect::ConnectionReplaced
                {
                    return Err(ProgressReceiveError::Stale);
                }
                state.generation = generation;
            }
            if state
                .registration
                .apply(RegistrationInput::Challenge { generation, nonce })
                != RegistrationEffect::SendChallenge
            {
                return Err(ProgressReceiveError::Stale);
            }
        }
        let descriptor = exchange.challenge(nonce).await?;
        let binding = ProgressIdentity::new(*Uuid::new_v4().as_bytes());
        let bound = ProgressBound {
            challenge: nonce,
            binding,
            descriptor,
        };
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.closed {
            return Err(ProgressReceiveError::UnknownTask);
        }
        if usize::from(descriptor.item_count) != state.plan.len() {
            return Err(ProgressReceiveError::InvalidPlan);
        }
        let previous = state.task_descriptor();
        if state.registration.apply(RegistrationInput::Describe {
            generation,
            nonce,
            binding,
            descriptor,
        }) != RegistrationEffect::SendBound
        {
            return Err(ProgressReceiveError::Stale);
        }
        if previous != Some(descriptor) {
            state.snapshot_revision = None;
            state.snapshot_content = None;
        }
        if previous.is_none_or(|previous| previous.attempt != descriptor.attempt) {
            state.items.clear();
            state.freshness.apply(ProgressFreshnessInput::NewAttempt);
        }
        if previous.is_some_and(|previous| previous.producer != descriptor.producer) {
            state.freshness.apply(ProgressFreshnessInput::NewProducer);
        }
        state.bound = Some(bound);
        state.queried_scope = None;
        state.version = state.version.saturating_add(1);
        Ok(bound)
    }
    async fn adopt_terminal(
        &self,
        peer: &DeviceId,
        generation: u64,
        notice: ProgressTerminalNotice,
    ) -> Result<ProgressTerminalNotice, ProgressReceiveError> {
        if peer != &self.peer || notice.source != self.source {
            return Err(ProgressReceiveError::UnknownTask);
        }
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.closed {
            return Err(ProgressReceiveError::UnknownTask);
        }
        if !state.plan.contains(&notice.result.item) {
            return Err(ProgressReceiveError::InvalidPlan);
        }
        if state.registration.apply(RegistrationInput::Notification {
            generation,
            binding: notice.binding,
            source: notice.source,
            attempt: notice.attempt,
            revision: notice.revision,
        }) != RegistrationEffect::CheckNotification
        {
            return Err(ProgressReceiveError::Stale);
        }
        let item = state.items.entry(notice.result.item).or_default();
        match item.apply(ProgressItemInput::Terminal(notice.result)) {
            ProgressItemEffect::Adopted => {
                fresh_notification(&self.clock, &mut state);
                if state.items.len() == state.plan.len()
                    && state
                        .items
                        .values()
                        .all(|item| matches!(item.state(), Some(ProgressItemState::Terminal(_))))
                {
                    state.freshness.apply(ProgressFreshnessInput::Settled);
                    state.view = ProgressFreshnessView::Settled;
                }
                state.version = state.version.saturating_add(1);
            }
            ProgressItemEffect::Duplicate => {}
            ProgressItemEffect::Conflict => return Err(ProgressReceiveError::Conflict),
            ProgressItemEffect::Stale => return Err(ProgressReceiveError::Stale),
            ProgressItemEffect::Invalid => return Err(ProgressReceiveError::InvalidPlan),
        }
        Ok(notice)
    }
}

fn read_clock(clock: &Clock, state: &mut State) -> Option<Duration> {
    match clock.sample() {
        Ok(Sample { now, epoch }) => {
            if state.epoch.is_some_and(|old| old != epoch) {
                state
                    .freshness
                    .apply(ProgressFreshnessInput::ContinuityLost);
            }
            state.epoch = Some(epoch);
            Some(now)
        }
        Err(source) => {
            state.clock_fault = Some(Arc::new(source));
            state
                .freshness
                .apply(ProgressFreshnessInput::ContinuityLost);
            None
        }
    }
}
fn fresh_notification(clock: &Clock, state: &mut State) {
    if let Some(now) = read_clock(clock, state) {
        state
            .freshness
            .apply(ProgressFreshnessInput::Notification(now));
        state.clock_fault = None;
        state.view = state.freshness.view(Some(now));
    }
}

fn refresh_view(clock: &Clock, state: &mut State) -> ProgressFreshnessView {
    if state.freshness.view(None) == ProgressFreshnessView::Settled {
        return ProgressFreshnessView::Settled;
    }
    let now = read_clock(clock, state);
    let view = state.freshness.view(now);
    if view != state.view {
        state.view = view;
        state.version = state.version.saturating_add(1);
    }
    view
}

#[path = "receiver_snapshot.rs"]
mod snapshot_adoption;

#[path = "receiver_unattributed.rs"]
mod unattributed;
