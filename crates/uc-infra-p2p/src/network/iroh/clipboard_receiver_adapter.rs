//! Iroh-backed implementation of [`ClipboardReceiverPort`] (Slice 2 Phase 2).
//!
//! The adapter publishes an [`InboundClipboard`] broadcast stream that the
//! application inbound runtime subscribes to. Actual connections are handled by
//! [`IrohClipboardReceiverHandler`] — the same `ProtocolHandler` split
//! pattern we use for pairing / peer_reachability (see `docs/design-docs/layers/infrastructure.md` §4.3):
//! adapter owns the broadcast `Sender` and the domain dependencies, the
//! handler is a cheap `Clone` that iroh's `Router` registers under
//! [`CLIPBOARD_ALPN`](super::clipboard_dispatch_adapter::CLIPBOARD_ALPN).
//!
//! ## Identity resolution
//!
//! Each inbound connection's `Connection::remote_id()` is the peer's iroh
//! `EndpointId`, which is an `iroh_base::PublicKey` newtype over the
//! 32-byte Ed25519 public key. The receiver feeds those bytes into the
//! same [`IdentityFingerprintFactoryPort`] that `IrohIdentityStore` uses
//! when persisting the local fingerprint, recovers the remote's
//! `IdentityFingerprint`, and looks it up in [`PeerIdentityDirectoryPort`].
//! This invariant was established by the T2 probe
//! (`tests/iroh_clipboard_identity_probe.rs`) — no new port method is
//! needed and no `EndpointId` type leaks above the adapter.
//!
//! Unknown peers (fingerprint not in the identity directory) receive
//! [`AckCode::Rejected`] and the connection is closed. They never make it
//! to the broadcast stream, so the application runtime does not need a
//! second identity rejection path.
//!
//! ## Failure semantics
//!
//! The handler is best-effort per connection. Every failure path is logged
//! and the connection closes; the port-level contract is "lagging
//! subscribers recover via the next dispatch", so dropping an occasional
//! inbound frame is never fatal. That keeps accept-loop code free of
//! retry logic.

use std::sync::Arc;
use std::time::{Duration, Instant};
use uc_application::deps::PeerIdentityDirectoryPort;
use uc_application::deps::{ClipboardDelivery, ClipboardReceiverPort};
use uc_observability_contract::{error_source::io_error_kind, uc_debug, uc_warn};

use async_trait::async_trait;
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use tokio::sync::broadcast;
use tracing::{instrument, Instrument};

#[cfg(test)]
use uc_core::ids::DeviceId;
use uc_core::membership::PeerAdmissionPort;
use uc_core::ports::security::IdentityFingerprintFactoryPort;
use uc_core::ports::{InboundClipboard, InboundClipboardDisposition, InboundClipboardReceipt};
use uc_observability_contract::diagnostics::connectivity::{
    complete_clipboard_receive_failure, ClipboardReceiveFailure, ClipboardReceiveObservation,
    InboundPeerProtocol,
};
use uc_observability_contract::diagnostics::{
    complete_operation, operation_span, DiagnosticDomain, DiagnosticErrorType, DiagnosticOperation,
    DiagnosticRole, DiagnosticSpanKind, OperationCompletion, OperationContext,
};

use super::admitted_connections::AdmittedConnections;
use super::conn_path::connection_channel;
use super::inbound_peer::InboundPeerGate;
use super::trace_context::set_remote_parent;
use uc_sync_protocol::clipboard::{self as clipboard_wire, AckCode};

/// Capacity of the `InboundClipboard` broadcast channel. Matches the
/// peer_reachability adapter (`PRESENCE_EVENT_CHANNEL_CAPACITY`) so both streams
/// share the same burst tolerance. Lagging subscribers drop frames per
/// broadcast semantics — they recover on the peer's next dispatch.
const INBOUND_CHANNEL_CAPACITY: usize = 64;
const APPLICATION_SETTLEMENT_TIMEOUT: Duration = Duration::from_secs(60);

/// Receiver-side adapter. Holds the broadcast sender and domain
/// dependencies needed to resolve `endpoint_id → DeviceId`. See
/// [`IrohClipboardReceiverAdapter::handler`] for the paired
/// `ProtocolHandler`.
pub struct IrohClipboardReceiverAdapter {
    event_tx: broadcast::Sender<ClipboardDelivery>,
    handler_state: Arc<HandlerState>,
}

struct HandlerState {
    connections: AdmittedConnections,
    gate: InboundPeerGate,
    event_tx: broadcast::Sender<ClipboardDelivery>,
}

impl IrohClipboardReceiverAdapter {
    pub fn new(
        identities: Arc<dyn PeerIdentityDirectoryPort>,
        peer_admission: Arc<dyn PeerAdmissionPort>,
        fingerprint_factory: Arc<dyn IdentityFingerprintFactoryPort>,
    ) -> Self {
        Self::with_connections(
            identities,
            peer_admission,
            fingerprint_factory,
            AdmittedConnections::default(),
        )
    }

