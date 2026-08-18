//! Legacy-upgrade workflow tests.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

use super::{
    select_legacy_upgrade_candidates, AutomaticLegacyUpgrade, AutomaticLegacyUpgradeDeps,
    AutomaticLegacyUpgradeRuntime, LegacyDiscoveryPhase, LegacyUpgradePassOutcome,
};
use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::broadcast;
use uc_core::ids::DeviceId;
use uc_core::membership::{
    CurrentWorkspaceLocalMembership, CurrentWorkspacePeerScopeError, CurrentWorkspacePeerScopePort,
    CurrentWorkspacePeerScopeSource, CurrentWorkspacePeerSnapshot, LegacyProtectionCommand,
    LegacyProtectionPort, LegacyProtectionResult, LegacyProtectionSnapshot,
    LegacyRequestInspection, LegacyUpgradeDescriptor, LegacyUpgradeDispatchError,
    LegacyUpgradeDispatchPort, LegacyUpgradeEndpointPort, LegacyUpgradeError, LegacyUpgradeId,
    LegacyUpgradeRequest, LegacyUpgradeResponse, MemberRepositoryPort, MembershipError,
    ProtectionGroupAdmission, ProtectionGroupId, SpaceMember,
};
use uc_core::ports::{
    DeviceIdentityPort, PresenceError, PresenceEvent, PresencePort, ReachabilityState,
};
use uc_core::space_access::GroupAdmission;

#[derive(Clone, Default)]
struct CapturedWriter(Arc<Mutex<Vec<u8>>>);

