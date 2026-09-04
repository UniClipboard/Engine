use std::sync::Arc;

use crate::space::membership::WakeSpaceMembershipMaintenancePort;
use uc_core::ports::SettingsPort;

mod activate_complete;
mod cancel_join;
mod handle_candidate;
mod handle_commit;
mod handle_complete;
mod handle_settled;
mod resolve_invitation;
mod start_join;

pub use activate_complete::{
    CompletedJoinerActivation, ExecuteJoinerActivationError, ExecuteJoinerActivationPort,
    JoinerActivationCommitToken, JoinerActivationMutation, JoinerActivationOutcome,
    JoinerActivationStateError, JoinerActivationStatePort, LoadedJoinerActivation,
};
pub use cancel_join::{
    CurrentJoinAdmissionStatePort, JoinerCancellationCommitToken, JoinerCancellationMaterial,
    JoinerCancellationMaterialError, JoinerCancellationMutation, JoinerCancellationStateError,
    LoadedCurrentJoin, PrepareJoinerCancellationPort,
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
pub use resolve_invitation::{ResolveJoinerInvitationError, ResolveJoinerInvitationPort};
pub use start_join::{
    JoinerStartMaterial, JoinerStartMaterialError, JoinerStartMaterialPort, JoinerStartMutation,
    JoinerStartStateError, JoinerStartStatePort, LoadedJoinerStartState,
    PrepareJoinerInvitationError, PrepareJoinerInvitationPort, PreparedJoinerInvitation,
    SpaceAdmissionCommitToken,
};

pub(crate) struct JoinerAdmissionService {
    pub(super) settings: Arc<dyn SettingsPort>,
    pub(super) prepare_invitation: Arc<dyn PrepareJoinerInvitationPort>,
    pub(super) resolve_invitation: Arc<dyn ResolveJoinerInvitationPort>,
    pub(super) start_material: Arc<dyn JoinerStartMaterialPort>,
    pub(super) start_state: Arc<dyn JoinerStartStatePort>,
    pub(super) cancellation_state: Arc<dyn CurrentJoinAdmissionStatePort>,
    pub(super) prepare_cancellation: Arc<dyn PrepareJoinerCancellationPort>,
    pub(super) prepare_candidate: Arc<dyn PrepareJoinerCandidatePort>,
    pub(super) prepare_applied: Arc<dyn PrepareJoinerAppliedPort>,
    pub(super) prepare_activation: Arc<dyn PrepareJoinerActivationPort>,
    pub(super) activation_state: Arc<dyn JoinerActivationStatePort>,
    pub(super) execute_activation: Arc<dyn ExecuteJoinerActivationPort>,
    pub(super) maintenance_wake: Arc<dyn WakeSpaceMembershipMaintenancePort>,
    pub(super) re_pairing: Arc<dyn crate::space::membership::ResolveRePairingPort>,
}

impl JoinerAdmissionService {
    pub(crate) fn new(
        settings: Arc<dyn SettingsPort>,
        prepare_invitation: Arc<dyn PrepareJoinerInvitationPort>,
        resolve_invitation: Arc<dyn ResolveJoinerInvitationPort>,
        start_material: Arc<dyn JoinerStartMaterialPort>,
        start_state: Arc<dyn JoinerStartStatePort>,
        cancellation_state: Arc<dyn CurrentJoinAdmissionStatePort>,
        prepare_cancellation: Arc<dyn PrepareJoinerCancellationPort>,
        prepare_candidate: Arc<dyn PrepareJoinerCandidatePort>,
        prepare_applied: Arc<dyn PrepareJoinerAppliedPort>,
        prepare_activation: Arc<dyn PrepareJoinerActivationPort>,
        activation_state: Arc<dyn JoinerActivationStatePort>,
        execute_activation: Arc<dyn ExecuteJoinerActivationPort>,
        maintenance_wake: Arc<dyn WakeSpaceMembershipMaintenancePort>,
        re_pairing: Arc<dyn crate::space::membership::ResolveRePairingPort>,
    ) -> Self {
        Self {
            settings,
            prepare_invitation,
            resolve_invitation,
            start_material,
            start_state,
            cancellation_state,
            prepare_cancellation,
            prepare_candidate,
            prepare_applied,
            prepare_activation,
            activation_state,
            execute_activation,
            maintenance_wake,
            re_pairing,
        }
    }
}
