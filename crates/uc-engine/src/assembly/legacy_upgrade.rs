use std::sync::Arc;

use tokio::sync::broadcast;
use uc_application::facade::{
    AutomaticLegacyUpgrade, AutomaticLegacyUpgradeDeps, AutomaticLegacyUpgradeRuntime,
};
use uc_core::membership::{
    LegacyProtectionPort, LegacyUpgradeDispatchPort, LegacyUpgradeEndpointPort,
    MemberRepositoryPort,
};
use uc_core::ports::security::IdentityFingerprintFactoryPort;
use uc_core::ports::{DeviceIdentityPort, PeerAddressRepositoryPort, PresenceEvent};
use uc_infra::network::iroh::{IrohNodeBuilder, IrohNodeError};

pub(super) fn install_automatic_legacy_upgrade(
    builder: &mut IrohNodeBuilder,
    peer_addr_repo: Arc<dyn PeerAddressRepositoryPort>,
    member_repo: Arc<dyn MemberRepositoryPort>,
    fingerprint_factory: Arc<dyn IdentityFingerprintFactoryPort>,
    device_identity: Arc<dyn DeviceIdentityPort>,
    protection: Arc<dyn LegacyProtectionPort>,
    presence_events: broadcast::Receiver<PresenceEvent>,
) -> Result<AutomaticLegacyUpgradeRuntime, IrohNodeError> {
    let adapter = builder.build_legacy_upgrade_adapter(
        peer_addr_repo,
        Arc::clone(&member_repo),
        fingerprint_factory,
    );
    let dispatch: Arc<dyn LegacyUpgradeDispatchPort> = adapter.clone();
    let automatic_upgrade = Arc::new(AutomaticLegacyUpgrade::new(AutomaticLegacyUpgradeDeps {
        member_repo,
        device_identity,
        protection,
        dispatch,
    }));
    let endpoint: Arc<dyn LegacyUpgradeEndpointPort> = automatic_upgrade.clone();
    builder.install_legacy_upgrade_handler(adapter.as_ref(), endpoint)?;
    Ok(automatic_upgrade.start(presence_events))
}
