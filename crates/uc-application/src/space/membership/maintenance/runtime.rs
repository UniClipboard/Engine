use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use thiserror::Error;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::{JoinError, JoinHandle};
use tokio_util::sync::CancellationToken;

use uc_core::ports::{PeerReachabilityChanged, ReachabilityState};

use super::{MaintainSpaceMembershipUseCase, MembershipMaintenanceTrigger};
use crate::space::lifecycle::MembershipSessionActivityPort;

pub trait MembershipNetworkActivityPort: Send + Sync {
    fn pause_network_work(&self);
    fn resume_network_work(&self);
}

enum RuntimeCommand {
    Pause(oneshot::Sender<()>),
    Resume(oneshot::Sender<()>),
    StateChanged,
}

#[derive(Clone, Error)]
pub enum SpaceMembershipMaintenanceRuntimeError {
    #[error("space membership maintenance runtime is closed")]
    Closed,
    #[error("space membership maintenance task failed")]
    Task(#[source] Arc<JoinError>),
}

impl fmt::Debug for SpaceMembershipMaintenanceRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

#[derive(Clone)]
pub(crate) struct SpaceMembershipMaintenanceActivity {
    commands: mpsc::UnboundedSender<RuntimeCommand>,
    cancel: CancellationToken,
    failure: Arc<OnceLock<Arc<JoinError>>>,
}

impl SpaceMembershipMaintenanceActivity {
    pub async fn pause(&self) -> Result<(), SpaceMembershipMaintenanceRuntimeError> {
        self.request(RuntimeCommand::Pause).await
    }

    pub async fn resume(&self) -> Result<(), SpaceMembershipMaintenanceRuntimeError> {
        self.request(RuntimeCommand::Resume).await
    }

    pub fn request_state_changed(&self) -> Result<(), SpaceMembershipMaintenanceRuntimeError> {
        self.check_failure()?;
        self.commands
            .send(RuntimeCommand::StateChanged)
            .map_err(|_| self.closed_error())
    }

    async fn request(
        &self,
        command: impl FnOnce(oneshot::Sender<()>) -> RuntimeCommand,
    ) -> Result<(), SpaceMembershipMaintenanceRuntimeError> {
        self.check_failure()?;
        let (completed, receiver) = oneshot::channel();
        self.commands
            .send(command(completed))
            .map_err(|_| self.closed_error())?;
        receiver.await.map_err(|_| self.closed_error())
    }

    fn check_failure(&self) -> Result<(), SpaceMembershipMaintenanceRuntimeError> {
        match self.failure.get() {
            Some(source) => Err(SpaceMembershipMaintenanceRuntimeError::Task(Arc::clone(
                source,
            ))),
            None => Ok(()),
        }
    }

    fn closed_error(&self) -> SpaceMembershipMaintenanceRuntimeError {
        self.check_failure()
            .err()
            .unwrap_or(SpaceMembershipMaintenanceRuntimeError::Closed)
    }
}

#[async_trait]
impl MembershipSessionActivityPort for SpaceMembershipMaintenanceActivity {
    async fn pause(&self) -> anyhow::Result<()> {
        self.pause().await.map_err(anyhow::Error::new)
    }

    async fn resume(&self) -> anyhow::Result<()> {
        self.resume().await.map_err(anyhow::Error::new)
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
            failure: Arc::new(OnceLock::new()),
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
        let failure = Arc::clone(&activity.failure);
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
                    result = async {
                        match active_round.as_mut() {
                            Some(round) => Some(round.await),
                            None => None,
                        }
                    }, if active_round.is_some() => {
                        active_round = None;
                        if let Some(Err(source)) = result {
                            let _ = failure.set(Arc::new(source));
                            break;
                        }
                        if !paused {
                            if let Some(trigger) = queued_triggers.pop_front() {
                                active_round = Some(spawn_round(Arc::clone(&maintain), trigger));
                            }
                        }
                    },
                    command = command_rx.recv() => match command {
                        Some(RuntimeCommand::Pause(completed)) => {
                            paused = true;
                            queued_triggers.clear();
                            network_activity.pause_network_work();
                            if let Some(round) = active_round.take() {
                                if let Err(source) = round.await {
                                    let _ = failure.set(Arc::new(source));
                                    break;
                                }
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
                if let Err(source) = round.await {
                    let _ = failure.set(Arc::new(source));
                }
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

    pub async fn shutdown(mut self) -> anyhow::Result<()> {
        self.activity.cancel.cancel();
        if let Some(task) = self.task.take() {
            if let Err(source) = task.await {
                let _ = self.activity.failure.set(Arc::new(source));
            }
        }
        self.activity.check_failure().map_err(anyhow::Error::new)
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
