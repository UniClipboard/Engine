use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;

use uc_core::ports::{PresenceEvent, ReachabilityState};

use super::{MaintainSpaceMembershipUseCase, MembershipMaintenanceTrigger};

pub trait MembershipNetworkActivityPort: Send + Sync {
    fn pause_network_work(&self);
    fn resume_network_work(&self);
}

enum RuntimeCommand {
    Pause(oneshot::Sender<()>),
    Resume(oneshot::Sender<()>),
    StateChanged,
    Shutdown(oneshot::Sender<()>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SpaceMembershipMaintenanceRuntimeError {
    #[error("space membership maintenance runtime is closed")]
    Closed,
}

#[derive(Clone)]
pub(crate) struct SpaceMembershipMaintenanceActivity {
    commands: mpsc::UnboundedSender<RuntimeCommand>,
}

impl SpaceMembershipMaintenanceActivity {
    pub async fn pause(&self) -> Result<(), SpaceMembershipMaintenanceRuntimeError> {
        self.request(RuntimeCommand::Pause).await
    }

    pub async fn resume(&self) -> Result<(), SpaceMembershipMaintenanceRuntimeError> {
        self.request(RuntimeCommand::Resume).await
    }

    pub fn request_state_changed(&self) -> Result<(), SpaceMembershipMaintenanceRuntimeError> {
        self.commands
            .send(RuntimeCommand::StateChanged)
            .map_err(|_| SpaceMembershipMaintenanceRuntimeError::Closed)
    }

    async fn request(
        &self,
        command: impl FnOnce(oneshot::Sender<()>) -> RuntimeCommand,
    ) -> Result<(), SpaceMembershipMaintenanceRuntimeError> {
        let (completed, receiver) = oneshot::channel();
        self.commands
            .send(command(completed))
            .map_err(|_| SpaceMembershipMaintenanceRuntimeError::Closed)?;
        receiver
            .await
            .map_err(|_| SpaceMembershipMaintenanceRuntimeError::Closed)
    }
}

#[async_trait::async_trait]
impl crate::space::lifecycle::MembershipSessionActivityPort for SpaceMembershipMaintenanceActivity {
    async fn pause(&self) -> Result<(), String> {
        self.pause().await.map_err(|error| error.to_string())
    }

    async fn resume(&self) -> Result<(), String> {
        self.resume().await.map_err(|error| error.to_string())
    }
}

pub(crate) struct SpaceMembershipMaintenanceRuntime {
    activity: SpaceMembershipMaintenanceActivity,
    task: Option<JoinHandle<()>>,
}

impl SpaceMembershipMaintenanceRuntime {
    pub(crate) fn start(
        maintain: Arc<MaintainSpaceMembershipUseCase>,
        mut presence_events: broadcast::Receiver<PresenceEvent>,
        periodic_interval: Duration,
        network_activity: Arc<dyn MembershipNetworkActivityPort>,
    ) -> Self {
        let (commands, mut command_rx) = mpsc::unbounded_channel();
        let activity = SpaceMembershipMaintenanceActivity { commands };
        let task = tokio::spawn(async move {
            let mut paused = false;
            let mut presence_open = true;
            let mut active_round = Some(spawn_round(
                Arc::clone(&maintain),
                MembershipMaintenanceTrigger::Startup,
            ));
            let mut queued_triggers = VecDeque::new();
            let mut periodic = tokio::time::interval_at(
                tokio::time::Instant::now() + periodic_interval,
                periodic_interval,
            );
            loop {
                tokio::select! {
                    command = command_rx.recv() => match command {
                        Some(RuntimeCommand::Pause(completed)) => {
                            paused = true;
                            queued_triggers.clear();
                            network_activity.pause_network_work();
                            if let Some(round) = active_round.take() {
                                let _ = round.await;
                            }
                            let _ = completed.send(());
                        }
                        Some(RuntimeCommand::Resume(completed)) => {
                            network_activity.resume_network_work();
                            let should_run = paused;
                            paused = false;
                            if should_run && active_round.is_none() {
                                active_round = Some(spawn_round(
                                    Arc::clone(&maintain),
                                    MembershipMaintenanceTrigger::Resume,
                                ));
                            }
                            let _ = completed.send(());
                        }
                        Some(RuntimeCommand::StateChanged) if !paused => {
                            schedule_round(
                                &maintain,
                                &mut active_round,
                                &mut queued_triggers,
                                MembershipMaintenanceTrigger::StateChanged,
                            );
                        }
                        Some(RuntimeCommand::StateChanged) => {}
                        Some(RuntimeCommand::Shutdown(completed)) => {
                            network_activity.pause_network_work();
                            if let Some(mut round) = active_round.take() {
                                let _ = tokio::time::timeout(Duration::from_secs(5), &mut round).await;
                            }
                            let _ = completed.send(());
                            break;
                        }
                        None => break,
                    },
                    result = async {
                        match active_round.as_mut() {
                            Some(round) => Some(round.await),
                            None => None,
                        }
                    }, if active_round.is_some() => {
                        let _ = result;
                        active_round = None;
                        if !paused {
                            if let Some(trigger) = queued_triggers.pop_front() {
                                active_round = Some(spawn_round(Arc::clone(&maintain), trigger));
                            }
                        }
                    },
                    event = presence_events.recv(), if !paused && presence_open => match event {
                        Ok(event) if event.state == ReachabilityState::Online => {
                            schedule_round(
                                &maintain,
                                &mut active_round,
                                &mut queued_triggers,
                                MembershipMaintenanceTrigger::PeerOnline(event.device_id),
                            );
                        }
                        Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                        Err(broadcast::error::RecvError::Closed) => presence_open = false,
                    },
                    _ = periodic.tick(), if !paused => {
                        schedule_round(
                            &maintain,
                            &mut active_round,
                            &mut queued_triggers,
                            MembershipMaintenanceTrigger::Periodic,
                        );
                    }
                }
            }
        });
        Self {
            activity,
            task: Some(task),
        }
    }

    pub fn activity(&self) -> SpaceMembershipMaintenanceActivity {
        self.activity.clone()
    }

    pub async fn shutdown(mut self) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let (completed, receiver) = oneshot::channel();
        if self
            .activity
            .commands
            .send(RuntimeCommand::Shutdown(completed))
            .is_ok()
        {
            let _ = tokio::time::timeout_at(deadline, receiver).await;
        }
        if let Some(mut task) = self.task.take() {
            if tokio::time::timeout_at(deadline, &mut task).await.is_err() {
                task.abort();
                let _ = task.await;
            }
        }
    }
}

fn spawn_round(
    maintain: Arc<MaintainSpaceMembershipUseCase>,
    trigger: MembershipMaintenanceTrigger,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        maintain.execute(trigger).await;
    })
}

fn schedule_round(
    maintain: &Arc<MaintainSpaceMembershipUseCase>,
    active_round: &mut Option<JoinHandle<()>>,
    queued_triggers: &mut VecDeque<MembershipMaintenanceTrigger>,
    trigger: MembershipMaintenanceTrigger,
) {
    if active_round.is_some() {
        if !queued_triggers.contains(&trigger) {
            queued_triggers.push_back(trigger);
        }
    } else {
        *active_round = Some(spawn_round(Arc::clone(maintain), trigger));
    }
}

impl Drop for SpaceMembershipMaintenanceRuntime {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}
