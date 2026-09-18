use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;

use uc_core::ports::{PeerReachabilityChanged, ReachabilityState};
use uc_observability_contract::diagnostics::connectivity::{
    LocalWorkOutcome, MaintenanceDisposition, MaintenanceObservation, RecoveryTrigger,
};

use super::{MaintainSpaceMembershipUseCase, MembershipMaintenanceTrigger};

pub trait MembershipNetworkActivityPort: Send + Sync {
    fn pause_network_work(&self);
    fn resume_network_work(&self);
}

enum RuntimeCommand {
    Pause(oneshot::Sender<()>),
    Resume(oneshot::Sender<()>),
    PrepareSession(ScheduledRound),
    StateChanged(ScheduledRound),
    Deadline(tokio::time::Instant),
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
        let round = ScheduledRound::new(MembershipMaintenanceTrigger::StateChanged);
        self.commands
            .send(RuntimeCommand::StateChanged(round))
            .map_err(|error| {
                if let RuntimeCommand::StateChanged(round) = error.0 {
                    round
                        .observation
                        .not_executed(MaintenanceDisposition::Closed);
                }
                SpaceMembershipMaintenanceRuntimeError::Closed
            })
    }

    pub async fn prepare_for_session(&self) -> Result<(), SpaceMembershipMaintenanceRuntimeError> {
        let (completed, receiver) = oneshot::channel();
        let round = ScheduledRound::new(MembershipMaintenanceTrigger::StateChanged)
            .with_completion(completed);
        self.commands
            .send(RuntimeCommand::PrepareSession(round))
            .map_err(|error| {
                if let RuntimeCommand::PrepareSession(round) = error.0 {
                    round
                        .observation
                        .not_executed(MaintenanceDisposition::Closed);
                }
                SpaceMembershipMaintenanceRuntimeError::Closed
            })?;
        receiver
            .await
            .map_err(|_| SpaceMembershipMaintenanceRuntimeError::Closed)
    }

    pub fn request_deadline(
        &self,
        remaining: Duration,
    ) -> Result<(), SpaceMembershipMaintenanceRuntimeError> {
        self.commands
            .send(RuntimeCommand::Deadline(
                tokio::time::Instant::now() + remaining,
            ))
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

    async fn prepare_for_session(&self) -> Result<(), String> {
        self.prepare_for_session()
            .await
            .map_err(|error| error.to_string())
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
        let activity = SpaceMembershipMaintenanceActivity { commands };
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
        let task = tokio::spawn(async move {
            let mut paused = false;
            let mut peer_reachability_open = true;
            let mut peer_contacts_open = true;
            let mut history_open = true;
            let mut active_round = Some(spawn_round(
                Arc::clone(&maintain),
                ScheduledRound::new(MembershipMaintenanceTrigger::Startup),
            ));
            let mut queued_triggers = VecDeque::new();
            let mut deadline: Option<std::pin::Pin<Box<tokio::time::Sleep>>> = None;
            let mut periodic = tokio::time::interval_at(
                tokio::time::Instant::now() + periodic_interval,
                periodic_interval,
            );
            loop {
                tokio::select! {
                    command = command_rx.recv() => match command {
                        Some(RuntimeCommand::Pause(completed)) => {
                            paused = true;
                            for round in queued_triggers.drain(..) {
                                let round: ScheduledRound = round;
                                round.observation.not_executed(MaintenanceDisposition::Paused);
                            }
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
                                    ScheduledRound::new(MembershipMaintenanceTrigger::Resume),
                                ));
                            }
                            let _ = completed.send(());
                        }
                        Some(RuntimeCommand::PrepareSession(round)) => {
                            network_activity.resume_network_work();
                            paused = false;
                            schedule_round(
                                &maintain,
                                &mut active_round,
                                &mut queued_triggers,
                                round,
                            );
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
    round: ScheduledRound,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let ScheduledRound {
            trigger,
            mut observation,
            completed,
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
        if let Some(completed) = completed {
            let _ = completed.send(());
        }
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
            .find(|existing| round.completed.is_none() && existing.trigger == round.trigger)
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
    completed: Option<oneshot::Sender<()>>,
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
            completed: None,
        }
    }

    fn with_completion(mut self, completed: oneshot::Sender<()>) -> Self {
        self.completed = Some(completed);
        self
    }
}

impl Drop for SpaceMembershipMaintenanceRuntime {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}
