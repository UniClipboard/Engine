use std::collections::HashMap;
use std::future::Future;
use std::sync::{Mutex, MutexGuard};

use uc_observability_contract::diagnostics::{
    SpaceAdmissionObservation, SpaceAdmissionObservationOutcome,
};

pub(super) fn message_action(
    kind: uc_core::membership::SpaceAdmissionMessageKind,
) -> Option<uc_observability_contract::diagnostics::AdmissionObservationAction> {
    use uc_core::membership::SpaceAdmissionMessageKind as Message;
    use uc_observability_contract::diagnostics::AdmissionObservationAction as Action;
    match kind {
        Message::JoinRequest => Some(Action::RequestJoin),
        Message::Prepared => Some(Action::ConfirmPrepared),
        Message::Applied => Some(Action::ConfirmApplied),
        Message::CompleteAck => Some(Action::Settle),
        Message::CancelRequested => Some(Action::Cancel),
        _ => None,
    }
}

#[derive(Default)]
pub(crate) struct SpaceAdmissionObservationRegistry {
    active: Mutex<HashMap<[u8; 32], SpaceAdmissionObservation>>,
}

impl SpaceAdmissionObservationRegistry {
    pub(in crate::space) fn begin(&self, material: [u8; 32]) {
        self.lock()
            .entry(material)
            .or_insert_with(|| SpaceAdmissionObservation::begin(&material));
    }

    pub(in crate::space) async fn scope<T>(
        &self,
        material: [u8; 32],
        future: impl Future<Output = T>,
    ) -> T {
        let observation = {
            let mut active = self.lock();
            active
                .entry(material)
                .or_insert_with(|| SpaceAdmissionObservation::begin(&material))
                .clone()
        };
        observation.scope(future).await
    }

    pub(in crate::space) fn finish(
        &self,
        material: [u8; 32],
        outcome: SpaceAdmissionObservationOutcome,
    ) {
        if let Some(observation) = self.lock().remove(&material) {
            observation.finish(outcome);
        }
    }

    pub(in crate::space) fn finish_all(&self, outcome: SpaceAdmissionObservationOutcome) {
        let observations = self
            .lock()
            .drain()
            .map(|(_, observation)| observation)
            .collect::<Vec<_>>();
        for observation in observations {
            observation.finish(outcome);
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<[u8; 32], SpaceAdmissionObservation>> {
        match self.active.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    #[cfg(test)]
    pub(in crate::space) fn active_count(&self) -> usize {
        self.lock().len()
    }
}
