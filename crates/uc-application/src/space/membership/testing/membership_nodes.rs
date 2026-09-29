//! 多节点成员测试台：每个节点是一套完整的成员状态负责人、待办执行器与用户动作，节点之间经虚拟
//! 网络交换成员历史与受限投递。时钟由全部节点共享，由场景推进。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use uc_core::ids::DeviceId;
use uc_core::membership::{
    LedgerInput, MemberInstanceId, MembershipEventId, MembershipHistoryAckV3,
    MembershipHistoryExchangeEndpointPort, MembershipHistoryExchangeError,
    MembershipHistoryExchangePort, MembershipHistoryMessage, MembershipLedger, PeerLink,
    VersionedMembershipHistory,
};
use uc_core::ports::ReachabilityState;

use super::virtual_membership_network::{VirtualMembershipNetwork, VirtualMembershipNetworkError};
use super::{
    AcceptingVerifier, CompletedStep, FixedSpaceWorkMode, MemoryMembershipRecords,
    RecordingEffects, RecordingHostEvents, RecordingWake, TestClock, TestSigner,
};
use crate::space::membership::query_device_trust::NoCurrentJoinStatus;
use crate::space::membership::{
    ledger_error, DecideDeviceTrustChange, DecideDeviceTrustChangeError,
    DecideDeviceTrustChangeResult, DecideDeviceTrustChangeUseCase, DeviceTrustChangeChoice,
    DeviceTrustObservation, DeviceTrustStatus, HandleMembershipHistoryMessageUseCase,
    LoadDeviceTrustObservationsPort, MembershipMaintenanceReport, MembershipMaintenanceTrigger,
    MembershipOwner, MembershipRecord, MembershipWorker, MembershipWorkerDeps,
    QueryDeviceTrustError, QueryDeviceTrustUseCase, RefreshVerifiedPeerAddressPort,
    RemoveSpaceMemberError, RemoveSpaceMemberResult, RemoveSpaceMemberUseCase,
    RestrictedMembershipDelivery, RestrictedMembershipDeliveryError,
    RestrictedMembershipDeliveryPort, RunMembershipWorkPort,
};

/// 设备到节点标签的映射；未登记的设备视为离线。
#[derive(Default)]
struct NodeDirectory(Mutex<BTreeMap<DeviceId, &'static str>>);

impl NodeDirectory {
    fn label(&self, device: &DeviceId) -> Option<&'static str> {
        self.0.lock().unwrap().get(device).copied()
    }
}

/// 记录地址刷新请求，以及请求发生时本机账本是否已确认该对端位于本机当前位置。
struct RecordingAddressRefresh {
    records: Arc<MemoryMembershipRecords>,
    calls: Mutex<Vec<(DeviceId, bool)>>,
}

#[async_trait]
impl RefreshVerifiedPeerAddressPort for RecordingAddressRefresh {
    async fn refresh_verified_peer_address(&self, peer: &DeviceId) {
        let confirmed = match self.records.record() {
            MembershipRecord::Space(space) => {
                let ledger = MembershipLedger::restore(space.ledger).unwrap();
                let current = ledger.history().current_position().unwrap();
                matches!(
                    ledger.peer(peer),
                    Some(PeerLink::Member(link)) if link.confirmed_position() == Some(&current)
                )
            }
            MembershipRecord::NoSpace { .. } => false,
        };
        self.calls.lock().unwrap().push((*peer, confirmed));
    }
}

/// 一个节点对外的成员历史交换与受限投递，全部经虚拟网络送达。
struct NodeTransport {
    network: Arc<VirtualMembershipNetwork>,
    directory: Arc<NodeDirectory>,
    source: &'static str,
}

impl NodeTransport {
    async fn send(
        &self,
        peer: &DeviceId,
        message: MembershipHistoryMessage,
    ) -> Result<MembershipHistoryMessage, MembershipHistoryExchangeError> {
        let Some(target) = self.directory.label(peer) else {
            return Err(MembershipHistoryExchangeError::offline());
        };
        self.network
            .send(self.source, target, message)
            .await
            .map_err(|error| match error {
                VirtualMembershipNetworkError::Offline
                | VirtualMembershipNetworkError::UnknownNode => {
                    MembershipHistoryExchangeError::offline()
                }
                VirtualMembershipNetworkError::Rejected => MembershipHistoryExchangeError::Rejected,
                VirtualMembershipNetworkError::PairingInProgress => {
                    MembershipHistoryExchangeError::PairingInProgress
                }
                VirtualMembershipNetworkError::InvalidNode
                | VirtualMembershipNetworkError::DuplicateNode
                | VirtualMembershipNetworkError::DuplicateIdentity
                | VirtualMembershipNetworkError::Transport
                | VirtualMembershipNetworkError::FrameBudgetExceeded => {
                    MembershipHistoryExchangeError::transport()
                }
            })
    }
}

