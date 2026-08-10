//! Event-driven runtime for the unified workspace convergence owner.
//!
//! The runtime advances immediately on new changes, handoff progress,
//! confirmations, member-online events and session resumption; it never
//! relies on fixed-interval polling for normal progress. A slow timer only
//! covers offline recovery retries.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use uc_core::ports::PresenceEvent;

use super::WorkspaceConvergence;

const RECONCILE_INTERVAL: Duration = Duration::from_secs(60);

#[allow(dead_code)]
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
        let wake = self.wake_handle();
        let task = tokio::spawn(async move {
            let mut paused = false;
            let mut presence_open = true;
            let mut run_now = true;
            loop {
                if run_now && !paused {
                    run_now = false;
                    if let Err(error) = owner.reconcile().await {
                        warn!(
                            error = %error,
                            retryable = true,
                            "workspace convergence reconcile deferred"
                        );
                    }
                }
                let timer = tokio::time::sleep(RECONCILE_INTERVAL);
                tokio::pin!(timer);
                let wake = wake.notified();
                tokio::pin!(wake);
                tokio::select! {
                    command = command_rx.recv() => match command {
                        Some(WorkspaceConvergenceRuntimeCommand::Pause(completed)) => {
                            paused = true;
                            let _ = completed.send(());
                        }
                        Some(WorkspaceConvergenceRuntimeCommand::Resume(completed)) => {
                            paused = false;
                            run_now = true;
                            let _ = completed.send(());
                        }
                        Some(WorkspaceConvergenceRuntimeCommand::Shutdown(completed)) => {
                            let _ = completed.send(());
                            break;
                        }
                        None => break,
                    },
                    _ = &mut wake, if !paused => {
                        run_now = true;
                    }
                    event = presence_events.recv(), if !paused && presence_open => match event {
                        Ok(event) if event.state == uc_core::ports::ReachabilityState::Online => {
                            debug!(device_id = %event.device_id.as_str(), "workspace convergence: member online");
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
                    _ = &mut timer, if !paused => {
                        run_now = true;
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
        let (completed, response) = oneshot::channel();
        if self
            .activity
            .commands
            .send(WorkspaceConvergenceRuntimeCommand::Shutdown(completed))
            .is_ok()
        {
            let _ = response.await;
        }
        if let Some(task) = self.task.take() {
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

impl Drop for WorkspaceConvergenceRuntime {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}
