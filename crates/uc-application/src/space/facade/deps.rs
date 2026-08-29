use std::sync::Arc;
use tokio::sync::broadcast;

use uc_core::membership::{
    MemberRepositoryPort, RelationshipStateResetPort, SpaceSecurityStateResetPort,
};
use uc_core::ports::pairing_invitation::{
    PairingInvitationAddressQueryPort, PairingInvitationByAddressPort, PairingInvitationPort,
};
use uc_core::ports::{
    ClockPort, DeviceIdentityPort, LocalIdentityPort, PeerReachabilityPort, SettingsPort,
};
use uc_observability_contract::analytics::AnalyticsFacade;

use crate::clipboard::write::MobileConsumableBackfill;
use crate::deps::DeviceManagementResetDataPort;
use crate::deps::{
    CurrentSpaceIdentityPort, InitialSpaceActivationPort, RePairingStateStorePort,
    SpaceAccessPorts, SpaceRebuildProgressPort,
};
use crate::space::application::SpaceApplicationDeps;

pub struct SpaceSessionDeps {
    pub space_access: SpaceAccessPorts,
    pub mobile_consumable_backfill: Arc<dyn MobileConsumableBackfill>,
    pub engine_version_state: Arc<dyn uc_core::ports::EngineVersionStatePort>,
    pub current_engine_version: String,
    pub current_space_identity: Arc<dyn CurrentSpaceIdentityPort>,
    pub initial_space_activation: Arc<dyn InitialSpaceActivationPort>,
    pub admission_credentials: Arc<dyn crate::deps::PrepareSpaceAdmissionCredentialsPort>,
    pub activity: Arc<dyn crate::space::lifecycle::SpaceSessionActivityPort>,
}

pub struct SpaceAdmissionDeps {
    pub local_identity: Arc<dyn LocalIdentityPort>,
    pub device_identity: Arc<dyn DeviceIdentityPort>,
    pub member_repo: Arc<dyn MemberRepositoryPort>,
    pub settings: Arc<dyn SettingsPort>,
    pub clock: Arc<dyn ClockPort>,
    pub pairing_invitation: Arc<dyn PairingInvitationPort>,
    pub pairing_invitation_addresses: Arc<dyn PairingInvitationAddressQueryPort>,
    pub pairing_invitation_by_address: Arc<dyn PairingInvitationByAddressPort>,
    pub presence: Arc<dyn PeerReachabilityPort>,
    pub analytics: Arc<dyn AnalyticsFacade>,
    pub connection_channel: Option<Arc<dyn uc_core::ports::ConnectionChannelPort>>,
}

pub struct SpaceTransitionDeps {
    pub device_management_reset_data: Arc<dyn DeviceManagementResetDataPort>,
    pub relationship_reset: Arc<dyn RelationshipStateResetPort>,
    pub space_security_reset: Arc<dyn SpaceSecurityStateResetPort>,
    pub space_rebuild_progress: Arc<dyn SpaceRebuildProgressPort>,
    pub re_pairing_state_store: Arc<dyn RePairingStateStorePort>,
}

pub struct SpaceFacadeDeps {
    pub session: SpaceSessionDeps,
    pub admission: SpaceAdmissionDeps,
    pub transition: SpaceTransitionDeps,
    pub application: SpaceApplicationDeps,
    pub peer_reachability_changed_events:
        broadcast::Receiver<uc_core::ports::PeerReachabilityChanged>,
}
