use super::{RetrySchedule, TargetRetry};
use crate::clipboard::sync::DeliveryResult;
use tokio::time::Instant;
use uc_core::ids::{DeviceId, EntryId};

#[derive(Default)]
pub(super) struct ScheduleProbe;
#[derive(Default)]
pub(super) struct TargetProbe;
pub(super) struct UpdateObservation;

impl RetrySchedule {
    pub(super) fn begin_observed_update(&mut self, _: DeviceId) -> UpdateObservation {
        UpdateObservation
    }
    pub(in super::super) fn observe_result(&mut self, _: &DeliveryResult) {}
    pub(in super::super) fn observe_reconciliation(&mut self, _: DeviceId, _: &EntryId) {}
    pub(in super::super) fn observe_failure(&mut self, _: DeviceId) {}
    pub(in super::super) fn reset_observation_source(&mut self) {}
    pub(super) fn observe_awaiting(&self, _: &DeviceId) {}
    pub(super) fn observe_settlement(&self, _: &DeviceId, _: &EntryId, _: bool) {}
    pub(super) fn observe_clear(&self, _: &DeviceId) {}
    pub(super) fn observe_update(&mut self, _: DeviceId, _: UpdateObservation, _: Instant) {}
}
impl TargetRetry {
    pub(super) fn observe_due(&self, _: DeviceId, _: Instant) {}
}
