use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Notify;
use uc_application::deps::{
    CompletedJoinerActivation, ExecuteJoinerActivationError, ExecuteJoinerActivationPort,
};
use uc_core::membership::{JoinerActivationPreparation, SpaceAdmissionId};

const IDLE: u8 = 0;
const ARMED: u8 = 1;
const ENTERED: u8 = 2;

#[derive(Default)]
pub(crate) struct JoinerFinalConfirmationGate {
    state: AtomicU8,
    entered: Notify,
    released: Notify,
}

impl JoinerFinalConfirmationGate {
    pub(crate) fn arm(&self) -> bool {
        self.state
            .compare_exchange(IDLE, ARMED, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    pub(crate) async fn wait_until_entered(&self) {
        loop {
            let notified = self.entered.notified();
            if self.state.load(Ordering::Acquire) == ENTERED {
                return;
            }
            notified.await;
        }
    }

    pub(crate) fn release(&self) -> bool {
        if self
            .state
            .compare_exchange(ENTERED, IDLE, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }
        self.released.notify_waiters();
        true
    }

    async fn pause_if_armed(&self) {
        if self
            .state
            .compare_exchange(ARMED, ENTERED, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        self.entered.notify_waiters();
        loop {
            let notified = self.released.notified();
            if self.state.load(Ordering::Acquire) != ENTERED {
                return;
            }
            notified.await;
        }
    }
}

pub(crate) struct GatedJoinerActivation {
    inner: Arc<dyn ExecuteJoinerActivationPort>,
    gate: Arc<JoinerFinalConfirmationGate>,
}

impl GatedJoinerActivation {
    pub(crate) fn new(
        inner: Arc<dyn ExecuteJoinerActivationPort>,
        gate: Arc<JoinerFinalConfirmationGate>,
    ) -> Self {
        Self { inner, gate }
    }
}

#[async_trait]
impl ExecuteJoinerActivationPort for GatedJoinerActivation {
    async fn execute(
        &self,
        admission_id: SpaceAdmissionId,
        preparation: JoinerActivationPreparation<'_>,
    ) -> Result<CompletedJoinerActivation, ExecuteJoinerActivationError> {
        let completed = self.inner.execute(admission_id, preparation).await?;
        self.gate.pause_if_armed().await;
        Ok(completed)
    }

    async fn terminate(
        &self,
        admission_id: SpaceAdmissionId,
        saved_transition: &[u8],
    ) -> Result<(), ExecuteJoinerActivationError> {
        self.inner.terminate(admission_id, saved_transition).await
    }
}
