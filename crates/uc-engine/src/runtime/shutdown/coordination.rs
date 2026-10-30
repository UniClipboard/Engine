use std::future::Future;

use async_trait::async_trait;
use tokio::time::Instant;

#[derive(Default)]
pub(super) struct ShutdownOutcome {
    pub(super) errors: Vec<anyhow::Error>,
    pub(super) session_stopped: bool,
    pub(super) file_transfers_closed: bool,
    pub(super) process_tasks_stopped: bool,
}

impl ShutdownOutcome {
    pub(super) fn resources_can_close(&self) -> bool {
        self.session_stopped && self.file_transfers_closed && self.process_tasks_stopped
    }
}

#[async_trait]
pub(super) trait ShutdownActions: Send + Sync {
    async fn stop_session(&self, deadline: Option<Instant>) -> anyhow::Result<()>;
    async fn close_file_transfers(&self) -> anyhow::Result<()>;
    async fn stop_process_tasks(&self, deadline: Option<Instant>) -> anyhow::Result<()>;
    fn close_local_resources(&self);
}

pub(super) async fn stop_runtime_resource_users<Recovery>(
    stop_network_recovery: Recovery,
    actions: &dyn ShutdownActions,
    deadline: Option<Instant>,
) -> ShutdownOutcome
where
    Recovery: Future<Output = anyhow::Result<()>>,
{
    let (recovery, mut outcome) = tokio::join!(
        stop_network_recovery,
        stop_resource_users(actions, deadline),
    );
    if outcome.resources_can_close() && recovery.is_ok() {
        actions.close_local_resources();
    }
    if let Err(error) = recovery {
        outcome.errors.insert(0, error);
    }
    outcome
}

pub(super) async fn stop_resource_users_and_close(
    actions: &dyn ShutdownActions,
    deadline: Option<Instant>,
) -> ShutdownOutcome {
    let outcome = stop_resource_users(actions, deadline).await;
    if outcome.resources_can_close() {
        actions.close_local_resources();
    }
    outcome
}

pub(super) async fn stop_resource_users(
    actions: &dyn ShutdownActions,
    deadline: Option<Instant>,
) -> ShutdownOutcome {
    let (mut outcome, process_tasks) = tokio::join!(
        stop_session_and_transfers(actions, deadline),
        actions.stop_process_tasks(deadline),
    );
    match process_tasks {
        Ok(()) => outcome.process_tasks_stopped = true,
        Err(error) => outcome.errors.push(error),
    }
    outcome
}

async fn stop_session_and_transfers(
    actions: &dyn ShutdownActions,
    deadline: Option<Instant>,
) -> ShutdownOutcome {
    let mut outcome = ShutdownOutcome::default();
    match actions.stop_session(deadline).await {
        Ok(()) => outcome.session_stopped = true,
        Err(error) => outcome.errors.push(error),
    }
    match actions.close_file_transfers().await {
        Ok(()) => outcome.file_transfers_closed = true,
        Err(error) => outcome.errors.push(error),
    }
    outcome
}
