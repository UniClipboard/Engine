use super::super::blobs::BLOBS_ALPN;
use super::super::conn_path::connection_channel;
use super::super::observed_connections::close_reason;
use super::super::session_generation::{begin_session_local_work, SessionLocalWork};
#[cfg(feature = "test-util")]
use super::super::test_support::register_resource_connection;
use super::stream::{wrap, wrap_receive, wrap_send};
use async_trait::async_trait;
use iroh::endpoint::{Connection, Side};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::broadcast;
use uc_application::deps::{
    ConnectionMode, ConnectionProtocol, ConnectivityError, PeerConnection, PeerReceiveStream,
    PeerSendStream, PeerUnidirectionalConnection, StreamPair,
};
use uc_core::ids::DeviceId;
use uc_core::ports::{ConnectionChannel, PeerReachabilityChanged, ReachabilityState};
use uc_observability_contract::diagnostics::connectivity::{
    record_presence_closed, ConnectionDirection,
};
use uc_sync_protocol::active_clipboard_pull::ACTIVE_CLIPBOARD_PULL_ALPN;
use uc_sync_protocol::active_clipboard_state::ACTIVE_CLIPBOARD_ALPN;
use uc_sync_protocol::clipboard::{
    ATTRIBUTED_CLIPBOARD_ALPN, CLIPBOARD_ALPN, MULTIPLEXED_CLIPBOARD_ALPN,
};
use uc_sync_protocol::group_update::{GROUP_UPDATE_ALPN, LEGACY_GROUP_UPDATE_ALPN};
use uc_sync_protocol::membership_branch_recovery::MEMBERSHIP_BRANCH_RECOVERY_ALPN;
use uc_sync_protocol::membership_history::{
    MEMBERSHIP_HISTORY_EXCHANGE_ALPN, MULTIPLEXED_MEMBERSHIP_HISTORY_ALPN,
};
use uc_sync_protocol::peer_reachability::{LEGACY_PEER_REACHABILITY_ALPN, PEER_REACHABILITY_ALPN};
use uc_sync_protocol::transfer_progress::v2::TRANSFER_PROGRESS_ALPN;
use uc_sync_protocol::transfer_progress::LEGACY_TRANSFER_PROGRESS_ALPN;

