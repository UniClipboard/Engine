use std::sync::Arc;

use uc_core::membership::{
    MemberRepositoryPort, RelationshipStateResetPort, RemovalTargetGatePort,
    SpaceSecurityStateResetPort,
};
use uc_core::ports::clipboard::BlobMigrationRepoPort;
use uc_core::ports::pairing::{PairingEventPort, PairingSessionPort};
use uc_core::ports::pairing_invitation::{
    PairingInvitationAddressQueryPort, PairingInvitationByAddressPort, PairingInvitationPort,
};
use uc_core::ports::security::{BlobCipherPort, KeyMigrationPort};
use uc_core::ports::setup::MigrationStatePort;
use uc_core::ports::space::ProofPort;
use uc_core::ports::{
    ClockPort, DeviceIdentityPort, LocalIdentityPort, PeerAddressRepositoryPort, PresencePort,
    SettingsPort, SetupStatusPort,
};
use uc_core::trusted_peer::TrustedPeerRepositoryPort;
use uc_observability_contract::analytics::AnalyticsFacade;

use crate::clipboard_write::MobileConsumableBackfill;
use crate::deps::SpaceAccessPorts;
use crate::space::convergence::WorkspaceConvergence;

pub struct SpaceSessionDeps {
    pub space_access: SpaceAccessPorts,
    pub setup_status: Arc<dyn SetupStatusPort>,
    pub mobile_consumable_backfill: Arc<dyn MobileConsumableBackfill>,
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
    pub removal_gate: Arc<dyn RemovalTargetGatePort>,
    /// The workspace convergence owner behind the admission seam. Always
    /// present: the assembly layer guarantees the owner exists.
    pub workspace_convergence: Arc<WorkspaceConvergence>,
}

pub struct SpaceTransitionDeps {
    pub relationship_reset: Arc<dyn RelationshipStateResetPort>,
    pub space_security_reset: Arc<dyn SpaceSecurityStateResetPort>,
    pub migration_state: Arc<dyn MigrationStatePort>,
    pub key_migration: Arc<dyn KeyMigrationPort>,
    pub blob_migration_repo: Arc<dyn BlobMigrationRepoPort>,
    pub blob_cipher: Arc<dyn BlobCipherPort>,
}

pub struct SpaceFacadeDeps {
    pub session: SpaceSessionDeps,
    pub admission: SpaceAdmissionDeps,
    pub transition: SpaceTransitionDeps,
}
