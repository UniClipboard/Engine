//! Event-driven runtime for the unified workspace convergence owner.
//!
//! The runtime advances immediately on new changes, handoff progress,
//! member-online events and session resumption. Membership history is
//! reconciled only after an authenticated peer becomes reachable; the
//! superseded automatic-removal protocol is not a runtime fallback.

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use uc_core::ports::PresenceEvent;

use super::WorkspaceConvergence;

enum WorkspaceConvergenceRuntimeCommand {
    Pause(oneshot::Sender<()>),
    Resume(oneshot::Sender<()>),
    Shutdown(oneshot::Sender<()>),
}

pub struct WorkspaceConvergenceRuntime {
    activity: WorkspaceConvergenceActivity,
    task: Option<JoinHandle<()>>,
}

#[derive(Clone)]
pub struct WorkspaceConvergenceActivity {
    commands: mpsc::UnboundedSender<WorkspaceConvergenceRuntimeCommand>,
}

impl WorkspaceConvergence {
    /// Start the event-driven runtime. Returns the runtime handle; the
    /// runtime keeps a clone of the owner.
    pub fn start(
        self: Arc<Self>,
        mut presence_events: broadcast::Receiver<PresenceEvent>,
    ) -> WorkspaceConvergenceRuntime {
        let (commands, mut command_rx) = mpsc::unbounded_channel();
        let owner = Arc::clone(&self);
        let task = tokio::spawn(async move {
            if let Err(error) = owner.recover_pending_membership_effects().await {
                warn!(error = %error, "workspace convergence: pending membership effects deferred");
            }
            if let Err(error) = owner.deliver_pending_membership_decisions().await {
                warn!(error = %error, "workspace convergence: pending membership decisions deferred");
            }
            let mut paused = false;
            let mut presence_open = true;
            let mut recovery_tick = tokio::time::interval_at(
                tokio::time::Instant::now() + Duration::from_secs(30),
                Duration::from_secs(30),
            );
            loop {
                tokio::select! {
                    command = command_rx.recv() => match command {
                        Some(WorkspaceConvergenceRuntimeCommand::Pause(completed)) => {
                            paused = true;
                            let _ = completed.send(());
                        }
                        Some(WorkspaceConvergenceRuntimeCommand::Resume(completed)) => {
                            paused = false;
                            let _ = completed.send(());
                            if let Err(error) = owner.recover_pending_membership_effects().await {
                                warn!(error = %error, "workspace convergence: pending membership effects deferred after resume");
                            }
                            if let Err(error) = owner.deliver_pending_membership_decisions().await {
                                warn!(error = %error, "workspace convergence: pending membership decisions deferred after resume");
                            }
                            if let Err(error) = owner.synchronize_chain().await {
                                warn!(error = %error, "workspace convergence: resumed membership history exchange deferred");
                            }
                        }
                        Some(WorkspaceConvergenceRuntimeCommand::Shutdown(completed)) => {
                            let _ = completed.send(());
                            break;
                        }
                        None => break,
                    },
                    event = presence_events.recv(), if !paused && presence_open => match event {
                        Ok(event) if event.state == uc_core::ports::ReachabilityState::Online => {
                            debug!(device_id = %event.device_id.as_str(), "workspace convergence: member online");
                            if let Err(error) = owner
                                .reconcile_membership_history_with_peer(&event.device_id)
                                .await
                            {
                                warn!(error = %error, "workspace convergence: membership history exchange deferred");
                            }
                        }
                        Ok(event) if event.state == uc_core::ports::ReachabilityState::Offline => {
                            debug!(device_id = %event.device_id.as_str(), "workspace convergence: member offline");
                        }
                        Ok(_) => {}
                        Err(broadcast::error::RecvError::Lagged(_)) => {}
                        Err(broadcast::error::RecvError::Closed) => {
                            presence_open = false;
                        }
                    },
                    _ = recovery_tick.tick(), if !paused => {
                        if let Err(error) = owner.recover_pending_membership_effects().await {
                            warn!(error = %error, "workspace convergence: periodic membership effect recovery deferred");
                        }
                        if let Err(error) = owner.deliver_pending_membership_decisions().await {
                            warn!(error = %error, "workspace convergence: periodic membership decision delivery deferred");
                        }
                    }
                }
            }
        });
        WorkspaceConvergenceRuntime {
            activity: WorkspaceConvergenceActivity { commands },
            task: Some(task),
        }
    }
}

impl WorkspaceConvergenceRuntime {
    pub fn activity(&self) -> WorkspaceConvergenceActivity {
        self.activity.clone()
    }

    pub async fn shutdown(mut self) {
        // The runtime may be mid-reconcile inside a bounded network
        // exchange; never block the engine's shutdown on it. Send the
        // command, wait briefly, then abort the task.
        let (completed, response) = oneshot::channel();
        if self
            .activity
            .commands
            .send(WorkspaceConvergenceRuntimeCommand::Shutdown(completed))
            .is_ok()
        {
            if tokio::time::timeout(Duration::from_secs(5), response)
                .await
                .is_err()
            {
                warn!(
                    reason = "shutdown_wait_timeout",
                    "workspace convergence runtime did not stop in time; aborting"
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
                        error_kind = "workspace_convergence_runtime_panic",
                        "workspace convergence runtime stopped unexpectedly"
                    );
                }
            }
        }
    }
}

impl WorkspaceConvergenceActivity {
    pub(crate) async fn pause(&self) -> Result<(), String> {
        let (completed, response) = oneshot::channel();
        self.commands
            .send(WorkspaceConvergenceRuntimeCommand::Pause(completed))
            .map_err(|_| "workspace convergence runtime stopped".to_owned())?;
        response
            .await
            .map_err(|_| "workspace convergence runtime stopped".to_owned())
    }

    pub(crate) async fn resume(&self) -> Result<(), String> {
        let (completed, response) = oneshot::channel();
        self.commands
            .send(WorkspaceConvergenceRuntimeCommand::Resume(completed))
            .map_err(|_| "workspace convergence runtime stopped".to_owned())?;
        response
            .await
            .map_err(|_| "workspace convergence runtime stopped".to_owned())
    }
}

impl Drop for WorkspaceConvergenceRuntime {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}