#[async_trait]
impl MembershipHistoryExchangePort for NodeTransport {
    async fn exchange_membership_history(
        &self,
        peer: &DeviceId,
        message: MembershipHistoryMessage,
    ) -> Result<MembershipHistoryMessage, MembershipHistoryExchangeError> {
        self.send(peer, message).await
    }
}

#[async_trait]
impl RestrictedMembershipDeliveryPort for NodeTransport {
    async fn deliver_restricted_membership(
        &self,
        peer: &DeviceId,
        delivery: &RestrictedMembershipDelivery,
    ) -> Result<(), RestrictedMembershipDeliveryError> {
        let message = match delivery {
            RestrictedMembershipDelivery::Event(event) => {
                MembershipHistoryMessage::RestrictedEventV3(event.as_ref().clone())
            }
            RestrictedMembershipDelivery::Decision(decision) => {
                MembershipHistoryMessage::RestrictedDecisionV3(decision.as_ref().clone())
            }
        };
        match self.send(peer, message).await {
            Ok(MembershipHistoryMessage::AckV3(
                MembershipHistoryAckV3::RestrictedApplied
                | MembershipHistoryAckV3::RestrictedConsistent,
            )) => Ok(()),
            Ok(_) | Err(MembershipHistoryExchangeError::Rejected) => {
                Err(RestrictedMembershipDeliveryError::Rejected)
            }
            Err(
                MembershipHistoryExchangeError::Offline { .. }
                | MembershipHistoryExchangeError::PairingInProgress
                | MembershipHistoryExchangeError::Transport { .. },
            ) => Err(RestrictedMembershipDeliveryError::Deferred),
        }
    }
}

/// 节点重启后仍由同一网络入口接收消息。
struct SwitchableEndpoint(Mutex<Arc<HandleMembershipHistoryMessageUseCase>>);

#[async_trait]
impl MembershipHistoryExchangeEndpointPort for SwitchableEndpoint {
    async fn handle_membership_history_exchange(
        &self,
        source_device_id: &DeviceId,
        message: MembershipHistoryMessage,
    ) -> Result<MembershipHistoryMessage, MembershipHistoryExchangeError> {
        let handler = Arc::clone(&self.0.lock().unwrap());
        handler
            .handle_membership_history_exchange(source_device_id, message)
            .await
    }
}

struct OfflineObservations;

#[async_trait]
impl LoadDeviceTrustObservationsPort for OfflineObservations {
    async fn load(
        &self,
        device_ids: &[DeviceId],
    ) -> Result<Vec<DeviceTrustObservation>, QueryDeviceTrustError> {
        Ok(device_ids
            .iter()
            .map(|device_id| DeviceTrustObservation {
                device_id: *device_id,
                display_name: None,
                reachability: ReachabilityState::Offline,
            })
            .collect())
    }
}

/// 一次进程生命周期内的节点组件；重启时整体重建。
struct NodeParts {
    owner: Arc<MembershipOwner>,
    worker: Arc<MembershipWorker>,
    remove: RemoveSpaceMemberUseCase,
    decide: DecideDeviceTrustChangeUseCase,
    query: Arc<QueryDeviceTrustUseCase>,
}

pub(crate) struct VirtualNode {
    label: &'static str,
    device: DeviceId,
    records: Arc<MemoryMembershipRecords>,
    clock: Arc<TestClock>,
    signer: Arc<TestSigner>,
    transport: Arc<NodeTransport>,
    endpoint: Arc<SwitchableEndpoint>,
    wake: Arc<RecordingWake>,
    address_refresh: Arc<RecordingAddressRefresh>,
    parts: Mutex<Arc<NodeParts>>,
}

