use std::sync::Arc;

use super::{PendingAdmissionRecoveryStatePort, SpaceAdmissionTransportPort};

pub(crate) struct AdmissionRecoveryService {
    pub(super) state: Arc<dyn PendingAdmissionRecoveryStatePort>,
    pub(super) transport: Arc<dyn SpaceAdmissionTransportPort>,
}

impl AdmissionRecoveryService {
    pub(crate) fn new(
        state: Arc<dyn PendingAdmissionRecoveryStatePort>,
        transport: Arc<dyn SpaceAdmissionTransportPort>,
    ) -> Self {
        Self { state, transport }
    }
}
