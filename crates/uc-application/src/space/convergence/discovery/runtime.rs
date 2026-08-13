use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use super::{gossip_reconcile_delay, MembershipConvergence, INITIAL_RETRY_DELAY_MS};

enum MembershipConvergenceRuntimeCommand {
    Pause(oneshot::Sender<()>),
    Resume(oneshot::Sender<()>),
    Shutdown(oneshot::Sender<()>),
}

#[derive(Debug, thiserror::Error)]
pub(super) enum MembershipConvergenceRuntimeError {
    #[error("membership gossip runtime is stopped")]
    Stopped,
}

pub struct MembershipConvergenceRuntime {
    activity: MembershipConvergenceActivity,
    task: Option<JoinHandle<()>>,
}

#[derive(Clone)]
pub struct MembershipConvergenceActivity {
    commands: mpsc::UnboundedSender<MembershipConvergenceRuntimeCommand>,
}

impl MembershipConvergence {
    pub fn start(
        self: Arc<Self>,
        mut presence_events: broadcast::Receiver<uc_core::ports::PresenceEvent>,
    ) -> MembershipConvergenceRuntime {
        let (commands, mut command_rx) = mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            let mut paused = false;
            let mut presence_open = true;
            let mut announcement_changes_open = true;
            let mut run_now = true;
            loop {
                let mut pass_failed = false;
                if run_now && !paused {
                    run_now = false;
                    let mut pass = Box::pin(self.reconcile_once());
                    let (completed_pass, pause_completed) = loop {
                        tokio::select! {
                            result = &mut pass => break (Some(result), None),
                            command = command_rx.recv() => match command {
                                Some(MembershipConvergenceRuntimeCommand::Pause(completed)) => {
                                    paused = true;
                                    run_now = true;
                                    break (None, Some(completed));
                                }
                                Some(MembershipConvergenceRuntimeCommand::Resume(completed)) => {
                                    let _ = completed.send(());
                                }
                                Some(MembershipConvergenceRuntimeCommand::Shutdown(completed)) => {
                                    let _ = completed.send(());
                                    return;
                                }
                                None => return,
                            }
                        }
                    };
                    // Dropping the pass first guarantees that a completed pause
                    // cannot leave an in-flight network exchange running.
                    drop(pass);
                    if let Some(completed) = pause_completed {
                        let _ = completed.send(());
                    }
                    match completed_pass {
                        Some(Ok(outcome)) => {
                            debug!(
                                delivered_batches = outcome.delivered_batches,
                                confirmed_candidates = outcome.confirmed_candidates,
                                synchronized_members = outcome.synchronized_members,
                                deferred_items = outcome.deferred_items,
                                "membership gossip pass completed"
                            );
                        }
                        Some(Err(_)) => {
                            pass_failed = true;
                            warn!(
                                error_kind = "membership_gossip_reconcile",
                                retryable = true,
                                "membership gossip pass deferred"
                            );
                        }
                        None => {}
                    }
                }

                let reconcile_delay = if paused {
                    gossip_reconcile_delay(&self.deps.device_identity.current_device_id())
                } else if pass_failed {
                    Duration::from_millis(INITIAL_RETRY_DELAY_MS as u64)
                } else {
                    self.next_reconcile_delay().await
                };
                let timer = tokio::time::sleep(reconcile_delay);
                tokio::pin!(timer);
                let announcement_change = self
                    .deps
                    .announcement_material
                    .wait_for_announcement_change();
                tokio::pin!(announcement_change);
                tokio::select! {
                    command = command_rx.recv() => match command {
                        Some(MembershipConvergenceRuntimeCommand::Pause(completed)) => {
                            paused = true;
                            let _ = completed.send(());
                        }
                        Some(MembershipConvergenceRuntimeCommand::Resume(completed)) => {
                            paused = false;
                            run_now = true;
                            let _ = completed.send(());
                        }
                        Some(MembershipConvergenceRuntimeCommand::Shutdown(completed)) => {
                            let _ = completed.send(());
                            break;
                        }
                        None => break,
                    },
                    _ = self.wake.notified(), if !paused => {
                        run_now = true;
                    }
                    event = presence_events.recv(), if !paused && presence_open => match event {
                        Ok(event) if event.state == uc_core::ports::ReachabilityState::Online => {
                            run_now = true;
                        }
                        Ok(_) => {}
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            run_now = true;
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            presence_open = false;
                        }
                    },
                    change = &mut announcement_change, if !paused && announcement_changes_open => {
                        match change {
                            Ok(()) => run_now = true,
                            Err(_) => announcement_changes_open = false,
                        }
                    },
                    _ = &mut timer, if !paused => {
                        run_now = true;
                    }
                }
            }
        });
        MembershipConvergenceRuntime {
            activity: MembershipConvergenceActivity { commands },
            task: Some(task),
        }
    }
}

impl MembershipConvergenceRuntime {
    pub(crate) fn activity(&self) -> MembershipConvergenceActivity {
        self.activity.clone()
    }

    pub(crate) async fn shutdown(mut self) {
        let (completed, response) = oneshot::channel();
        if self
            .activity
            .commands
            .send(MembershipConvergenceRuntimeCommand::Shutdown(completed))
            .is_ok()
        {
            if tokio::time::timeout(Duration::from_secs(2), response)
                .await
                .is_err()
            {
                warn!(
                    error_kind = "membership_gossip_shutdown_timeout",
                    "membership gossip runtime did not stop in time; aborting"
                );
            }
        }
        if let Some(task) = self.task.take() {
            if !task.is_finished() {
                task.abort();
            }
            if let Err(error) = task.await {
                if !error.is_cancelled() {
                    warn!(
                        error_kind = "membership_gossip_runtime_panic",
                        "membership gossip runtime stopped unexpectedly"
                    );
                }
            }
        }
    }
}

impl MembershipConvergenceActivity {
    pub(super) async fn pause(&self) -> Result<(), MembershipConvergenceRuntimeError> {
        let (completed, response) = oneshot::channel();
        self.commands
            .send(MembershipConvergenceRuntimeCommand::Pause(completed))
            .map_err(|_| MembershipConvergenceRuntimeError::Stopped)?;
        response
            .await
            .map_err(|_| MembershipConvergenceRuntimeError::Stopped)
    }

    pub(super) async fn resume(&self) -> Result<(), MembershipConvergenceRuntimeError> {
        let (completed, response) = oneshot::channel();
        self.commands
            .send(MembershipConvergenceRuntimeCommand::Resume(completed))
            .map_err(|_| MembershipConvergenceRuntimeError::Stopped)?;
        response
            .await
            .map_err(|_| MembershipConvergenceRuntimeError::Stopped)
    }
}

#[async_trait]
impl crate::space::convergence::discovery::MembershipConvergenceActivityPort
    for MembershipConvergenceActivity
{
    async fn pause(&self) -> Result<(), String> {
        self.pause().await.map_err(|error| error.to_string())
    }

    async fn resume(&self) -> Result<(), String> {
        self.resume().await.map_err(|error| error.to_string())
    }
}

impl Drop for MembershipConvergenceRuntime {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}