impl VirtualNode {
    fn build_parts(
        records: &Arc<MemoryMembershipRecords>,
        clock: &Arc<TestClock>,
        signer: &Arc<TestSigner>,
        transport: &Arc<NodeTransport>,
        wake: &Arc<RecordingWake>,
        address_refresh: &Arc<RecordingAddressRefresh>,
    ) -> NodeParts {
        let owner = Arc::new(MembershipOwner::new(
            records.clone(),
            Arc::new(AcceptingVerifier),
            clock.clone(),
            Arc::new(RecordingHostEvents::default()),
            wake.clone(),
        ));
        let effects = Arc::new(RecordingEffects::default());
        let worker = Arc::new(MembershipWorker::new(
            owner.clone(),
            MembershipWorkerDeps {
                member_facts: effects.clone(),
                security: effects.clone(),
                activation: effects,
                restricted_delivery: transport.clone(),
                history_transport: transport.clone(),
                address_refresh: address_refresh.clone(),
                conflicts: Arc::new(CompletedStep),
                group_updates: Arc::new(CompletedStep),
            },
        ));
        let query = Arc::new(QueryDeviceTrustUseCase::new_for_tests(
            owner.clone(),
            Arc::new(OfflineObservations),
            Arc::new(NoCurrentJoinStatus),
        ));
        NodeParts {
            remove: RemoveSpaceMemberUseCase::new(
                owner.clone(),
                signer.clone(),
                query.clone(),
                worker.clone(),
            ),
            decide: DecideDeviceTrustChangeUseCase::new(
                owner.clone(),
                signer.clone(),
                query.clone(),
                worker.clone(),
            ),
            owner,
            worker,
            query,
        }
    }

    fn parts(&self) -> Arc<NodeParts> {
        Arc::clone(&self.parts.lock().unwrap())
    }

    pub(crate) fn label(&self) -> &'static str {
        self.label
    }

    pub(crate) fn record(&self) -> MembershipRecord {
        self.records.record()
    }

    /// 当前 Space 的成员账本；没有当前 Space 时为空。
    pub(crate) fn ledger(&self) -> Option<MembershipLedger> {
        match self.records.record() {
            MembershipRecord::Space(space) => {
                Some(MembershipLedger::restore(space.ledger).unwrap())
            }
            MembershipRecord::NoSpace { .. } => None,
        }
    }

    pub(crate) fn owner(&self) -> Arc<MembershipOwner> {
        self.parts().owner.clone()
    }

    pub(crate) fn wake(&self) -> &RecordingWake {
        &self.wake
    }

    /// 本节点发起的地址刷新：`(对端, 刷新时本机账本是否已确认该对端位于当前位置)`。
    pub(crate) fn address_refreshes(&self) -> Vec<(DeviceId, bool)> {
        self.address_refresh.calls.lock().unwrap().clone()
    }

    /// 模拟进程重启：丢弃内存中的全部组件，从同一持久记录重建。
    pub(crate) fn restart(&self) {
        let parts = Arc::new(Self::build_parts(
            &self.records,
            &self.clock,
            &self.signer,
            &self.transport,
            &self.wake,
            &self.address_refresh,
        ));
        *self.endpoint.0.lock().unwrap() = Arc::new(HandleMembershipHistoryMessageUseCase::new(
            parts.owner.clone(),
            FixedSpaceWorkMode::active(),
            self.address_refresh.clone(),
        ));
        *self.parts.lock().unwrap() = parts;
    }

    pub(crate) async fn run_worker(&self) -> MembershipMaintenanceReport {
        self.parts()
            .worker
            .run_membership_work(&MembershipMaintenanceTrigger::StateChanged)
            .await
    }

    pub(crate) async fn remove(
        &self,
        target: &DeviceId,
    ) -> Result<RemoveSpaceMemberResult, RemoveSpaceMemberError> {
        self.parts().remove.execute(target).await
    }

    pub(crate) async fn decide(
        &self,
        change_id: MembershipEventId,
        choice: DeviceTrustChangeChoice,
    ) -> Result<DecideDeviceTrustChangeResult, DecideDeviceTrustChangeError> {
        self.parts()
            .decide
            .execute(DecideDeviceTrustChange {
                change_id,
                choice,
                confirm_local_removal: true,
            })
            .await
    }

    pub(crate) async fn status(&self) -> DeviceTrustStatus {
        self.parts().query.execute().await.unwrap()
    }

    /// 本机待决定的移除。
    pub(crate) fn pending_decision(&self) -> Option<MembershipEventId> {
        let ledger = self.ledger()?;
        ledger
            .history()
            .pending_removal_decision(ledger.local_member())
    }
}

/// 共享时钟与虚拟网络上的一组节点。
pub(crate) struct VirtualMembershipNodes {
    network: Arc<VirtualMembershipNetwork>,
    directory: Arc<NodeDirectory>,
    clock: Arc<TestClock>,
    nodes: Vec<Arc<VirtualNode>>,
}

