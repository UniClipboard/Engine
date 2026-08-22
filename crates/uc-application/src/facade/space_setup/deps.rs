use std::sync::Arc;

use uc_core::membership::{
    AdmissionAttemptRepositoryPort, DeviceManagementResetDataPort, MemberRepositoryPort,
    RelationshipStateResetPort, SpaceSecurityStateResetPort,
};
use uc_core::ports::pairing::{PairingEventPort, PairingSessionPort};
use uc_core::ports::pairing_invitation::{
    PairingInvitationAddressQueryPort, PairingInvitationByAddressPort, PairingInvitationPort,
};
use uc_core::ports::space::ProofPort;
use uc_core::ports::{
    ClockPort, DeviceIdentityPort, LocalIdentityPort, PeerAddressRepositoryPort, PresencePort,
    SettingsPort,
};
use uc_core::trusted_peer::TrustedPeerRepositoryPort;
use uc_observability_contract::analytics::AnalyticsFacade;

use crate::clipboard::write::MobileConsumableBackfill;
use crate::deps::{
    CurrentSpaceIdentityPort, InitialSpaceActivationPort, RePairingStateStorePort,
    SpaceAccessPorts, SpaceRebuildProgressPort,
};
use crate::space::assembly::SpaceModules;

pub struct SpaceSessionDeps {
    pub space_access: SpaceAccessPorts,
    pub mobile_consumable_backfill: Arc<dyn MobileConsumableBackfill>,
    pub engine_version_state: Arc<dyn uc_core::ports::EngineVersionStatePort>,
    pub current_engine_version: String,
    pub current_space_identity: Arc<dyn CurrentSpaceIdentityPort>,
    pub initial_space_activation: Arc<dyn InitialSpaceActivationPort>,
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
    pub pairing_session: Arc<dyn PairingSessionPort>,
    pub pairing_events: Arc<dyn PairingEventPort>,
    pub proof_port: Arc<dyn ProofPort>,
    pub trusted_peer_repo: Arc<dyn TrustedPeerRepositoryPort>,
    pub peer_addr_repo: Arc<dyn PeerAddressRepositoryPort>,
    pub presence: Arc<dyn PresencePort>,
    pub analytics: Arc<dyn AnalyticsFacade>,
    /// The assembled space convergence owners behind the admission seam.
    /// Always present: the assembly layer guarantees the owner exists.
    pub convergence: Arc<SpaceModules>,
}

pub struct SpaceTransitionDeps {
    pub admission_attempts: Arc<dyn AdmissionAttemptRepositoryPort>,
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
}
