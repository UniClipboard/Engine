use std::sync::Arc;

use crate::space::membership::WakeSpaceMembershipMaintenancePort;
use uc_core::ports::SettingsPort;

mod activate_complete;
mod handle_candidate;
mod handle_commit;
mod handle_complete;
mod handle_settled;
mod start_join;

pub use activate_complete::{
    CompletedJoinerActivation, ExecuteJoinerActivationError, ExecuteJoinerActivationPort,
    JoinerActivationCommitToken, JoinerActivationMutation, JoinerActivationStateError,
    JoinerActivationStatePort, LoadedJoinerActivation,
};
pub use handle_candidate::{
    PrepareJoinerCandidateError, PrepareJoinerCandidatePort, PreparedJoinerCandidateMaterial,
};
pub use handle_commit::{
    PrepareJoinerAppliedError, PrepareJoinerAppliedPort, PreparedJoinerAppliedMaterial,
};
pub use handle_complete::{
    PrepareJoinerActivationError, PrepareJoinerActivationPort, PreparedJoinerActivation,
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
    pub(super) prepare_activation: Arc<dyn PrepareJoinerActivationPort>,
    pub(super) activation_state: Arc<dyn JoinerActivationStatePort>,
    pub(super) execute_activation: Arc<dyn ExecuteJoinerActivationPort>,
    pub(super) maintenance_wake: Arc<dyn WakeSpaceMembershipMaintenancePort>,
}

impl JoinerAdmissionService {
    pub(crate) fn new(
        settings: Arc<dyn SettingsPort>,
        start_material: Arc<dyn JoinerStartMaterialPort>,
        start_state: Arc<dyn JoinerStartStatePort>,
        prepare_candidate: Arc<dyn PrepareJoinerCandidatePort>,
        prepare_applied: Arc<dyn PrepareJoinerAppliedPort>,
        prepare_activation: Arc<dyn PrepareJoinerActivationPort>,
        activation_state: Arc<dyn JoinerActivationStatePort>,
        execute_activation: Arc<dyn ExecuteJoinerActivationPort>,
        maintenance_wake: Arc<dyn WakeSpaceMembershipMaintenancePort>,
    ) -> Self {
        Self {
            settings,
            start_material,
            start_state,
            prepare_candidate,
            prepare_applied,
            prepare_activation,
            activation_state,
            execute_activation,
            maintenance_wake,
        }
    }
}
