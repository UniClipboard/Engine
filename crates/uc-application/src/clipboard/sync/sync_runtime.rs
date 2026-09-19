//! Owns automatic clipboard delivery after local capture and peer recovery.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

mod recovery;
mod shutdown;

use recovery::{OfflineDeliveryRecovery, OfflineDeliveryRecoveryDeps, RecoveryDeliveryPort};

use tracing::warn;

use uc_core::ids::{DeviceId, EntryId};
use uc_core::ports::clipboard::{ClipboardEventRepositoryPort, ListClipboardEntriesPort};
use uc_core::ports::{
    ClockPort, DeviceIdentityPort, EntryDeliveryRepositoryPort, PeerAddressRepositoryPort,
    PeerReachabilityPort, SettingsPort,
};
use uc_observability_contract::diagnostics::connectivity::{
    LocalWorkObservation, LocalWorkOutcome, LocalWorkStep,
};

use crate::clipboard::inbound::ClipboardInboundRuntime;
use crate::clipboard::outbound::{
    ClipboardOutboundError, ClipboardOutboundFacade, ClipboardOutboundInput,
    ClipboardOutboundOutcome, ClipboardOutboundPort,
};
use crate::deps::CurrentSpaceMemberScopePort;
use crate::runtime_lifecycle::LifecycleError;

/// 自动出站的完整生命周期。调用方只提交本地捕获；手动重发使用独立
/// facade，离线恢复保持为本运行期的内部责任。
pub struct ClipboardSyncRuntime {
    outbound: Arc<dyn ClipboardOutboundPort>,
    settings: Arc<dyn SettingsPort>,
    inbound: tokio::sync::Mutex<Option<ClipboardInboundRuntime>>,
    delivery_gate: Arc<tokio::sync::Mutex<()>>,
    recovery: OfflineDeliveryRecovery,
    stopping: AtomicBool,
    shutdown_result: tokio::sync::Mutex<Option<Result<(), Arc<LifecycleError>>>>,
}

pub struct ClipboardSyncRuntimeDeps {
    pub outbound: Arc<ClipboardOutboundFacade>,
    pub settings: Arc<dyn SettingsPort>,
    pub inbound: ClipboardInboundRuntime,
    pub peer_reachability: Arc<dyn PeerReachabilityPort>,
    pub known_peers: Arc<dyn PeerAddressRepositoryPort>,
    pub member_scope: Arc<dyn CurrentSpaceMemberScopePort>,
    pub entries: Arc<dyn ListClipboardEntriesPort>,
    pub events: Arc<dyn ClipboardEventRepositoryPort>,
    pub deliveries: Arc<dyn EntryDeliveryRepositoryPort>,
    pub device_identity: Arc<dyn DeviceIdentityPort>,
    pub clock: Arc<dyn ClockPort>,
}

impl ClipboardSyncRuntime {
    pub fn start(deps: ClipboardSyncRuntimeDeps) -> Self {
        let delivery_gate = Arc::new(tokio::sync::Mutex::new(()));
        let recovery = OfflineDeliveryRecovery::start(OfflineDeliveryRecoveryDeps {
            peer_reachability: deps.peer_reachability,
            known_peers: deps.known_peers,
            member_scope: deps.member_scope,
            settings: Arc::clone(&deps.settings),
            entries: deps.entries,
            events: deps.events,
            deliveries: deps.deliveries,
            device_identity: deps.device_identity,
            clock: deps.clock,
            delivery: Arc::clone(&deps.outbound) as Arc<dyn RecoveryDeliveryPort>,
            delivery_gate: Arc::clone(&delivery_gate),
        });
        Self {
            outbound: deps.outbound,
            settings: deps.settings,
            inbound: tokio::sync::Mutex::new(Some(deps.inbound)),
            delivery_gate,
            recovery,
            stopping: AtomicBool::new(false),
            shutdown_result: tokio::sync::Mutex::new(None),
        }
    }

    /// Sends a local capture through the complete automatic-delivery
    /// lifecycle while allowing an explicit caller to narrow its targets.
    pub async fn dispatch_local_capture_to_targets(
        &self,
        input: ClipboardOutboundInput,
        target_filter: Option<Vec<DeviceId>>,
    ) -> Result<ClipboardOutboundOutcome, ClipboardOutboundError> {
        let gate_observation =
            LocalWorkObservation::begin(LocalWorkStep::ClipboardDeliveryGateWait);
        let _gate = self.delivery_gate.lock().await;
        gate_observation.finish(LocalWorkOutcome::Ok);
        if self.stopping.load(Ordering::Acquire) {
            return Ok(ClipboardOutboundOutcome::Skipped {
                reason: "runtime_stopped".to_owned(),
            });
        }
        if !automatic_sync_enabled(self.settings.as_ref()).await {
            return Ok(ClipboardOutboundOutcome::Skipped {
                reason: "automatic_sync_disabled".to_string(),
            });
        }
        let entry_id = EntryId::from(input.entry_id.as_str());
        let outcome = self.outbound.dispatch_capture(input, target_filter).await?;
        if let ClipboardOutboundOutcome::Dispatched {
            per_target,
            pending_targets,
            ..
        } = &outcome
        {
            let mut targets: Vec<DeviceId> =
                per_target.iter().map(|target| target.device_id).collect();
            targets.extend(pending_targets.iter().copied());
            self.recovery
                .supersede_older_recoverable_entries(&entry_id, &targets)
                .await;
        }
        Ok(outcome)
    }
}

async fn automatic_sync_enabled(settings: &dyn SettingsPort) -> bool {
    let observation = LocalWorkObservation::begin(LocalWorkStep::ClipboardSyncSettingsLoad);
    let result = settings.load().await;
    observation.finish(if result.is_ok() {
        LocalWorkOutcome::Ok
    } else {
        LocalWorkOutcome::Error
    });
    match result {
        Ok(settings) => settings.sync.sync_enabled && settings.sync.auto_sync_enabled,
        Err(_) => {
            warn!(
                error_kind = "settings_load",
                "clipboard sync: automatic delivery skipped"
            );
            false
        }
    }
}
