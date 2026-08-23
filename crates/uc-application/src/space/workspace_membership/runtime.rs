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

use super::WorkspaceMembership;

enum WorkspaceConvergenceRuntimeCommand {
    Pause(oneshot::Sender<()>),
    Resume(oneshot::Sender<()>),
    Shutdown(oneshot::Sender<()>),
}

#[derive(Clone, Copy)]
enum RecoveryTrigger {
    Startup,
    Resume,
    Periodic,
}

struct RecoveryTask(Option<JoinHandle<()>>);

impl RecoveryTask {
    fn startup(owner: Arc<WorkspaceMembership>) -> Self {
        Self(Some(spawn_recovery(owner, RecoveryTrigger::Startup)))
    }

    fn is_running(&self) -> bool {
        self.0.is_some()
    }

    fn start(&mut self, owner: Arc<WorkspaceMembership>, trigger: RecoveryTrigger) {
        if self.0.is_none() {
            self.0 = Some(spawn_recovery(owner, trigger));
        }
    }

    async fn cancel(&mut self) {
        let Some(task) = self.0.take() else {
            return;
        };
        task.abort();
        let _ = task.await;
    }
}

impl Drop for RecoveryTask {
    fn drop(&mut self) {
        if let Some(task) = &self.0 {
            task.abort();
        }
    }
}

pub struct WorkspaceMembershipRuntime {
    activity: WorkspaceMembershipActivity,
    task: Option<JoinHandle<()>>,
}

#[derive(Clone)]
pub struct WorkspaceMembershipActivity {
    commands: mpsc::UnboundedSender<WorkspaceConvergenceRuntimeCommand>,
}

impl WorkspaceMembership {
    /// Start the event-driven runtime. Returns the runtime handle; the
    /// runtime keeps a clone of the owner.
    pub fn start(
        self: Arc<Self>,
        mut presence_events: broadcast::Receiver<PresenceEvent>,
    ) -> WorkspaceMembershipRuntime {
        let (commands, mut command_rx) = mpsc::unbounded_channel();
        let owner = Arc::clone(&self);
        let task = tokio::spawn(async move {
            let mut recovery_task = RecoveryTask::startup(Arc::clone(&owner));
            let mut resume_recovery_pending = false;
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
                            resume_recovery_pending = false;
                            recovery_task.cancel().await;
                            let _ = completed.send(());
                        }
                        Some(WorkspaceConvergenceRuntimeCommand::Resume(completed)) => {
                            paused = false;
                            resume_recovery_pending = true;
                            let _ = completed.send(());
                            if !recovery_task.is_running() {
                                recovery_task.start(Arc::clone(&owner), RecoveryTrigger::Resume);
                                resume_recovery_pending = false;
                            }
                        }
                        Some(WorkspaceConvergenceRuntimeCommand::Shutdown(completed)) => {
                            recovery_task.cancel().await;
                            let _ = completed.send(());
                            break;
                        }
                        None => break,
                    },
                    result = async {
                        match recovery_task.0.as_mut() {
                            Some(task) => task.await,
                            None => std::future::pending().await,
                        }
                    }, if recovery_task.is_running() => {
                        if let Err(error) = result {
                            if !error.is_cancelled() {
                                warn!(error_kind = "workspace_convergence_recovery_panic", "workspace convergence recovery stopped unexpectedly");
                            }
                        }
                        recovery_task.0 = None;
                        if resume_recovery_pending && !paused {
                            recovery_task.start(Arc::clone(&owner), RecoveryTrigger::Resume);
                            resume_recovery_pending = false;
                        }
                    }
                    event = presence_events.recv(), if !paused && !recovery_task.is_running() && presence_open => match event {
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
                    _ = recovery_tick.tick(), if !paused && !recovery_task.is_running() => {
                        recovery_task.start(Arc::clone(&owner), RecoveryTrigger::Periodic);
                    }
                }
            }
        });
        WorkspaceMembershipRuntime {
            activity: WorkspaceMembershipActivity { commands },
            task: Some(task),
        }
    }
}

fn spawn_recovery(owner: Arc<WorkspaceMembership>, trigger: RecoveryTrigger) -> JoinHandle<()> {
    tokio::spawn(run_recovery(owner, trigger))
}

async fn run_recovery(owner: Arc<WorkspaceMembership>, trigger: RecoveryTrigger) {
    let context = match trigger {
        RecoveryTrigger::Startup => "startup",
        RecoveryTrigger::Resume => "resume",
        RecoveryTrigger::Periodic => "periodic",
    };
    if let Err(error) = owner.recover_pending_admissions().await {
        warn!(error = %error, recovery_context = context, "workspace convergence: pending admissions deferred");
    }
    if let Err(error) = owner.recover_pending_membership_effects().await {
        warn!(error = %error, recovery_context = context, "workspace convergence: pending membership effects deferred");
    }
    if let Err(error) = owner.deliver_pending_membership_decisions().await {
        warn!(error = %error, recovery_context = context, "workspace convergence: pending membership decisions deferred");
    }
    if matches!(trigger, RecoveryTrigger::Resume) {
        if let Err(error) = owner.synchronize_chain().await {
            warn!(error = %error, "workspace convergence: resumed membership history exchange deferred");
        }
    }
}

impl WorkspaceMembershipRuntime {
    pub fn activity(&self) -> WorkspaceMembershipActivity {
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

impl WorkspaceMembershipActivity {
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

impl Drop for WorkspaceMembershipRuntime {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}
