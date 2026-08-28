use std::sync::Arc;

use crate::space::membership::WakeSpaceMembershipMaintenancePort;
use uc_core::ports::SettingsPort;

mod handle_candidate;
mod handle_commit;
mod start_join;

pub use handle_candidate::{
    PrepareJoinerCandidateError, PrepareJoinerCandidatePort, PreparedJoinerCandidateMaterial,
};
pub use handle_commit::{
    PrepareJoinerAppliedError, PrepareJoinerAppliedPort, PreparedJoinerAppliedMaterial,
};
pub use start_join::{
    JoinerStartMaterial, JoinerStartMaterialError, JoinerStartMaterialPort, JoinerStartMutation,
    JoinerStartStateError, JoinerStartStatePort, LoadedJoinerStartState, SpaceAdmissionCommitToken,
};

pub(crate) struct JoinerAdmissionService {
    pub(super) settings: Arc<dyn SettingsPort>,
    pub(super) start_material: Arc<dyn JoinerStartMaterialPort>,
    pub(super) start_state: Arc<dyn JoinerStartStatePort>,
    pub(super) prepare_candidate: Arc<dyn PrepareJoinerCandidatePort>,
    pub(super) prepare_applied: Arc<dyn PrepareJoinerAppliedPort>,
    pub(super) maintenance_wake: Arc<dyn WakeSpaceMembershipMaintenancePort>,
}

impl JoinerAdmissionService {
    pub(crate) fn new(
        settings: Arc<dyn SettingsPort>,
        start_material: Arc<dyn JoinerStartMaterialPort>,
        start_state: Arc<dyn JoinerStartStatePort>,
        prepare_candidate: Arc<dyn PrepareJoinerCandidatePort>,
        prepare_applied: Arc<dyn PrepareJoinerAppliedPort>,
        maintenance_wake: Arc<dyn WakeSpaceMembershipMaintenancePort>,
    ) -> Self {
        Self {
            settings,
            start_material,
            start_state,
            prepare_candidate,
            prepare_applied,
            maintenance_wake,
        }
    }
}