pub(crate) struct IrohPeerConnection {
    native: Connection,
    _session_work: Option<SessionLocalWork>,
    unreachable: AtomicBool,
    close_recorded: AtomicBool,
    liveness: Option<(
        DeviceId,
        Mutex<broadcast::Receiver<PeerReachabilityChanged>>,
    )>,
}
impl IrohPeerConnection {
    pub(crate) fn wrap(connection: Connection) -> Arc<dyn PeerConnection> {
        #[cfg(feature = "test-util")]
        register_resource_connection(&connection);
        let session_work = if connection.alpn() == BLOBS_ALPN {
            let work = begin_session_local_work();
            if work.is_none() {
                connection.close(0u32.into(), b"session_retired");
            }
            work
        } else {
            None
        };
        Arc::new(Self {
            _session_work: session_work,
            native: connection,
            unreachable: AtomicBool::new(false),
            close_recorded: AtomicBool::new(false),
            liveness: None,
        })
    }
    fn observe_liveness(&self) {
        let Some((target, events)) = &self.liveness else {
            return;
        };
        let mut events = events.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            match events.try_recv() {
                Ok(event)
                    if event.device_id == *target && event.state == ReachabilityState::Offline =>
                {
                    self.unreachable.store(true, Ordering::Release);
                    self.close();
                    return;
                }
                Ok(_) => {}
                Err(broadcast::error::TryRecvError::Lagged(_)) => {
                    self.unreachable.store(true, Ordering::Release);
                    self.close();
                    return;
                }
                Err(_) => return,
            }
        }
    }
    pub(crate) fn wrap_outbound(
        connection: Connection,
        target: DeviceId,
        events: broadcast::Receiver<PeerReachabilityChanged>,
    ) -> Arc<dyn PeerConnection> {
        #[cfg(feature = "test-util")]
        register_resource_connection(&connection);
        Arc::new(Self {
            _session_work: None,
            native: connection,
            unreachable: AtomicBool::new(false),
            close_recorded: AtomicBool::new(false),
            liveness: Some((target, Mutex::new(events))),
        })
    }
}
#[async_trait]
impl PeerConnection for IrohPeerConnection {
    fn id(&self) -> usize {
        self.native.stable_id()
    }
    fn authenticated_key(&self) -> [u8; 32] {
        *self.native.remote_id().as_bytes()
    }
    fn mode(&self) -> ConnectionMode {
        if matches!(
            self.native.alpn(),
            ATTRIBUTED_CLIPBOARD_ALPN
                | MULTIPLEXED_CLIPBOARD_ALPN
                | MULTIPLEXED_MEMBERSHIP_HISTORY_ALPN
                | PEER_REACHABILITY_ALPN
                | BLOBS_ALPN
                | LEGACY_TRANSFER_PROGRESS_ALPN
                | TRANSFER_PROGRESS_ALPN
        ) {
            ConnectionMode::Multiplexed
        } else if self.native.alpn() == LEGACY_PEER_REACHABILITY_ALPN {
            ConnectionMode::RetainedSingleExchange
        } else {
            ConnectionMode::SingleExchange
        }
    }
    fn protocol(&self) -> ConnectionProtocol {
        match self.native.alpn() {
            BLOBS_ALPN => ConnectionProtocol::Blob,
            LEGACY_TRANSFER_PROGRESS_ALPN => ConnectionProtocol::TransferProgressV1,
            TRANSFER_PROGRESS_ALPN => ConnectionProtocol::TransferProgressV2,
            PEER_REACHABILITY_ALPN => ConnectionProtocol::PeerReachabilityV2,
            LEGACY_PEER_REACHABILITY_ALPN => ConnectionProtocol::PeerReachabilityV1,
            ATTRIBUTED_CLIPBOARD_ALPN => ConnectionProtocol::AttributedClipboard,
            MULTIPLEXED_CLIPBOARD_ALPN => ConnectionProtocol::MultiplexedClipboard,
            ACTIVE_CLIPBOARD_PULL_ALPN => ConnectionProtocol::ActiveClipboardPull,
            ACTIVE_CLIPBOARD_ALPN => ConnectionProtocol::ActiveClipboardState,
            CLIPBOARD_ALPN => ConnectionProtocol::Clipboard,
            MEMBERSHIP_HISTORY_EXCHANGE_ALPN => ConnectionProtocol::MembershipHistory,
            MEMBERSHIP_BRANCH_RECOVERY_ALPN => ConnectionProtocol::MembershipBranchRecovery,
            MULTIPLEXED_MEMBERSHIP_HISTORY_ALPN => ConnectionProtocol::MultiplexedMembershipHistory,
            GROUP_UPDATE_ALPN => ConnectionProtocol::GroupUpdate,
            LEGACY_GROUP_UPDATE_ALPN => ConnectionProtocol::LegacyGroupUpdate,
            // 只在固定协议 handler/backend 中构造；未知连接不提供新协议能力。
            _ => ConnectionProtocol::Unsupported,
        }
    }
    fn channel(&self) -> ConnectionChannel {
        connection_channel(&self.native)
    }
    fn is_closed(&self) -> bool {
        // 获取租约同步消费已排队的离线证据，不依赖监督任务先获得调度。
        self.observe_liveness();
        self.native.close_reason().is_some()
    }
    fn is_unreachable(&self) -> bool {
        self.unreachable.load(Ordering::Acquire)
    }
    fn close(&self) {
        self.native.close(0u32.into(), b"exchange ended");
    }
    async fn closed(&self) {
        if let Some((target, events)) = &self.liveness {
            let mut events = events
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .resubscribe();
            self.observe_liveness();
            loop {
                tokio::select! {
                    _ = self.native.closed() => return,
                    event = events.recv() => match event {
                        Ok(event) if event.device_id == *target && event.state == ReachabilityState::Offline => {
                            self.unreachable.store(true, Ordering::Release);
                            self.close();
                            return;
                        }
                        Ok(_) => self.observe_liveness(),
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            // 丢失已确认离线的历史后，不让未知旧连接继续供新交换复用。
                            self.unreachable.store(true, Ordering::Release);
                            self.close();
                            return;
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }
        let reason = self.native.closed().await;
        if matches!(
            self.native.alpn(),
            PEER_REACHABILITY_ALPN | LEGACY_PEER_REACHABILITY_ALPN
        ) && !self.close_recorded.swap(true, Ordering::AcqRel)
        {
            let direction = if self.native.side() == Side::Client {
                ConnectionDirection::Outbound
            } else {
                ConnectionDirection::Inbound
            };
            record_presence_closed(direction, close_reason(&reason));
        }
    }
    async fn open_bi(&self) -> Result<StreamPair, ConnectivityError> {
        self.observe_liveness();
        self.native
            .open_bi()
            .await
            .map(|(send, recv)| wrap(send, recv))
            .map_err(|error| ConnectivityError::Stream(Box::new(error)))
    }
    async fn accept_bi(&self) -> Result<StreamPair, ConnectivityError> {
        self.native
            .accept_bi()
            .await
            .map(|(send, recv)| wrap(send, recv))
            .map_err(|error| ConnectivityError::Stream(Box::new(error)))
    }
    fn unidirectional(&self) -> Option<&dyn PeerUnidirectionalConnection> {
        Some(self)
    }
}

#[async_trait]
impl PeerUnidirectionalConnection for IrohPeerConnection {
    async fn open_uni(&self) -> Result<Box<dyn PeerSendStream>, ConnectivityError> {
        self.observe_liveness();
        self.native
            .open_uni()
            .await
            .map(wrap_send)
            .map_err(|source| ConnectivityError::Stream(Box::new(source)))
    }

    async fn accept_uni(&self) -> Result<Box<dyn PeerReceiveStream>, ConnectivityError> {
        self.native
            .accept_uni()
            .await
            .map(wrap_receive)
            .map_err(|source| ConnectivityError::Stream(Box::new(source)))
    }
}