impl CapturedWriter {
    fn dump(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

impl Write for CapturedWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedWriter {
    type Writer = CapturedWriter;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

struct ScenarioMembers {
    members: Vec<SpaceMember>,
}

#[async_trait]
impl MemberRepositoryPort for ScenarioMembers {
    async fn get(&self, device_id: &DeviceId) -> Result<Option<SpaceMember>, MembershipError> {
        Ok(self
            .members
            .iter()
            .find(|member| &member.device_id == device_id)
            .cloned())
    }

    async fn list(&self) -> Result<Vec<SpaceMember>, MembershipError> {
        Ok(self.members.clone())
    }

    async fn save(&self, _member: &SpaceMember) -> Result<(), MembershipError> {
        panic!("automatic upgrade must not mutate membership")
    }

    async fn remove(&self, _device_id: &DeviceId) -> Result<bool, MembershipError> {
        panic!("automatic upgrade must not mutate membership")
    }
}

struct ScenarioIdentity(DeviceId);

impl DeviceIdentityPort for ScenarioIdentity {
    fn current_device_id(&self) -> DeviceId {
        self.0
    }
}

struct ScenarioPeerScope {
    source: CurrentWorkspacePeerScopeSource,
    peers: Vec<DeviceId>,
}

#[async_trait]
impl CurrentWorkspacePeerScopePort for ScenarioPeerScope {
    async fn snapshot(
        &self,
    ) -> Result<CurrentWorkspacePeerSnapshot, CurrentWorkspacePeerScopeError> {
        Ok(CurrentWorkspacePeerSnapshot {
            revision: 1,
            source: self.source,
            local_membership: CurrentWorkspaceLocalMembership::Active,
            peer_device_ids: self.peers.clone(),
        })
    }
}

struct ScenarioProtection {
    descriptor: Mutex<LegacyUpgradeDescriptor>,
    bootstrap_group_id: Mutex<ProtectionGroupId>,
    protected: Mutex<HashSet<DeviceId>>,
    pending_readmission: Mutex<HashSet<DeviceId>>,
    cached: Mutex<Vec<(LegacyUpgradeRequest, ProtectionGroupAdmission)>>,
    snapshot_calls: AtomicUsize,
    inspection_calls: AtomicUsize,
    admission_calls: AtomicUsize,
    snapshot_error: AtomicUsize,
}

impl ScenarioProtection {
    fn legacy() -> Self {
        Self {
            descriptor: Mutex::new(LegacyUpgradeDescriptor::legacy(
                LegacyUpgradeId::from_bytes([1; 32]),
            )),
            bootstrap_group_id: Mutex::new(group("unconfigured-group")),
            protected: Mutex::new(HashSet::new()),
            pending_readmission: Mutex::new(HashSet::new()),
            cached: Mutex::new(Vec::new()),
            snapshot_calls: AtomicUsize::new(0),
            inspection_calls: AtomicUsize::new(0),
            admission_calls: AtomicUsize::new(0),
            snapshot_error: AtomicUsize::new(0),
        }
    }

    fn set_group(&self, group_id: &str) {
        *self.descriptor.lock().unwrap() =
            LegacyUpgradeDescriptor::ready(LegacyUpgradeId::from_bytes([1; 32]), group(group_id));
    }

    fn set_bootstrap_group(&self, group_id: &str) {
        *self.bootstrap_group_id.lock().unwrap() = group(group_id);
    }

    fn set_awaiting_readmission(&self, device_id: DeviceId) {
        self.pending_readmission.lock().unwrap().insert(device_id);
    }

    fn group_id(&self) -> Option<ProtectionGroupId> {
        self.descriptor
            .lock()
            .unwrap()
            .protection_group_id()
            .cloned()
    }

    fn fail_snapshot(&self) {
        self.snapshot_error.store(1, Ordering::Release);
    }
}

#[async_trait]
impl LegacyProtectionPort for ScenarioProtection {
    async fn snapshot(
        &self,
        member_ids: &[DeviceId],
    ) -> Result<LegacyProtectionSnapshot, LegacyUpgradeError> {
        self.snapshot_calls.fetch_add(1, Ordering::AcqRel);
        if self.snapshot_error.load(Ordering::Acquire) != 0 {
            return Err(LegacyUpgradeError::Internal(
                "simulated protection snapshot failure".into(),
            ));
        }
        let protected = self.protected.lock().unwrap();
        let pending_readmission = self.pending_readmission.lock().unwrap();
        Ok(LegacyProtectionSnapshot {
            descriptor: self.descriptor.lock().unwrap().clone(),
            protected_members: member_ids
                .iter()
                .filter(|device_id| protected.contains(device_id))
                .copied()
                .collect(),
            pending_readmission_members: member_ids
                .iter()
                .filter(|device_id| pending_readmission.contains(device_id))
                .copied()
                .collect(),
        })
    }

    async fn begin_attempt(
        &self,
        source_device_id: &DeviceId,
        target_device_id: &DeviceId,
    ) -> Result<LegacyUpgradeRequest, LegacyUpgradeError> {
        let key_package = format!("{}:{}", source_device_id, target_device_id).into_bytes();
        Ok(LegacyUpgradeRequest::unsigned(
            *source_device_id,
            *target_device_id,
            self.descriptor.lock().unwrap().clone(),
            key_package,
        )
        .with_proof(vec![9]))
    }

    async fn begin_readmission_confirmation(
        &self,
        source_device_id: &DeviceId,
        target_device_id: &DeviceId,
    ) -> Result<LegacyUpgradeRequest, LegacyUpgradeError> {
        Ok(LegacyUpgradeRequest::readmission_confirmation(
            *source_device_id,
            *target_device_id,
            self.descriptor.lock().unwrap().clone(),
        )
        .with_proof(vec![9]))
    }

    async fn begin_readmission_probe(
        &self,
        source_device_id: &DeviceId,
        target_device_id: &DeviceId,
    ) -> Result<LegacyUpgradeRequest, LegacyUpgradeError> {
        Ok(LegacyUpgradeRequest::readmission_probe(
            *source_device_id,
            *target_device_id,
            self.descriptor.lock().unwrap().clone(),
        )
        .with_proof(vec![9]))
    }

    async fn inspect_request(
        &self,
        request: &LegacyUpgradeRequest,
    ) -> Result<LegacyRequestInspection, LegacyUpgradeError> {
        self.inspection_calls.fetch_add(1, Ordering::AcqRel);
        if request.descriptor().upgrade_id() != LegacyUpgradeId::from_bytes([1; 32]) {
            return Ok(LegacyRequestInspection::Invalid);
        }
        Ok(
            match self
                .cached
                .lock()
                .unwrap()
                .iter()
                .find(|(cached_request, _)| cached_request == request)
                .map(|(_, admission)| admission.clone())
            {
                Some(admission) => LegacyRequestInspection::Replay(admission),
                None => LegacyRequestInspection::Verified,
            },
        )
    }

    async fn execute(
        &self,
        command: LegacyProtectionCommand,
    ) -> Result<LegacyProtectionResult, LegacyUpgradeError> {
        match command {
            LegacyProtectionCommand::CreateGroup { .. } => {
                let group_id = self.bootstrap_group_id.lock().unwrap().clone();
                *self.descriptor.lock().unwrap() =
                    LegacyUpgradeDescriptor::ready(LegacyUpgradeId::from_bytes([1; 32]), group_id);
                Ok(LegacyProtectionResult::GroupReady(
                    self.descriptor.lock().unwrap().clone(),
                ))
            }
            LegacyProtectionCommand::JoinGroup { admission, .. } => {
                *self.descriptor.lock().unwrap() = LegacyUpgradeDescriptor::ready(
                    LegacyUpgradeId::from_bytes([1; 32]),
                    admission.protection_group_id,
                );
                Ok(LegacyProtectionResult::GroupReady(
                    self.descriptor.lock().unwrap().clone(),
                ))
            }
            LegacyProtectionCommand::AdmitMember { request, .. } => {
                self.admission_calls.fetch_add(1, Ordering::AcqRel);
                self.protected
                    .lock()
                    .unwrap()
                    .insert(*request.source_device_id());
                let group_id = self.group_id().ok_or(LegacyUpgradeError::Unavailable)?;
                let admission = ProtectionGroupAdmission {
                    protection_group_id: group_id,
                    admission: GroupAdmission {
                        welcome: vec![1],
                        encrypted_key_catalog: vec![2],
                        existing_member_updates: Vec::new(),
                        group_epoch: 2,
                    },
                };
                self.cached
                    .lock()
                    .unwrap()
                    .push((request, admission.clone()));
                Ok(LegacyProtectionResult::MemberAdmitted(admission))
            }
            LegacyProtectionCommand::AcknowledgeReadmission { member } => {
                self.pending_readmission.lock().unwrap().remove(&member);
                Ok(LegacyProtectionResult::GroupReady(
                    self.descriptor.lock().unwrap().clone(),
                ))
            }
        }
    }
}

#[test]
fn candidate_selection_separates_legacy_roster_from_current_history_readmissions() {
    let legacy_peer = DeviceId::new("device-b");
    let pending_peer = DeviceId::new("device-c");

    assert_eq!(
        select_legacy_upgrade_candidates(
            CurrentWorkspacePeerScopeSource::Legacy,
            vec![legacy_peer],
            vec![pending_peer],
        ),
        vec![legacy_peer]
    );
    assert_eq!(
        select_legacy_upgrade_candidates(
            CurrentWorkspacePeerScopeSource::CurrentHistory,
            vec![legacy_peer],
            vec![pending_peer],
        ),
        vec![pending_peer]
    );
}

#[derive(Default)]
struct ScenarioNetwork {
    endpoints: Mutex<HashMap<DeviceId, Arc<AutomaticLegacyUpgrade>>>,
    exchange_calls: AtomicUsize,
    reconnects: Mutex<Vec<DeviceId>>,
}

#[async_trait]
impl LegacyUpgradeDispatchPort for ScenarioNetwork {
    async fn exchange_legacy_upgrade(
        &self,
        peer: &DeviceId,
        request: &LegacyUpgradeRequest,
    ) -> Result<LegacyUpgradeResponse, LegacyUpgradeDispatchError> {
        self.exchange_calls.fetch_add(1, Ordering::AcqRel);
        assert_eq!(peer, request.target_device_id());
        let endpoint = self
            .endpoints
            .lock()
            .unwrap()
            .get(peer)
            .cloned()
            .ok_or(LegacyUpgradeDispatchError::Offline)?;
        endpoint
            .handle_legacy_upgrade_request(request.source_device_id(), request.clone())
            .await
            .map_err(|_| LegacyUpgradeDispatchError::Rejected)
    }
}

#[async_trait]
impl PresencePort for ScenarioNetwork {
    async fn ensure_reachable(
        &self,
        device: &DeviceId,
    ) -> Result<ReachabilityState, PresenceError> {
        self.reconnects.lock().unwrap().push(*device);
        Ok(ReachabilityState::Online)
    }

    async fn current_state(&self, _device: &DeviceId) -> ReachabilityState {
        ReachabilityState::Unknown
    }

    fn subscribe(&self) -> broadcast::Receiver<PresenceEvent> {
        broadcast::channel(1).1
    }
}

struct UpgradeWorld {
    network: Arc<ScenarioNetwork>,
    devices: HashMap<DeviceId, Arc<AutomaticLegacyUpgrade>>,
    protection: HashMap<DeviceId, Arc<ScenarioProtection>>,
}

impl UpgradeWorld {
    fn new(device_ids: &[&str]) -> Self {
        let network = Arc::new(ScenarioNetwork::default());
        let ids = device_ids
            .iter()
            .map(|device_id| DeviceId::new(*device_id))
            .collect::<Vec<_>>();
        let protection = ids
            .iter()
            .map(|device_id| (*device_id, Arc::new(ScenarioProtection::legacy())))
            .collect::<HashMap<_, _>>();
        let devices = ids
            .iter()
            .map(|device_id| {
                let members = ids
                    .iter()
                    .filter(|member_id| *member_id != device_id)
                    .map(|member_id| scenario_member(*member_id))
                    .collect();
                let automatic_upgrade = Arc::new(
                    AutomaticLegacyUpgrade::new(AutomaticLegacyUpgradeDeps {
                        member_repo: Arc::new(ScenarioMembers { members }),
                        device_identity: Arc::new(ScenarioIdentity(*device_id)),
                        protection: protection.get(device_id).unwrap().clone(),
                        dispatch: network.clone(),
                        presence: network.clone(),
                    })
                    .with_peer_scope(Arc::new(ScenarioPeerScope {
                        source: CurrentWorkspacePeerScopeSource::Legacy,
                        peers: ids
                            .iter()
                            .filter(|peer| *peer != device_id)
                            .copied()
                            .collect(),
                    })),
                );
                (*device_id, automatic_upgrade)
            })
            .collect();
        Self {
            network,
            devices,
            protection,
        }
    }

    fn device(&self, device_id: &str) -> Arc<AutomaticLegacyUpgrade> {
        self.devices.get(&DeviceId::new(device_id)).unwrap().clone()
    }

    fn protection(&self, device_id: &str) -> Arc<ScenarioProtection> {
        self.protection
            .get(&DeviceId::new(device_id))
            .unwrap()
            .clone()
    }

    fn publish_devices(&self) {
        self.network.endpoints.lock().unwrap().extend(
            self.devices
                .iter()
                .map(|(id, device)| (*id, device.clone())),
        );
    }

    async fn reconcile(&self, device_id: &str, phase: LegacyDiscoveryPhase) {
        self.device(device_id).reconcile_once(phase).await.unwrap();
    }
}

fn group(group_id: &str) -> ProtectionGroupId {
    ProtectionGroupId::from_string(group_id).unwrap()
}

fn scenario_member(device_id: DeviceId) -> SpaceMember {
    SpaceMember {
        device_id,
        device_name: device_id.as_str().to_owned(),
        identity_fingerprint: uc_core::security::IdentityFingerprint::from_raw_string(
            "AAAAAAAAAAAAAAAA",
        )
        .unwrap(),
        joined_at: Utc::now(),
        sync_preferences: Default::default(),
    }
}

fn legacy_request(source: &str, target: &str) -> LegacyUpgradeRequest {
    LegacyUpgradeRequest::unsigned(
        DeviceId::new(source),
        DeviceId::new(target),
        LegacyUpgradeDescriptor::legacy(LegacyUpgradeId::from_bytes([1; 32])),
        vec![1],
    )
    .with_proof(vec![2])
}

async fn wait_for_snapshot_calls(protection: &ScenarioProtection, expected: usize) {
    for _ in 0..100 {
        if protection.snapshot_calls.load(Ordering::Acquire) >= expected {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("automatic upgrade pass did not finish")
}

#[tokio::test]
async fn unknown_or_removed_devices_cannot_enter_the_upgrade_channel() {
    let world = UpgradeWorld::new(&["device-a"]);
    let protection = world.protection("device-a");
    let request = LegacyUpgradeRequest::unsigned(
        DeviceId::new("removed-device"),
        DeviceId::new("device-a"),
        LegacyUpgradeDescriptor::ready(LegacyUpgradeId::from_bytes([1; 32]), group("group-a")),
        vec![1],
    );

    assert_eq!(
        world
            .device("device-a")
            .handle_legacy_upgrade_request(&DeviceId::new("removed-device"), request)
            .await
            .unwrap_err(),
        LegacyUpgradeError::Unauthorized
    );
    assert_eq!(protection.inspection_calls.load(Ordering::Acquire), 0);
}

#[tokio::test]
async fn a_legacy_device_automatically_installs_a_ready_peers_admission() {
    let world = UpgradeWorld::new(&["device-a", "device-b"]);
    world.protection("device-a").set_group("group-a");
    world.publish_devices();

    world
        .reconcile("device-b", LegacyDiscoveryPhase::Discovering)
        .await;

    assert_eq!(
        world.protection("device-b").group_id().as_ref(),
        Some(&group("group-a"))
    );
}

#[tokio::test]
async fn joining_a_ready_group_reconnects_the_sponsor_immediately() {
    let world = UpgradeWorld::new(&["device-a", "device-b"]);
    world.protection("device-a").set_group("group-a");
    world.publish_devices();

    world
        .reconcile("device-b", LegacyDiscoveryPhase::Discovering)
        .await;

    assert_eq!(
        *world.network.reconnects.lock().unwrap(),
        vec![DeviceId::new("device-a")]
    );
}

#[tokio::test]
async fn current_history_continues_only_persisted_legacy_readmissions() {
    let local_device_id = DeviceId::new("device-a");
    let pending_device_id = DeviceId::new("device-b");
    let stale_device_id = DeviceId::new("device-c");
    let protection = Arc::new(ScenarioProtection::legacy());
    protection.set_group("group-a");
    protection.set_awaiting_readmission(pending_device_id);
    let network = Arc::new(ScenarioNetwork::default());
    let automatic_upgrade = AutomaticLegacyUpgrade::new(AutomaticLegacyUpgradeDeps {
        member_repo: Arc::new(ScenarioMembers {
            members: vec![
                scenario_member(pending_device_id),
                scenario_member(stale_device_id),
            ],
        }),
        device_identity: Arc::new(ScenarioIdentity(local_device_id)),
        protection,
        dispatch: network.clone(),
        presence: network.clone(),
    })
    .with_peer_scope(Arc::new(ScenarioPeerScope {
        source: CurrentWorkspacePeerScopeSource::CurrentHistory,
        peers: Vec::new(),
    }));

    automatic_upgrade
        .reconcile_once(LegacyDiscoveryPhase::Discovering)
        .await
        .unwrap();

    assert_eq!(network.exchange_calls.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn current_history_without_pending_readmissions_finishes_without_network_work() {
    let local_device_id = DeviceId::new("device-a");
    let stale_device_id = DeviceId::new("device-b");
    let protection = Arc::new(ScenarioProtection::legacy());
    let network = Arc::new(ScenarioNetwork::default());
    let automatic_upgrade = AutomaticLegacyUpgrade::new(AutomaticLegacyUpgradeDeps {
        member_repo: Arc::new(ScenarioMembers {
            members: vec![scenario_member(stale_device_id)],
        }),
        device_identity: Arc::new(ScenarioIdentity(local_device_id)),
        protection: protection.clone(),
        dispatch: network.clone(),
        presence: network.clone(),
    })
    .with_peer_scope(Arc::new(ScenarioPeerScope {
        source: CurrentWorkspacePeerScopeSource::CurrentHistory,
        peers: Vec::new(),
    }));

    let outcome = automatic_upgrade
        .reconcile_once(LegacyDiscoveryPhase::Discovering)
        .await
        .unwrap();

    assert_eq!(outcome, LegacyUpgradePassOutcome::ready(false));
    assert_eq!(network.exchange_calls.load(Ordering::Acquire), 0);
    assert_eq!(protection.group_id(), None);
}

#[tokio::test]
async fn current_history_does_not_fall_back_when_the_readmission_snapshot_fails() {
    let local_device_id = DeviceId::new("device-a");
    let stale_device_id = DeviceId::new("device-b");
    let protection = Arc::new(ScenarioProtection::legacy());
    protection.fail_snapshot();
    let network = Arc::new(ScenarioNetwork::default());
    let automatic_upgrade = AutomaticLegacyUpgrade::new(AutomaticLegacyUpgradeDeps {
        member_repo: Arc::new(ScenarioMembers {
            members: vec![scenario_member(stale_device_id)],
        }),
        device_identity: Arc::new(ScenarioIdentity(local_device_id)),
        protection,
        dispatch: network.clone(),
        presence: network.clone(),
    })
    .with_peer_scope(Arc::new(ScenarioPeerScope {
        source: CurrentWorkspacePeerScopeSource::CurrentHistory,
        peers: Vec::new(),
    }));

    assert!(automatic_upgrade
        .reconcile_once(LegacyDiscoveryPhase::Discovering)
        .await
        .is_err());
    assert_eq!(network.exchange_calls.load(Ordering::Acquire), 0);
}

#[tokio::test]
async fn a_current_device_admits_a_known_legacy_member() {
    let local_device_id = DeviceId::new("device-a");
    let remote_device_id = DeviceId::new("device-b");
    let protection = Arc::new(ScenarioProtection::legacy());
    protection.set_group("group-a");
    let automatic_upgrade = AutomaticLegacyUpgrade::new(AutomaticLegacyUpgradeDeps {
        member_repo: Arc::new(ScenarioMembers {
            members: vec![scenario_member(remote_device_id)],
        }),
        device_identity: Arc::new(ScenarioIdentity(local_device_id)),
        protection: protection.clone(),
        dispatch: Arc::new(ScenarioNetwork::default()),
        presence: Arc::new(ScenarioNetwork::default()),
    })
    .with_peer_scope(Arc::new(ScenarioPeerScope {
        source: CurrentWorkspacePeerScopeSource::CurrentHistory,
        peers: vec![remote_device_id],
    }));

    let response = automatic_upgrade
        .handle_legacy_upgrade_request(&remote_device_id, legacy_request("device-b", "device-a"))
        .await
        .unwrap();

    assert!(matches!(
        response.kind,
        uc_core::membership::LegacyUpgradeResponseKind::Admission(_)
    ));
    assert_eq!(protection.admission_calls.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn a_current_device_admits_a_legacy_member_awaiting_readmission() {
    let local_device_id = DeviceId::new("device-a");
    let remote_device_id = DeviceId::new("device-b");
    let protection = Arc::new(ScenarioProtection::legacy());
    protection.set_group("group-a");
    protection.set_awaiting_readmission(remote_device_id);
    let automatic_upgrade = AutomaticLegacyUpgrade::new(AutomaticLegacyUpgradeDeps {
        member_repo: Arc::new(ScenarioMembers {
            members: vec![scenario_member(remote_device_id)],
        }),
        device_identity: Arc::new(ScenarioIdentity(local_device_id)),
        protection: protection.clone(),
        dispatch: Arc::new(ScenarioNetwork::default()),
        presence: Arc::new(ScenarioNetwork::default()),
    })
    .with_peer_scope(Arc::new(ScenarioPeerScope {
        source: CurrentWorkspacePeerScopeSource::CurrentHistory,
        peers: vec![],
    }));

    let response = automatic_upgrade
        .handle_legacy_upgrade_request(&remote_device_id, legacy_request("device-b", "device-a"))
        .await
        .unwrap();

    assert!(matches!(
        response.kind,
        uc_core::membership::LegacyUpgradeResponseKind::Admission(_)
    ));
    assert_eq!(protection.admission_calls.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn a_known_legacy_member_without_current_or_readmission_status_is_rejected() {
    let local_device_id = DeviceId::new("device-a");
    let remote_device_id = DeviceId::new("device-b");
    let protection = Arc::new(ScenarioProtection::legacy());
    protection.set_group("group-a");
    let automatic_upgrade = AutomaticLegacyUpgrade::new(AutomaticLegacyUpgradeDeps {
        member_repo: Arc::new(ScenarioMembers {
            members: vec![scenario_member(remote_device_id)],
        }),
        device_identity: Arc::new(ScenarioIdentity(local_device_id)),
        protection: protection.clone(),
        dispatch: Arc::new(ScenarioNetwork::default()),
        presence: Arc::new(ScenarioNetwork::default()),
    })
    .with_peer_scope(Arc::new(ScenarioPeerScope {
        source: CurrentWorkspacePeerScopeSource::CurrentHistory,
        peers: vec![],
    }));

    assert_eq!(
        automatic_upgrade
            .handle_legacy_upgrade_request(
                &remote_device_id,
                legacy_request("device-b", "device-a"),
            )
            .await
            .unwrap_err(),
        LegacyUpgradeError::Unauthorized
    );
    assert_eq!(protection.admission_calls.load(Ordering::Acquire), 0);
}

#[tokio::test]
async fn a_lost_admission_response_is_replayed_for_the_exact_retry() {
    let world = UpgradeWorld::new(&["device-a", "device-b"]);
    let sponsor = world.protection("device-a");
    sponsor.set_group("group-a");
    let request = legacy_request("device-b", "device-a");

    let first = world
        .device("device-a")
        .handle_legacy_upgrade_request(&DeviceId::new("device-b"), request.clone())
        .await
        .unwrap();
    let retried = world
        .device("device-a")
        .handle_legacy_upgrade_request(&DeviceId::new("device-b"), request)
        .await
        .unwrap();

    assert_eq!(retried, first);
    assert_eq!(sponsor.admission_calls.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn completed_legacy_readmission_is_acknowledged_only_after_the_joiner_confirms() {
    let world = UpgradeWorld::new(&["device-a", "device-b"]);
    let sponsor = world.protection("device-a");
    sponsor.set_group("group-a");
    sponsor.set_awaiting_readmission(DeviceId::new("device-b"));

    let admission = world
        .device("device-a")
        .handle_legacy_upgrade_request(
            &DeviceId::new("device-b"),
            legacy_request("device-b", "device-a"),
        )
        .await
        .unwrap();
    assert!(matches!(
        admission.kind,
        uc_core::membership::LegacyUpgradeResponseKind::Admission(_)
    ));
    assert!(sponsor
        .pending_readmission
        .lock()
        .unwrap()
        .contains(&DeviceId::new("device-b")));

    let confirmation = LegacyUpgradeRequest::readmission_confirmation(
        DeviceId::new("device-b"),
        DeviceId::new("device-a"),
        LegacyUpgradeDescriptor::ready(LegacyUpgradeId::from_bytes([1; 32]), group("group-a")),
    );
    world
        .device("device-a")
        .handle_legacy_upgrade_request(&DeviceId::new("device-b"), confirmation)
        .await
        .unwrap();

    assert!(!sponsor
        .pending_readmission
        .lock()
        .unwrap()
        .contains(&DeviceId::new("device-b")));
}

#[tokio::test]
async fn three_legacy_devices_automatically_converge_without_repairing() {
    let world = UpgradeWorld::new(&["device-a", "device-b", "device-c"]);
    world.protection("device-a").set_bootstrap_group("group-a");
    world.protection("device-b").set_bootstrap_group("group-b");
    world.protection("device-c").set_bootstrap_group("group-c");

    world
        .reconcile("device-a", LegacyDiscoveryPhase::Complete)
        .await;
    world.publish_devices();
    world
        .reconcile("device-b", LegacyDiscoveryPhase::Discovering)
        .await;
    world
        .reconcile("device-c", LegacyDiscoveryPhase::Discovering)
        .await;

    let selected_group = world.protection("device-a").group_id();
    assert_eq!(world.protection("device-b").group_id(), selected_group);
    assert_eq!(world.protection("device-c").group_id(), selected_group);
    assert_eq!(
        world.protection("device-a").protected.lock().unwrap().len(),
        2
    );
}

#[tokio::test]
async fn concurrent_temporary_groups_converge_on_the_smaller_group_id() {
    let world = UpgradeWorld::new(&["device-a", "device-b"]);
    world.protection("device-a").set_group("group-a");
    world.protection("device-b").set_group("group-b");
    world.publish_devices();

    world
        .reconcile("device-b", LegacyDiscoveryPhase::Discovering)
        .await;

    assert_eq!(
        world.protection("device-b").group_id(),
        Some(group("group-a"))
    );
}

#[tokio::test(start_paused = true)]
async fn runtime_reconciles_online_events_and_shutdown_stops_it() {
    let world = UpgradeWorld::new(&["device-a", "device-b"]);
    world.protection("device-a").set_group("group-a");
    let automatic_upgrade = world.device("device-b");
    let (presence_tx, presence_rx) = broadcast::channel(4);
    let started_at = tokio::time::Instant::now();
    let runtime = automatic_upgrade.start(presence_rx);
    wait_for_snapshot_calls(&world.protection("device-b"), 1).await;
    let startup_calls = world.network.exchange_calls.load(Ordering::Acquire);

    presence_tx
        .send(PresenceEvent {
            device_id: DeviceId::new("device-a"),
            state: ReachabilityState::Offline,
            at: Utc::now(),
        })
        .unwrap();
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        world.network.exchange_calls.load(Ordering::Acquire),
        startup_calls
    );

    world.publish_devices();
    presence_tx
        .send(PresenceEvent {
            device_id: DeviceId::new("device-a"),
            state: ReachabilityState::Online,
            at: Utc::now(),
        })
        .unwrap();
    for _ in 0..100 {
        if world.protection("device-b").group_id() == Some(group("group-a")) {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(
        world.protection("device-b").group_id(),
        Some(group("group-a"))
    );
    assert_eq!(tokio::time::Instant::now(), started_at);

    runtime.shutdown().await;
    assert!(presence_tx
        .send(PresenceEvent {
            device_id: DeviceId::new("device-a"),
            state: ReachabilityState::Online,
            at: Utc::now(),
        })
        .is_err());
}

#[tokio::test(start_paused = true)]
async fn runtime_reconciles_immediately_when_the_upgraded_device_starts() {
    let world = UpgradeWorld::new(&["device-a", "device-b"]);
    world.protection("device-a").set_group("group-a");
    world.publish_devices();
    let started_at = tokio::time::Instant::now();
    let (_presence_tx, presence_rx) = broadcast::channel(4);

    let runtime = world.device("device-b").start(presence_rx);
    for _ in 0..100 {
        if world.protection("device-b").group_id() == Some(group("group-a")) {
            break;
        }
        tokio::task::yield_now().await;
    }

    assert_eq!(
        world.protection("device-b").group_id(),
        Some(group("group-a"))
    );
    assert_eq!(tokio::time::Instant::now(), started_at);
    runtime.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn isolated_device_waits_for_discovery_before_bootstrapping() {
    let world = UpgradeWorld::new(&["device-a"]);
    let protection = world.protection("device-a");
    protection.set_bootstrap_group("group-a");
    let (_presence_tx, presence_rx) = broadcast::channel(4);
    let runtime = world.device("device-a").start(presence_rx);
    tokio::task::yield_now().await;

    tokio::time::advance(std::time::Duration::from_secs(5)).await;
    wait_for_snapshot_calls(&protection, 1).await;
    tokio::time::advance(std::time::Duration::from_secs(5)).await;
    wait_for_snapshot_calls(&protection, 2).await;
    tokio::time::advance(std::time::Duration::from_secs(4)).await;
    tokio::task::yield_now().await;
    assert_eq!(protection.group_id(), None);

    tokio::time::advance(std::time::Duration::from_secs(1)).await;
    wait_for_snapshot_calls(&protection, 3).await;
    assert_eq!(protection.group_id(), Some(group("group-a")));

    runtime.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn shutdown_reports_an_unexpected_runtime_failure() {
    let writer = CapturedWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(writer.clone())
        .with_ansi(false)
        .finish();
    let dispatch = tracing::Dispatch::new(subscriber);
    let _guard = tracing::dispatcher::set_default(&dispatch);
    let task = tokio::spawn(async { panic!("simulated runtime failure") });
    while !task.is_finished() {
        tokio::task::yield_now().await;
    }

    AutomaticLegacyUpgradeRuntime { task: Some(task) }
        .shutdown()
        .await;

    let logs = writer.dump();
    assert!(
        logs.contains("task.panicked"),
        "missing panic event: {logs}"
    );
    assert!(
        logs.contains("automatic_legacy_upgrade.runtime"),
        "missing task name: {logs}"
    );
}
