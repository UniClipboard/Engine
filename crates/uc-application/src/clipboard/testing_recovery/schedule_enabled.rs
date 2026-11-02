use super::super::RecoveryTrigger;
use super::{RetrySchedule, TargetRetry};
use crate::clipboard::sync::{DeliveryResult, DeliveryResultKind};
use crate::clipboard::testing_recovery::{
    self as observation, EventInput, RecoveryEventKind, RecoveryEventRef, ScheduleObservation,
};
use tokio::time::Instant;
use uc_core::ids::{DeviceId, EntryId};

#[derive(Default)]
pub(super) struct ScheduleProbe {
    source: Option<RecoveryEventRef>,
}

#[derive(Default)]
pub(super) struct TargetProbe {
    origin: Option<RecoveryEventRef>,
}

pub(super) struct UpdateObservation {
    previous_due: Option<Instant>,
    source: Option<RecoveryEventRef>,
}

impl RetrySchedule {
    pub(super) fn begin_observed_update(&mut self, target: DeviceId) -> UpdateObservation {
        let previous_due = self.targets.get(&target).and_then(|retry| retry.due);
        let source = self._observation.source.take();
        UpdateObservation {
            previous_due,
            source,
        }
    }

    pub(in super::super) fn observe_result(&mut self, result: &DeliveryResult) {
        let kind = match result.kind {
            DeliveryResultKind::Recoverable => RecoveryEventKind::ResultRecoverable,
            DeliveryResultKind::NotPersisted => RecoveryEventKind::ResultNotPersisted,
            DeliveryResultKind::Settled => RecoveryEventKind::ResultSettled,
        };
        self._observation.source = self.observe(
            kind,
            result.target_device_id,
            Some(&result.entry_id),
            Instant::now(),
            None,
        );
    }

    pub(in super::super) fn observe_reconciliation(&mut self, target: DeviceId, entry: &EntryId) {
        self._observation.source = self.observe(
            RecoveryEventKind::Reconciliation,
            target,
            Some(entry),
            Instant::now(),
            None,
        );
    }

    pub(in super::super) fn observe_failure(&mut self, target: DeviceId) {
        self._observation.source = self.observe(
            RecoveryEventKind::RoundFailure,
            target,
            None,
            Instant::now(),
            None,
        );
    }

    pub(in super::super) fn reset_observation_source(&mut self) {
        self._observation.source = None;
    }

    pub(super) fn observe_awaiting(&self, target: &DeviceId) {
        let at = Instant::now();
        observation::record(EventInput {
            kind: RecoveryEventKind::AwaitingResult,
            target: *target,
            entry: self.tracked_entry(target),
            tracked_entry: self.tracked_entry(target),
            schedule: self.observation(target),
            at,
            trigger: None,
            outcome: None,
            cause: None,
        });
    }

    pub(super) fn observe_settlement(&self, target: &DeviceId, entry: &EntryId, applied: bool) {
        let kind = if applied {
            RecoveryEventKind::SettlementApplied
        } else {
            RecoveryEventKind::SettlementIgnored
        };
        self.observe(
            kind,
            *target,
            Some(entry),
            Instant::now(),
            self._observation.source,
        );
    }

    pub(super) fn observe_clear(&self, target: &DeviceId) {
        let at = Instant::now();
        observation::record(EventInput {
            kind: RecoveryEventKind::Cleared,
            target: *target,
            entry: self.tracked_entry(target),
            tracked_entry: self.tracked_entry(target),
            schedule: self.observation(target),
            at,
            trigger: None,
            outcome: None,
            cause: None,
        });
    }

    pub(in super::super) fn observation(&self, target: &DeviceId) -> ScheduleObservation {
        self.targets
            .get(target)
            .map_or_else(ScheduleObservation::default, |retry| ScheduleObservation {
                rounds: retry.rounds,
                due: retry.due,
                origin: retry._observation.origin,
            })
    }

    fn observe(
        &self,
        kind: RecoveryEventKind,
        target: DeviceId,
        entry: Option<&EntryId>,
        at: Instant,
        cause: Option<RecoveryEventRef>,
    ) -> Option<RecoveryEventRef> {
        observation::record(EventInput {
            kind,
            target,
            entry: entry.or_else(|| self.tracked_entry(&target)),
            tracked_entry: self.tracked_entry(&target),
            schedule: self.observation(&target),
            at,
            trigger: None,
            outcome: None,
            cause,
        })
    }

    pub(super) fn observe_update(
        &mut self,
        target: DeviceId,
        observation: UpdateObservation,
        now: Instant,
    ) {
        let UpdateObservation {
            previous_due,
            source,
        } = observation;
        if let Some(retry) = self.targets.get_mut(&target) {
            if retry.due != previous_due {
                retry._observation.origin = source;
            }
        }
        self.observe(
            RecoveryEventKind::ScheduleUpdated,
            target,
            None,
            now,
            source,
        );
    }
}

impl TargetRetry {
    pub(super) fn observe_due(&self, target: DeviceId, now: Instant) {
        observation::record(EventInput {
            kind: RecoveryEventKind::RetryDue,
            target,
            entry: self.entry.as_ref(),
            tracked_entry: self.entry.as_ref(),
            schedule: ScheduleObservation {
                rounds: self.rounds + 1,
                due: self.due,
                origin: self._observation.origin,
            },
            at: now,
            trigger: Some(RecoveryTrigger::Retry.wire()),
            outcome: None,
            cause: None,
        });
    }
}
