use std::sync::Arc;

use super::handle_authenticated_message::{
    PrepareSponsorCandidatePort, SponsorJoinRequestStatePort,
};
use super::recover_pending::{
    PendingAdmissionRecoveryStatePort, PrepareJoinerCandidatePort, SpaceAdmissionTransportPort,
};
use super::{JoinerStartMaterialPort, JoinerStartStatePort};
use crate::space::membership::WakeSpaceMembershipMaintenancePort;
use uc_core::ports::SettingsPort;

pub(crate) struct SpaceAdmissionProtocol {
    pub(super) settings: Arc<dyn SettingsPort>,
    pub(super) joiner_start_material: Arc<dyn JoinerStartMaterialPort>,
    pub(super) joiner_start_state: Arc<dyn JoinerStartStatePort>,
    pub(super) pending_admission_recovery_state: Arc<dyn PendingAdmissionRecoveryStatePort>,
    pub(super) space_admission_transport: Arc<dyn SpaceAdmissionTransportPort>,
    pub(super) maintenance_wake: Arc<dyn WakeSpaceMembershipMaintenancePort>,
    pub(super) sponsor_join_request_state: Arc<dyn SponsorJoinRequestStatePort>,
    pub(super) prepare_sponsor_candidate: Arc<dyn PrepareSponsorCandidatePort>,
    pub(super) prepare_joiner_candidate: Arc<dyn PrepareJoinerCandidatePort>,
    pub(super) execution_lock: tokio::sync::Mutex<()>,
}

impl SpaceAdmissionProtocol {
    pub(crate) fn new(
        settings: Arc<dyn SettingsPort>,
        joiner_start_material: Arc<dyn JoinerStartMaterialPort>,
        joiner_start_state: Arc<dyn JoinerStartStatePort>,
        pending_admission_recovery_state: Arc<dyn PendingAdmissionRecoveryStatePort>,
        space_admission_transport: Arc<dyn SpaceAdmissionTransportPort>,
        maintenance_wake: Arc<dyn WakeSpaceMembershipMaintenancePort>,
        sponsor_join_request_state: Arc<dyn SponsorJoinRequestStatePort>,
        prepare_sponsor_candidate: Arc<dyn PrepareSponsorCandidatePort>,
        prepare_joiner_candidate: Arc<dyn PrepareJoinerCandidatePort>,
    ) -> Self {
        Self {
            settings,
            joiner_start_material,
            joiner_start_state,
            pending_admission_recovery_state,
            space_admission_transport,
            maintenance_wake,
            sponsor_join_request_state,
            prepare_sponsor_candidate,
            prepare_joiner_candidate,
            execution_lock: tokio::sync::Mutex::new(()),
        }
    }
}
