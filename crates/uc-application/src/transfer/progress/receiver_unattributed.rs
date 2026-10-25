//! 无尝试关联的通知只改变确认程度；不借用新版任务身份或采用粗粒度结果。
use super::TransferProgressReceiver;
use crate::transfer::progress::{ProgressReceiveError, ProgressUnattributedReceiverPort};
use async_trait::async_trait;
use std::sync::PoisonError;
use uc_core::file_transfer::{
    ProgressFreshnessInput, ProgressFreshnessView, ProgressIdentity, RegistrationEffect,
    RegistrationInput,
};
use uc_core::ids::DeviceId;

#[async_trait]
impl ProgressUnattributedReceiverPort for TransferProgressReceiver {
    async fn observe_unattributed(
        &self,
        peer: &DeviceId,
        source: ProgressIdentity,
    ) -> Result<(), ProgressReceiveError> {
        if peer != &self.peer || source != self.source {
            return Err(ProgressReceiveError::UnknownTask);
        }
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.closed {
            return Err(ProgressReceiveError::UnknownTask);
        }
        if state
            .registration
            .apply(RegistrationInput::UnattributedNotification { source })
            != RegistrationEffect::MarkUnconfirmed
        {
            return Err(ProgressReceiveError::Stale);
        }
        state
            .freshness
            .apply(ProgressFreshnessInput::ContinuityLost);
        if state.view != ProgressFreshnessView::Unconfirmed {
            state.view = ProgressFreshnessView::Unconfirmed;
            state.version = state.version.saturating_add(1);
        }
        Ok(())
    }
}
