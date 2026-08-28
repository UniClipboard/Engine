use std::sync::Arc;

use super::{JoinerStartMaterialPort, JoinerStartStatePort, PrepareJoinerCandidatePort};
use crate::space::membership::WakeSpaceMembershipMaintenancePort;
use uc_core::ports::SettingsPort;

pub(crate) struct JoinerAdmissionService {
    pub(super) settings: Arc<dyn SettingsPort>,
    pub(super) start_material: Arc<dyn JoinerStartMaterialPort>,
    pub(super) start_state: Arc<dyn JoinerStartStatePort>,
    pub(super) prepare_candidate: Arc<dyn PrepareJoinerCandidatePort>,
    pub(super) maintenance_wake: Arc<dyn WakeSpaceMembershipMaintenancePort>,
}

impl JoinerAdmissionService {
    pub(crate) fn new(
        settings: Arc<dyn SettingsPort>,
        start_material: Arc<dyn JoinerStartMaterialPort>,
        start_state: Arc<dyn JoinerStartStatePort>,
        prepare_candidate: Arc<dyn PrepareJoinerCandidatePort>,
        maintenance_wake: Arc<dyn WakeSpaceMembershipMaintenancePort>,
    ) -> Self {
        Self {
            settings,
            start_material,
            start_state,
            prepare_candidate,
            maintenance_wake,
        }
    }
}
