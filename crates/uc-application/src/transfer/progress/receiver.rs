//! 单个发送意图的完整登记、活动与终态采用负责人；网络等待不持有投影锁。
use super::{
    CurrentProgressTaskPort, ProgressBound, ProgressProjectionSnapshot, ProgressReceiveError,
    ProgressReceiverPort, ProgressTerminalNotice, ProgressUpdateNotice,
};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use uc_core::file_transfer::{
    valid_progress_plan, ProgressIdentity, ProgressItem, ProgressItemEffect, ProgressItemInput,
    ProgressItemProjection, ProgressRegistration, RegistrationEffect, RegistrationInput,
};
use uc_core::ids::DeviceId;
use uuid::Uuid;

pub struct TransferProgressReceiver {
    peer: DeviceId,
    source: ProgressIdentity,
    plan: Vec<ProgressItem>,
    state: Mutex<State>,
}
struct State {
    generation: u64,
    registration: ProgressRegistration<ProgressIdentity>,
    bound: Option<ProgressBound>,
    items: HashMap<ProgressItem, ProgressItemProjection>,
    version: u64,
}
impl TransferProgressReceiver {
    /// 由原始发送意图创建；身份与逻辑计划不由第一条远端进度授权。
    pub fn new(
        peer: DeviceId,
        source: ProgressIdentity,
        plan: Vec<ProgressItem>,
    ) -> Result<Self, ProgressReceiveError> {
        if !valid_progress_plan(&plan) {
            return Err(ProgressReceiveError::InvalidPlan);
        }
        Ok(Self {
            peer,
            source,
            plan,
            state: Mutex::new(State {
                generation: 0,
                registration: ProgressRegistration::new(source),
                bound: None,
                items: HashMap::new(),
                version: 0,
            }),
        })
    }
    pub fn snapshot(&self) -> ProgressProjectionSnapshot {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        ProgressProjectionSnapshot {
            version: state.version,
            bound: state.bound,
            items: self
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
                .collect(),
        }
    }
}
#[async_trait]
impl ProgressReceiverPort for TransferProgressReceiver {
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
        if !self.plan.contains(&notice.update.item) {
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
            ProgressItemEffect::Adopted => state.version = state.version.saturating_add(1),
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
        if usize::from(descriptor.item_count) != self.plan.len() {
            return Err(ProgressReceiveError::InvalidPlan);
        }
        if state.registration.apply(RegistrationInput::Describe {
            generation,
            nonce,
            binding,
            descriptor,
        }) != RegistrationEffect::SendBound
        {
            return Err(ProgressReceiveError::Stale);
        }
        if state
            .bound
            .is_none_or(|previous| previous.descriptor.attempt != descriptor.attempt)
        {
            state.items.clear();
        }
        state.bound = Some(bound);
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
        if !self.plan.contains(&notice.result.item) {
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
