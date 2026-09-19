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
use uc_observability_contract::diagnostics::connectivity::{
    LocalWorkOutcome, MaintenanceDisposition, MaintenanceObservation, RecoveryTrigger,
};

use super::{MaintainSpaceMembershipUseCase, MembershipMaintenanceTrigger};
use crate::space::lifecycle::MembershipSessionActivityPort;

pub trait MembershipNetworkActivityPort: Send + Sync {
    fn pause_network_work(&self);
    fn resume_network_work(&self);
}

enum RuntimeCommand {
    Pause(oneshot::Sender<()>),
    Resume(oneshot::Sender<()>),
    StateChanged(ScheduledRound),
    Deadline(tokio::time::Instant),
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
        let round = ScheduledRound::new(MembershipMaintenanceTrigger::StateChanged);
        self.commands
            .send(RuntimeCommand::StateChanged(round))
            .map_err(|error| {
                if let RuntimeCommand::StateChanged(round) = error.0 {
                    round
                        .observation
                        .not_executed(MaintenanceDisposition::Closed);
                }
                self.closed_error()
            })
    }

    pub fn request_deadline(
        &self,
        remaining: Duration,
    ) -> Result<(), SpaceMembershipMaintenanceRuntimeError> {
        self.check_failure()?;
        self.commands
            .send(RuntimeCommand::Deadline(
                tokio::time::Instant::now() + remaining,
            ))
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
        match self.failure.get() {
            Some(source) => SpaceMembershipMaintenanceRuntimeError::Task(Arc::clone(source)),
            None => SpaceMembershipMaintenanceRuntimeError::Closed,
        }
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

    fn wake(&self) -> anyhow::Result<()> {
        self.check_failure().map_err(anyhow::Error::new)?;
        let round = ScheduledRound::new(MembershipMaintenanceTrigger::StateChanged);
        self.commands
            .send(RuntimeCommand::StateChanged(round))
            .map_err(|error| {
                if let RuntimeCommand::StateChanged(round) = error.0 {
                    round
                        .observation
                        .not_executed(MaintenanceDisposition::Closed);
                }
                anyhow::Error::new(self.closed_error())
            })
    }
}

pub(crate) struct SpaceMembershipMaintenanceRuntime {
    activity: SpaceMembershipMaintenanceActivity,
    task: Option<JoinHandle<()>>,
}

