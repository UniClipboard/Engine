use super::owner::{ProgressRuntime, ProgressSession};
use super::TransferProgressNetwork;
use crate::connectivity::ConnectivityManager;
use crate::facade::host_event::HostEventBus;
use crate::transfer::progress::{
    transfer_progress_connectivity_policy, ProgressExchangeService, ProgressExchangeWorker,
    ProgressProjectionRegistry, ProgressProjectionWorker, ProgressSnapshotProviderPort,
    ProgressTaskAuthorityPort, TransferProgressProducer, TransferProgressReconciler,
    TransferProgressSnapshotReader,
};
use crate::transfer::receive::work::ReceiveWorkRegistry;
use std::sync::Arc;
use tokio::sync::broadcast;
use uc_core::ports::PeerReachabilityChanged;

/// 同一对象图统一收发、投影、核对与补发；Engine只注册完整入站端点。
pub(crate) struct ProgressRuntimeAssembly {
    pub session: ProgressSession,
    projection: ProgressProjectionWorker,
    exchange: ProgressExchangeWorker,
    receipts: Arc<ReceiveWorkRegistry>,
    host_events: Arc<HostEventBus>,
}
impl ProgressRuntimeAssembly {
    pub(crate) fn new(
        network: TransferProgressNetwork,
        receipts: Arc<ReceiveWorkRegistry>,
        host_events: Arc<HostEventBus>,
    ) -> Self {
        let authority: Arc<dyn ProgressTaskAuthorityPort> = receipts.clone();
        let snapshots: Arc<dyn ProgressSnapshotProviderPort> = receipts.clone();
        let TransferProgressNetwork {
            backend,
            admission,
            wire,
            snapshot_wire,
            inbound,
            clock,
        } = network;
        let manager = ConnectivityManager::new(
            backend,
            admission,
            transfer_progress_connectivity_policy(clock.clone()),
        );
        let (projections, projection) = ProgressProjectionRegistry::new(clock.clone());
        let producer = Arc::new(TransferProgressProducer::new(
            manager.clone(),
            wire,
            authority.clone(),
            snapshots.clone(),
            clock.clone(),
        ));
        let reader = Arc::new(TransferProgressSnapshotReader::new(
            manager.clone(),
            snapshot_wire,
        ));
        let (exchanges, exchange) = ProgressExchangeService::new(
            clock.clone(),
            authority,
            producer.clone(),
            reader,
            producer,
        );
        let protocol = inbound(
            Arc::new(projections.clone()),
            Arc::new(projections.clone()),
            snapshots,
        );
        let inbound = projections.inbound_transport(manager, protocol, clock);
        let reconciliation = Arc::new(TransferProgressReconciler::new(
            projections.clone(),
            exchanges.clone(),
        ));
        Self {
            session: ProgressSession {
                projections,
                exchanges,
                reconciliation,
                inbound,
            },
            projection,
            exchange,
            receipts,
            host_events,
        }
    }
    pub(crate) fn start(
        self,
        reachability: broadcast::Receiver<PeerReachabilityChanged>,
    ) -> ProgressRuntime {
        ProgressRuntime::start(
            self.session,
            self.projection,
            self.exchange,
            self.receipts,
            self.host_events,
            reachability,
        )
    }
}