    /// 与会话内其他协议共享已授权连接登记，撤销成员时由登记统一关闭。
    pub(crate) fn with_connections(
        identities: Arc<dyn PeerIdentityDirectoryPort>,
        peer_admission: Arc<dyn PeerAdmissionPort>,
        fingerprint_factory: Arc<dyn IdentityFingerprintFactoryPort>,
        connections: AdmittedConnections,
    ) -> Self {
        let (event_tx, _) = broadcast::channel(INBOUND_CHANNEL_CAPACITY);
        let handler_state = Arc::new(HandlerState {
            connections,
            gate: InboundPeerGate::new(
                InboundPeerProtocol::Clipboard,
                identities,
                peer_admission,
                fingerprint_factory,
            ),
            event_tx: event_tx.clone(),
        });
        Self {
            event_tx,
            handler_state,
        }
    }

    /// Cheap clone-able handle registered with iroh's `RouterBuilder`.
    /// Each inbound connection runs [`IrohClipboardReceiverHandler::accept`],
    /// which shares the adapter's broadcast `Sender` via `Arc`.
    pub fn handler(&self) -> IrohClipboardReceiverHandler {
        IrohClipboardReceiverHandler {
            state: Arc::clone(&self.handler_state),
        }
    }
}

#[async_trait]
impl ClipboardReceiverPort for IrohClipboardReceiverAdapter {
    fn subscribe(&self) -> broadcast::Receiver<ClipboardDelivery> {
        self.event_tx.subscribe()
    }
}

// ============================================================================
// ProtocolHandler (accept side)
// ============================================================================

#[derive(Clone)]
pub struct IrohClipboardReceiverHandler {
    state: Arc<HandlerState>,
}

impl std::fmt::Debug for IrohClipboardReceiverHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IrohClipboardReceiverHandler")
            .finish_non_exhaustive()
    }
}