impl VirtualMembershipNodes {
    pub(crate) fn new(start_ms: i64, max_frames: usize) -> Self {
        Self {
            network: Arc::new(VirtualMembershipNetwork::new(max_frames)),
            directory: Arc::new(NodeDirectory::default()),
            clock: TestClock::at(start_ms),
            nodes: Vec::new(),
        }
    }

    pub(crate) fn clock(&self) -> &TestClock {
        &self.clock
    }

    pub(crate) fn now_ms(&self) -> i64 {
        uc_core::ports::ClockPort::now_ms(self.clock.as_ref())
    }

    /// 以给定成员记录加入一个节点。
    pub(crate) fn add_node(
        &mut self,
        label: &'static str,
        record: MembershipRecord,
        signer: TestSigner,
    ) -> Arc<VirtualNode> {
        let records = MemoryMembershipRecords::new(record);
        let signer = Arc::new(signer);
        let device = signer.local_device_id;
        let transport = Arc::new(NodeTransport {
            network: self.network.clone(),
            directory: self.directory.clone(),
            source: label,
        });
        let wake = Arc::new(RecordingWake::default());
        let address_refresh = Arc::new(RecordingAddressRefresh {
            records: records.clone(),
            calls: Mutex::new(Vec::new()),
        });
        let parts = Arc::new(VirtualNode::build_parts(
            &records,
            &self.clock,
            &signer,
            &transport,
            &wake,
            &address_refresh,
        ));
        let endpoint = Arc::new(SwitchableEndpoint(Mutex::new(Arc::new(
            HandleMembershipHistoryMessageUseCase::new(
                parts.owner.clone(),
                FixedSpaceWorkMode::active(),
                address_refresh.clone(),
            ),
        ))));
        self.network
            .register(label, device, endpoint.clone())
            .unwrap();
        self.directory.0.lock().unwrap().insert(device, label);
        let node = Arc::new(VirtualNode {
            label,
            device,
            records,
            clock: self.clock.clone(),
            signer,
            transport,
            endpoint,
            wake,
            address_refresh,
            parts: Mutex::new(parts),
        });
        self.nodes.push(node.clone());
        node
    }

    pub(crate) fn nodes(&self) -> &[Arc<VirtualNode>] {
        &self.nodes
    }

    /// 双向断开两个节点。
    pub(crate) fn partition(&self, left: &'static str, right: &'static str) {
        self.network.partition(left, right).unwrap();
        self.network.partition(right, left).unwrap();
    }

    pub(crate) fn heal(&self, left: &'static str, right: &'static str) {
        self.network.heal(left, right).unwrap();
        self.network.heal(right, left).unwrap();
    }

    pub(crate) fn heal_all(&self) {
        for left in &self.nodes {
            for right in &self.nodes {
                if left.label != right.label {
                    self.network.heal(left.label, right.label).unwrap();
                }
            }
        }
    }

    /// 反复执行全部节点的到期待办，直到所有节点的成员记录不再变化。
    pub(crate) async fn settle(&self, max_rounds: usize) -> bool {
        for _ in 0..max_rounds {
            let before: Vec<_> = self.nodes.iter().map(|node| node.record()).collect();
            for node in &self.nodes {
                node.run_worker().await;
            }
            let after: Vec<_> = self.nodes.iter().map(|node| node.record()).collect();
            if before == after {
                return true;
            }
        }
        false
    }
}

/// 邀请方 `sponsor` 正式接纳 `joiner`：邀请方提交加入，加入方从同一历史建立成员状态。
pub(crate) async fn admit(
    sponsor: &VirtualNode,
    joiner: &VirtualNode,
    joiner_credential_byte: u8,
    marker: u8,
) -> MemberInstanceId {
    let ledger = sponsor.ledger().expect("the sponsor has a current space");
    let mut history: VersionedMembershipHistory = ledger.history().clone();
    let (member, _) = super::append_active_peer_to_history(
        &mut history,
        ledger.local_member(),
        joiner.device.as_str(),
        joiner_credential_byte,
        marker,
    );
    sponsor
        .owner()
        .commit(|draft| {
            draft
                .apply(LedgerInput::AdmissionCommitted {
                    history: history.clone(),
                })
                .map_err(ledger_error)
        })
        .await
        .unwrap();
    let joiner_device = joiner.device;
    joiner
        .owner()
        .commit(move |draft| draft.start_space(history, joiner_device, member))
        .await
        .unwrap();
    member
}