pub(crate) struct PreparedSpaceMembershipMaintenanceRuntime {
    maintain: Arc<MaintainSpaceMembershipUseCase>,
    peer_reachability_changed_events: broadcast::Receiver<PeerReachabilityChanged>,
    known_peer_contacts: broadcast::Receiver<super::KnownPeerContact>,
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
        known_peer_contacts: broadcast::Receiver<super::KnownPeerContact>,
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
            known_peer_contacts,
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
        known_peer_contacts: broadcast::Receiver<super::KnownPeerContact>,
        periodic_interval: Duration,
        network_activity: Arc<dyn MembershipNetworkActivityPort>,
    ) -> Self {
        Self::start_prepared(Self::prepare(
            maintain,
            peer_reachability_changed_events,
            known_peer_contacts,
            periodic_interval,
            network_activity,
            tokio::sync::watch::channel(()).1,
        ))
    }

    pub(crate) fn start_prepared(prepared: PreparedSpaceMembershipMaintenanceRuntime) -> Self {
        let PreparedSpaceMembershipMaintenanceRuntime {
            maintain,
            peer_reachability_changed_events: mut reachability_changes,
            known_peer_contacts: mut peer_contacts,
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
            let mut peer_contacts_open = true;
            let mut history_open = true;
            let mut active_round = (!task_cancel.is_cancelled()).then(|| {
                spawn_round(
                    Arc::clone(&maintain),
                    ScheduledRound::new(MembershipMaintenanceTrigger::Startup),
                )
            });
            let mut queued_triggers = VecDeque::new();
            let mut deadline: Option<std::pin::Pin<Box<tokio::time::Sleep>>> = None;
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
                            for round in queued_triggers.drain(..) {
                                let round: ScheduledRound = round;
                                round.observation.not_executed(MaintenanceDisposition::Paused);
                            }
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
                                    ScheduledRound::new(MembershipMaintenanceTrigger::Resume),
                                ));
                            }
                            let _ = completed.send(());
                        }
                        Some(RuntimeCommand::StateChanged(round)) if !paused => {
                            schedule_round(
                                &maintain,
                                &mut active_round,
                                &mut queued_triggers,
                                round,
                            );
                        }
                        Some(RuntimeCommand::StateChanged(round)) => { round.observation.not_executed(MaintenanceDisposition::Paused); }
                        Some(RuntimeCommand::Deadline(instant)) => {
                            if deadline
                                .as_ref()
                                .is_none_or(|current| instant < current.deadline())
                            {
                                deadline = Some(Box::pin(tokio::time::sleep_until(instant)));
                            }
                        }
                        None => break,
                    },
                    changed = history_changes.changed(), if !paused && history_open => {
                        if changed.is_err() { history_open = false; }
                        else {
                            schedule_round(&maintain, &mut active_round, &mut queued_triggers, ScheduledRound::new(MembershipMaintenanceTrigger::StateChanged));
                        }
                    },
                    event = reachability_changes.recv(), if !paused && peer_reachability_open => match event {
                        Ok(event) if event.state == ReachabilityState::Online => {
                            schedule_round(
                                &maintain,
                                &mut active_round,
                                &mut queued_triggers,
                                ScheduledRound::new(MembershipMaintenanceTrigger::PeerOnline(event.device_id)),
                            );
                        }
                        Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                        Err(broadcast::error::RecvError::Closed) => peer_reachability_open = false,
                    },
                    contact = peer_contacts.recv(), if !paused && peer_contacts_open => match contact {
                        Ok(contact) => {
                            schedule_round(
                                &maintain,
                                &mut active_round,
                                &mut queued_triggers,
                                ScheduledRound::new(MembershipMaintenanceTrigger::PeerContact(contact.device_id)),
                            );
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => {}
                        Err(broadcast::error::RecvError::Closed) => peer_contacts_open = false,
                    },
                    _ = periodic.tick(), if !paused => {
                        schedule_round(
                            &maintain,
                            &mut active_round,
                            &mut queued_triggers,
                            ScheduledRound::new(MembershipMaintenanceTrigger::Periodic),
                        );
                    }
                    _ = async {
                        match deadline.as_mut() {
                            Some(sleep) => sleep.await,
                            None => std::future::pending().await,
                        }
                    }, if !paused && deadline.is_some() => {
                        deadline = None;
                        schedule_round(
                            &maintain,
                            &mut active_round,
                            &mut queued_triggers,
                            ScheduledRound::new(MembershipMaintenanceTrigger::StateChanged),
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
    round: ScheduledRound,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let ScheduledRound {
            trigger,
            mut observation,
        } = round;
        observation.start();
        let report = observation.scope(maintain.execute(trigger)).await;
        let outcome = if report.corrupt_count > 0 {
            LocalWorkOutcome::Corrupt
        } else if report.stable_failure_count > 0 {
            LocalWorkOutcome::Error
        } else if report.deferred_count > 0 {
            LocalWorkOutcome::Deferred
        } else {
            LocalWorkOutcome::Ok
        };
        observation.finish(outcome);
    })
}

fn schedule_round(
    maintain: &Arc<MaintainSpaceMembershipUseCase>,
    active_round: &mut Option<JoinHandle<()>>,
    queued_triggers: &mut VecDeque<ScheduledRound>,
    round: ScheduledRound,
) {
    if active_round.is_some() {
        if let Some(existing) = queued_triggers
            .iter()
            .find(|existing| existing.trigger == round.trigger)
        {
            round.observation.coalesce(&existing.observation);
        } else {
            round.observation.queued();
            queued_triggers.push_back(round);
        }
    } else {
        *active_round = Some(spawn_round(Arc::clone(maintain), round));
    }
}

struct ScheduledRound {
    trigger: MembershipMaintenanceTrigger,
    observation: MaintenanceObservation,
}

impl ScheduledRound {
    fn new(trigger: MembershipMaintenanceTrigger) -> Self {
        let reason = match &trigger {
            MembershipMaintenanceTrigger::Startup => RecoveryTrigger::Startup,
            MembershipMaintenanceTrigger::Resume => RecoveryTrigger::Resume,
            MembershipMaintenanceTrigger::Periodic => RecoveryTrigger::Periodic,
            MembershipMaintenanceTrigger::StateChanged => RecoveryTrigger::StateChanged,
            MembershipMaintenanceTrigger::PeerContact(_) => RecoveryTrigger::PeerContact,
            MembershipMaintenanceTrigger::PeerOnline(_) => RecoveryTrigger::PeerOnline,
        };
        Self {
            trigger,
            observation: MaintenanceObservation::request(reason),
        }
    }
}

impl Drop for SpaceMembershipMaintenanceRuntime {
    fn drop(&mut self) {
        self.activity.cancel.cancel();
    }
}