impl ProtocolHandler for IrohClipboardReceiverHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let remote = connection.remote_id();
        let remote_bytes: [u8; 32] = *remote.as_bytes();

        // 1. Resolve the remote endpoint's public key back to a known
        //    SpaceMember.
        let resolved = self.state.gate.identify(&remote_bytes).await;
        // 先登记再授权：成员被移除时，登记会关闭这条仍在等待帧或应用结算的连接。
        let _registration = resolved
            .as_ref()
            .ok()
            .map(|device| self.state.connections.register(device, &connection));

        // 2. Open the bi-stream even for rejected peers — we still want to
        //    send a `Rejected` ack so the sender does not stall on its
        //    `recv.read_exact` waiting for a byte that never comes.
        let (mut send, mut recv) = match connection.accept_bi().await {
            Ok(pair) => pair,
            Err(err) => {
                uc_warn!(
                    error_kind = "accept_bi",
                    io_error_kind = io_error_kind(&err),
                    "clipboard receiver: accept_bi failed; dropping connection"
                );
                return Ok(());
            }
        };

        let peer_device_id = match resolved {
            Ok(device) => device,
            Err(rejection) => {
                self.state.gate.record_rejection(rejection);
                emit_ack(&mut send, AckCode::Rejected).await;
                // 回执先于连接清理，避免拨号方只收到连接关闭。
                let _ = connection.closed().await;
                return Ok(());
            }
        };

        // 3. Read the frame. Any codec-level failure ends the connection
        //    with a `Rejected` ack so the sender gets a typed
        //    `PeerRejected` error rather than an unexplained `Io`.
        if let Err(rejection) = self.state.gate.authorize(&peer_device_id).await {
            self.state.gate.record_rejection(rejection);
            emit_ack(&mut send, AckCode::Rejected).await;
            let _ = connection.closed().await;
            return Ok(());
        }

        let decoded = match clipboard_wire::read_frame_header(&mut recv).await {
            Ok(decoded) => decoded,
            Err(err) => {
                uc_warn!(
                    error_kind = "frame_decode",
                    io_error_kind = io_error_kind(&err),
                    "clipboard receiver: frame decode failed; sending Rejected ack"
                );
                let ack = if matches!(err, clipboard_wire::WireDecodeError::IncompatibleLayout) {
                    AckCode::Incompatible
                } else {
                    AckCode::Rejected
                };
                emit_ack(&mut send, ack).await;
                let _ = connection.closed().await;
                return Ok(());
            }
        };

        let span = operation_span(OperationContext {
            domain: DiagnosticDomain::Clipboard,
            operation: DiagnosticOperation::ClipboardReceive,
            role: DiagnosticRole::Server,
            kind: DiagnosticSpanKind::Server,
        });
        let _ = set_remote_parent(&span, decoded.trace_context.as_ref());
        let closed = connection.clone();
        let receive_observation = ClipboardReceiveObservation::default();
        let operation_observation = receive_observation.clone();
        let result = async move {
            let started = Instant::now();
            let ciphertext =
                match clipboard_wire::read_frame_payload(&mut recv, decoded.payload_len()).await {
                    Ok(ciphertext) => ciphertext,
                    Err(error) => {
                        complete_operation(OperationCompletion::failed(
                            DiagnosticDomain::Clipboard,
                            DiagnosticOperation::ClipboardReceive,
                            DiagnosticRole::Server,
                            DiagnosticErrorType::StreamFailed,
                            started.elapsed(),
                        ));
                        uc_warn!(
                            error_kind = "payload_read",
                            io_error_kind = io_error_kind(&error),
                            "clipboard receiver: payload read failed; sending Rejected ack"
                        );
                        emit_ack(&mut send, AckCode::Rejected).await;
                        return Ok(());
                    }
                };

            // 认证后的任务通过原有广播交接；没有消费者时拒绝，绝不伪报保存成功。
            let (receipt, result) = InboundClipboardReceipt::pending();
            let transport = connection_channel(&connection);
            let inbound = InboundClipboard {
                peer_device_id,
                header: decoded.header,
                ciphertext,
                transport,
                receipt,
            };
            if self
                .state
                .event_tx
                .send(ClipboardDelivery::new(inbound))
                .is_err()
            {
                uc_debug!("clipboard receiver: no subscribers attached; inbound frame dropped");
                emit_ack(&mut send, AckCode::Rejected).await;
                complete_clipboard_receive_failure(
                    ClipboardReceiveFailure::NoConsumer,
                    OperationCompletion::failed(
                        DiagnosticDomain::Clipboard,
                        DiagnosticOperation::ClipboardReceive,
                        DiagnosticRole::Server,
                        DiagnosticErrorType::Unavailable,
                        started.elapsed(),
                    ),
                );
                return Ok(());
            }

            let settlement =
                tokio::time::timeout(APPLICATION_SETTLEMENT_TIMEOUT, result.wait()).await;
            let (disposition, failure) = match settlement {
                Ok(Some(disposition)) => (
                    Some(disposition),
                    ClipboardReceiveFailure::ApplicationRejected,
                ),
                Ok(None) => (None, ClipboardReceiveFailure::ReceiptDropped),
                Err(_) => (None, ClipboardReceiveFailure::SettlementTimeout),
            };
            let ack = match disposition {
                Some(InboundClipboardDisposition::Applied) => AckCode::Accepted,
                Some(InboundClipboardDisposition::Duplicate) => AckCode::DuplicateIgnored,
                Some(InboundClipboardDisposition::Rejected) | None => AckCode::Rejected,
            };

            let completion = match ack {
                AckCode::Accepted | AckCode::DuplicateIgnored => OperationCompletion::succeeded(
                    DiagnosticDomain::Clipboard,
                    DiagnosticOperation::ClipboardReceive,
                    DiagnosticRole::Server,
                    started.elapsed(),
                ),
                AckCode::Rejected | AckCode::Incompatible => OperationCompletion::failed(
                    DiagnosticDomain::Clipboard,
                    DiagnosticOperation::ClipboardReceive,
                    DiagnosticRole::Server,
                    DiagnosticErrorType::Unavailable,
                    started.elapsed(),
                ),
            };
            if matches!(ack, AckCode::Rejected | AckCode::Incompatible) {
                operation_observation.finish_failure(failure, completion);
            } else {
                complete_operation(completion);
            }

            // 5. Ack the application result; hold the connection open until the peer closes
            //    it so the ack byte has time to flush. The sender side drops
            //    the connection after reading the ack, which resolves
            //    `Connection::closed()` here and lets the handler return.
            emit_ack(&mut send, ack).await;
            Ok(())
        }
        .instrument(span);
        let result = receive_observation.scope(result).await;
        // 回复后的连接清理不计入业务接收耗时。
        let _ = closed.closed().await;
        result
    }
}

/// Write a one-byte ack + finish the send half. Failures are logged but
/// swallowed: the connection is about to close either way, and the port
/// contract documents that adapter-level frame failures are best-effort.
#[instrument(skip_all, fields(ack = ?ack))]
async fn emit_ack(send: &mut iroh::endpoint::SendStream, ack: AckCode) {
    if let Err(err) = send.write_all(&[ack.as_byte()]).await {
        uc_debug!(
            error_kind = "ack_write",
            io_error_kind = io_error_kind(&err),
            "clipboard receiver: ack write failed"
        );
        return;
    }
    if let Err(err) = send.finish() {
        uc_debug!(
            error_kind = "send_finish",
            io_error_kind = io_error_kind(&err),
            "clipboard receiver: send.finish failed"
        );
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
#[path = "clipboard_receiver_tests.rs"]
mod tests;
