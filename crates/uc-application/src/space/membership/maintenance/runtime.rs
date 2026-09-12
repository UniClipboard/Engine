use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use uc_core::ports::{PeerReachabilityChanged, ReachabilityState};

use super::{MaintainSpaceMembershipUseCase, MembershipMaintenanceTrigger};

pub trait MembershipNetworkActivityPort: Send + Sync {
    fn pause_network_work(&self);
    fn resume_network_work(&self);
}

enum RuntimeCommand {
    Pause(oneshot::Sender<()>),
    Resume(oneshot::Sender<()>),
    StateChanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SpaceMembershipMaintenanceRuntimeError {
    #[error("space membership maintenance runtime is closed")]
    Closed,
}

#[derive(Clone)]
pub(crate) struct SpaceMembershipMaintenanceActivity {
    commands: mpsc::UnboundedSender<RuntimeCommand>,
    cancel: CancellationToken,
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

pub(crate) struct PreparedSpaceMembershipMaintenanceRuntime {
    maintain: Arc<MaintainSpaceMembershipUseCase>,
    peer_reachability_changed_events: broadcast::Receiver<PeerReachabilityChanged>,
    periodic_interval: Duration,
    network_activity: Arc<dyn MembershipNetworkActivityPort>,
    activity: SpaceMembershipMaintenanceActivity,
    command_rx: mpsc::UnboundedReceiver<RuntimeCommand>,
    history_changes: tokio::sync::watch::Receiver<()>,
}

impl PreparedSpaceMembershipMaintenanceRuntime {
    pub(crate) fn activity(&self) -> SpaceMembershipMaintenanceActivity {
        self.activity.clone()
    }
}

impl SpaceMembershipMaintenanceRuntime {
    pub(crate) fn prepare(
        maintain: Arc<MaintainSpaceMembershipUseCase>,
        peer_reachability_changed_events: broadcast::Receiver<PeerReachabilityChanged>,
        periodic_interval: Duration,
        network_activity: Arc<dyn MembershipNetworkActivityPort>,
        history_changes: tokio::sync::watch::Receiver<()>,
    ) -> PreparedSpaceMembershipMaintenanceRuntime {
        let (commands, command_rx) = mpsc::unbounded_channel();
        let activity = SpaceMembershipMaintenanceActivity {
            commands,
            cancel: CancellationToken::new(),
        };
        PreparedSpaceMembershipMaintenanceRuntime {
            maintain,
            peer_reachability_changed_events,
            periodic_interval,
            network_activity,
            activity,
            command_rx,
            history_changes,
        }
    }

    #[cfg(test)]
    pub(crate) fn start(
        maintain: Arc<MaintainSpaceMembershipUseCase>,
        peer_reachability_changed_events: broadcast::Receiver<PeerReachabilityChanged>,
        periodic_interval: Duration,
        network_activity: Arc<dyn MembershipNetworkActivityPort>,
    ) -> Self {
        Self::start_prepared(Self::prepare(
            maintain,
            peer_reachability_changed_events,
            periodic_interval,
            network_activity,
            tokio::sync::watch::channel(()).1,
        ))
    }

    pub(crate) fn start_prepared(prepared: PreparedSpaceMembershipMaintenanceRuntime) -> Self {
        let PreparedSpaceMembershipMaintenanceRuntime {
            maintain,
            peer_reachability_changed_events: mut reachability_changes,
            periodic_interval,
            network_activity,
            activity,
            mut command_rx,
            mut history_changes,
        } = prepared;
        let task_cancel = activity.cancel.clone();
        let task = tokio::spawn(async move {
            let mut paused = false;
            let mut peer_reachability_open = true;
            let mut history_open = true;
            let mut active_round = (!task_cancel.is_cancelled())
                .then(|| spawn_round(Arc::clone(&maintain), MembershipMaintenanceTrigger::Startup));
            let mut queued_triggers = VecDeque::new();
            let mut periodic = tokio::time::interval_at(
                tokio::time::Instant::now() + periodic_interval,
                periodic_interval,
            );
            loop {
                tokio::select! {
                    biased;
                    _ = task_cancel.cancelled() => break,
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
                    changed = history_changes.changed(), if !paused && history_open => {
                        if changed.is_err() { history_open = false; }
                        else {
                            schedule_round(&maintain, &mut active_round, &mut queued_triggers, MembershipMaintenanceTrigger::StateChanged);
                        }
                    },
                    event = reachability_changes.recv(), if !paused && peer_reachability_open => match event {
                        Ok(event) if event.state == ReachabilityState::Online => {
                            schedule_round(
                                &maintain,
                                &mut active_round,
                                &mut queued_triggers,
                                MembershipMaintenanceTrigger::PeerOnline(event.device_id),
                            );
                        }
                        Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                        Err(broadcast::error::RecvError::Closed) => peer_reachability_open = false,
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
            network_activity.pause_network_work();
            // 宿主期限由外层负责；必须等当前完整动作结束后才能释放成员运行期。
            if let Some(round) = active_round {
                let _ = round.await;
            }
        });
        Self {
            activity,
            task: Some(task),
        }
    }

    #[cfg(test)]
    pub fn activity(&self) -> SpaceMembershipMaintenanceActivity {
        self.activity.clone()
    }

    pub async fn shutdown(mut self) {
        self.activity.cancel.cancel();
        if let Some(task) = self.task.take() {
            let _ = task.await;
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
        self.activity.cancel.cancel();
    }
}
