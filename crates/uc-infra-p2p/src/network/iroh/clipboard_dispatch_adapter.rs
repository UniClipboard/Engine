//! Iroh-backed implementation of [`ClipboardDispatchPort`] (Slice 2 Phase 2).
//!
//! Each call opens a fresh iroh bi-stream on [`CLIPBOARD_ALPN`], writes the
//! framed header + ciphertext per [`uc_sync_protocol::clipboard`],
//! reads the peer's single-byte ack, and closes. Concurrent fan-out to
//! multiple peers is assembled by the application-layer dispatch use case;
//! this adapter stays single-target.
//!
//! Failure mapping is deliberately narrow:
//!
//! * `peer_addr_repo.get()` returning `None` (no stored transport address
//!   for this peer) → [`ClipboardDispatchError::Offline`]. From the caller's
//!   perspective this is indistinguishable from a dial-time failure, and
//!   collapsing them avoids a second error variant with no unique recovery
//!   path.
//! * Stored blob decode failure or dial failure → also `Offline`. The
//!   address record is either stale (peer's addr rotated) or the peer is
//!   genuinely unreachable; both cases correct themselves the next time
//!   the peer dispatches to us or pairing refreshes the record.
//! * Stream write / read I/O failure →
//!   [`ClipboardDispatchError::Io`].
//! * Peer sent a single-byte ack other than Accepted / DuplicateIgnored →
//!   [`ClipboardDispatchError::PeerRejected`] with the code embedded.
//! * Local boundary check (oversized payload, etc) refuses the payload
//!   before any wire activity →
//!   [`ClipboardDispatchError::LocalPolicyExceeded`]. This is **not** a
//!   peer-side rejection — peer was never contacted; caller is expected
//!   to route via blob ref / file transfer instead of retrying this peer.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use iroh::endpoint::Connection;
use iroh::{Endpoint, EndpointAddr};
use tokio::sync::broadcast;
use tracing::{instrument, Instrument};

use uc_core::ids::DeviceId;
use uc_core::ports::{
    ClipboardDispatchError, ClipboardDispatchPort, ClipboardHeader, ConnectionChannel, DispatchAck,
    DispatchReport, PeerAddressRepositoryPort, PeerReachabilityPort, SyncPayload,
};
use uc_observability_contract::diagnostics::{
    complete_operation, operation_span, DiagnosticDomain, DiagnosticErrorType, DiagnosticOperation,
    DiagnosticRole, DiagnosticSpanKind, OperationCompletion, OperationContext,
};
use uc_observability_contract::{error_source::io_error_kind, uc_debug, uc_warn};

use super::admitted_connections::AdmittedConnections;
use super::conn_path::connection_channel;
use super::connect::{connect_with_staggered_retry, StaggeredDialError};
use super::peer_address_resolver::PeerAddressResolver;
use super::trace_context::inject_current;
use uc_sync_protocol::clipboard::{self as clipboard_wire, AckCode, WireEncodeError};

/// ALPN identifier for the Slice 2 clipboard sync protocol. Independent of
/// the peer_reachability / pairing ALPNs so the Router can multiplex all three
/// transports on the same endpoint.
pub const CLIPBOARD_ALPN: &[u8] = b"uniclipboard/clipboard-applied/1";
#[cfg(test)]
pub const LEGACY_CLIPBOARD_ALPN: &[u8] = b"uniclipboard/clipboard/0";

/// Result a single in-flight dial broadcasts to every follower waiting
/// on the same peer's single-flight slot. `Connection: Clone` makes
/// fan-out cheap (each follower gets its own handle on the same QUIC
/// connection); the failure branch shares the leader's typed dial error so
/// followers keep the same root cause the leader observed.
type DialResult = Result<Connection, Arc<StaggeredDialError>>;

/// 回执等待上限：接收端在应用结算前不会回执，其结算上限是 60 秒，
/// 这里在其上留出余量，只兜住对端永不回执的情况。
const ACK_TIMEOUT: Duration = Duration::from_secs(75);

type DialSlots = HashMap<DeviceId, broadcast::Sender<DialResult>>;

/// 领头方持有的单飞条目。领头方的 future 在拨号中途被丢弃时，条目随之移除，
/// 跟随方因此收到“领头方丢失”并重新竞争，而不是永远等一个不会再有的结果。
struct LeaderSlot {
    slots: Arc<Mutex<DialSlots>>,
    target: DeviceId,
    armed: bool,
}

impl LeaderSlot {
    fn remove(&self) {
        self.slots
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.target);
    }

    /// 正常收尾：先移除条目再广播结果，之后的调用方会开启新一轮拨号。
    fn release(&mut self) {
        if std::mem::take(&mut self.armed) {
            self.remove();
        }
    }
}

impl Drop for LeaderSlot {
    fn drop(&mut self) {
        if self.armed {
            self.remove();
        }
    }
}

/// 单飞拨号的失败：领头方的拨号失败，或跟随方没有收到领头方的结果。
#[derive(Debug, thiserror::Error)]
enum SingleFlightDialError {
    #[error("single-flight dial failed")]
    Dial(#[source] Arc<StaggeredDialError>),
    #[error("single-flight follower lost leader broadcast")]
    LeaderLost(#[source] broadcast::error::RecvError),
}

pub struct IrohClipboardDispatchAdapter {
    endpoint: Arc<Endpoint>,
    peer_address_resolver: PeerAddressResolver,
    /// PeerReachabilityPort handle the adapter notifies on dial failure. A dial
    /// that we have to fall back to `Offline` is first-hand evidence the
    /// peer is unreachable; feeding that signal back through
    /// [`PeerReachabilityPort::report_communication_failure`] lets every other consumer of peer_reachability
    /// (roster view, fan-out skip logic) observe the truth without waiting
    /// for the keepalive worker's next probe cycle.
    peer_reachability: Arc<dyn PeerReachabilityPort>,
    /// Single-flight slot per destination device. Concurrent dispatches to
    /// the same peer collapse to one `connect_with_staggered_retry`
    /// invocation: the first caller becomes the leader (records its
    /// sender in this map, runs the dial, broadcasts the verdict), every
    /// later caller becomes a follower (subscribes and waits). See
    /// [`Self::dial_single_flight`].
    ///
    /// Pre-#886 a storm of N concurrent copies against the same offline
    /// peer kicked off N parallel staggered-retry loops (3·N raw `iroh
    /// connect` attempts) and N `report_communication_failure` calls; single-flight cuts
    /// both to 1.
    in_flight_dials: Arc<Mutex<DialSlots>>,
    /// 写完帧之后等待对端回执的上限。
    ack_timeout: Duration,
    /// 出站连接按目标设备登记；成员被移除时关闭它们，中止在途的发送。
    connections: AdmittedConnections,
}

impl IrohClipboardDispatchAdapter {
    pub fn new(
        endpoint: Arc<Endpoint>,
        peer_addr_repo: Arc<dyn PeerAddressRepositoryPort>,
        peer_reachability: Arc<dyn PeerReachabilityPort>,
    ) -> Self {
        Self {
            endpoint,
            peer_address_resolver: PeerAddressResolver::new(peer_addr_repo),
            peer_reachability,
            in_flight_dials: Arc::new(Mutex::new(HashMap::new())),
            ack_timeout: ACK_TIMEOUT,
            connections: AdmittedConnections::default(),
        }
    }

    /// 与会话内其他协议共享连接登记，撤销成员时由登记统一关闭。
    pub(crate) fn with_connections(mut self, connections: AdmittedConnections) -> Self {
        self.connections = connections;
        self
    }

    #[cfg(test)]
    fn with_ack_timeout(mut self, ack_timeout: Duration) -> Self {
        self.ack_timeout = ack_timeout;
        self
    }

    /// Dial `target` with at most one staggered-retry batch in flight per
    /// device. The first caller becomes the leader: it inserts a
    /// `broadcast::Sender` into [`Self::in_flight_dials`], drives the
    /// actual `connect_with_staggered_retry`, and broadcasts the verdict.
    /// Concurrent callers for the same device become followers and await
    /// the broadcast.
    ///
    /// On dial failure only the leader calls
    /// [`PeerReachabilityPort::report_communication_failure`]; followers inherit the verdict
    /// through the broadcast. This is what shrinks the storm metric in
    /// #886's acceptance table: N concurrent copies against an offline
    /// peer collapse from N·3 raw `iroh connect` attempts and N
    /// `report_communication_failure` calls to 3 and 1 respectively.
    async fn dial_single_flight(
        &self,
        target: &DeviceId,
        addr: EndpointAddr,
    ) -> Result<Connection, SingleFlightDialError> {
        enum Role {
            Leader(broadcast::Sender<DialResult>, LeaderSlot),
            Follower(broadcast::Receiver<DialResult>),
        }

        loop {
            // Capacity 8 is well above the expected fan-out (per-peer roster
            // size ≤ 10 in Slice 2; concurrent dispatches per peer realistically
            // ≤ 3) — broadcast slow-receiver lag is not in the failure modes
            // this slot needs to defend against.
            let role = {
                let mut map = self
                    .in_flight_dials
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                match map.get(target) {
                    Some(tx) => Role::Follower(tx.subscribe()),
                    None => {
                        let (tx, _) = broadcast::channel::<DialResult>(8);
                        map.insert(*target, tx.clone());
                        let slot = LeaderSlot {
                            slots: Arc::clone(&self.in_flight_dials),
                            target: *target,
                            armed: true,
                        };
                        Role::Leader(tx, slot)
                    }
                }
            };

            match role {
                Role::Leader(tx, mut slot) => {
                    let result = connect_with_staggered_retry(
                        Arc::clone(&self.endpoint),
                        addr,
                        CLIPBOARD_ALPN,
                        "clipboard",
                        uc_observability_contract::diagnostics::connectivity::AddressInputSource::Stored,
                    )
                    .await
                    .map_err(Arc::new);

                    // First-hand dial verdict — fold report_communication_failure into the
                    // leader's tail so concurrent followers piling on the
                    // same dead peer collapse to a single side-effect rather
                    // than each calling `peer_reachability.report_communication_failure` on their own
                    // failure return.
                    if let Err(ref err) = result {
                        uc_debug!(
                            error_kind = "dial_failed",
                            io_error_kind = io_error_kind(err),
                            "clipboard dispatch: single-flight dial failed; marking offline"
                        );
                        self.peer_reachability
                            .report_communication_failure(target)
                            .await;
                    }

                    // Remove the slot before broadcasting so any caller that
                    // races back in after we send sees an empty map and
                    // starts a fresh dial cycle.
                    slot.release();
                    let _ = tx.send(result.clone());
                    return result.map_err(SingleFlightDialError::Dial);
                }
                Role::Follower(mut rx) => match rx.recv().await {
                    Ok(result) => return result.map_err(SingleFlightDialError::Dial),
                    // 领头方的 future 被丢弃，没有结果可继承：重新竞争成为领头方。
                    Err(broadcast::error::RecvError::Closed) => continue,
                    Err(err) => return Err(SingleFlightDialError::LeaderLost(err)),
                },
            }
        }
    }

    /// Resolve the peer's current [`EndpointAddr`] from the repository.
    /// Returns `Ok(None)` when no record exists (maps to `Offline`) so the
    /// dispatch path stays branchless; any other failure (postcard / repo
    /// infra) surfaces as `Offline` after logging — a corrupt blob is a
    /// data-integrity issue the next pairing sync will self-heal.
    async fn resolve_addr(&self, target: &DeviceId) -> Option<EndpointAddr> {
        match self.peer_address_resolver.resolve(target).await {
            Ok(address) => address,
            Err(error) => {
                uc_warn!(
                    error_kind = error.kind(),
                    "clipboard dispatch address resolution failed; treating peer as offline"
                );
                None
            }
        }
    }

    /// Open one bi-stream, write the framed header + ciphertext, read the
    /// peer's one-byte ack, and interpret it. The caller owns the
    /// connection lifecycle (close happens when the handle drops).
    async fn send_and_ack(
        &self,
        connection: &Connection,
        header: &ClipboardHeader,
        payload: &SyncPayload,
    ) -> Result<DispatchAck, ClipboardDispatchError> {
        let stream = connection.open_bi().await;
        let (mut send, mut recv) = stream.map_err(|err| {
            ClipboardDispatchError::Io(anyhow::Error::from(err).context("open_bi").into())
        })?;

        // Write the frame + close the send half so the peer's read_exact on
        // the payload length / body reaches a terminal state.
        let frame_write = match clipboard_wire::write_frame(
            &mut send,
            header,
            inject_current(),
            &payload.ciphertext,
        )
        .await
        {
            Ok(()) => send.finish().map_err(|err| {
                ClipboardDispatchError::Io(anyhow::Error::from(err).context("send.finish").into())
            }),
            Err(error) => Err(map_encode_err(error)),
        };
        frame_write?;

        let mut ack_buf = [0u8; 1];
        match tokio::time::timeout(self.ack_timeout, recv.read_exact(&mut ack_buf)).await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                return Err(ClipboardDispatchError::Io(
                    anyhow::Error::from(err).context("ack read").into(),
                ));
            }
            Err(elapsed) => {
                return Err(ClipboardDispatchError::Io(
                    anyhow::Error::from(elapsed)
                        .context("ack read timed out")
                        .into(),
                ));
            }
        }

        // Any unknown code is adapter-level rejection rather than an ignored
        // success.
        match AckCode::try_from(ack_buf[0]) {
            Ok(AckCode::Accepted) => Ok(DispatchAck::Accepted),
            Ok(AckCode::DuplicateIgnored) => Ok(DispatchAck::DuplicateIgnored),
            Ok(AckCode::Incompatible) => Err(ClipboardDispatchError::PeerIncompatible),
            Ok(AckCode::Rejected) => Err(ClipboardDispatchError::PeerRejected(
                "peer returned Rejected ack".to_string(),
            )),
            // 未知回执字节只是协议分类；PeerRejected 保存的是拒绝原因文本，不承载下层错误。
            Err(_) => Err(ClipboardDispatchError::PeerRejected(
                "peer returned unknown ack byte".to_string(),
            )),
        }
    }
}

#[async_trait]
impl ClipboardDispatchPort for IrohClipboardDispatchAdapter {
    #[instrument(skip_all, fields(payload_len = payload.ciphertext.len()))]
    async fn dispatch(
        &self,
        target: &DeviceId,
        header: &ClipboardHeader,
        payload: SyncPayload,
    ) -> DispatchReport {
        // 1. Early-reject oversized payloads at the adapter boundary so
        //    the caller gets a clean error without us opening a stream
        //    just to tear it back down. This is a *local* policy check —
        //    no wire activity yet, peer never contacted. Surface as
        //    `LocalPolicyExceeded` so callers don't misread it as a peer
        //    decision (the caller's correct response is to re-dispatch
        //    via blob ref / file transfer, not to retry this peer).
        if payload.ciphertext.len() > clipboard_wire::MAX_PAYLOAD_SIZE as usize {
            return DispatchReport {
                transport: ConnectionChannel::Unknown,
                outcome: Err(ClipboardDispatchError::LocalPolicyExceeded(format!(
                    "ciphertext {} bytes exceeds wire MAX_PAYLOAD_SIZE {}",
                    payload.ciphertext.len(),
                    clipboard_wire::MAX_PAYLOAD_SIZE
                ))),
            };
        }

        // 成员在拨号期间被移除时，迟到的连接不得再用来发送。
        let revocation_epoch = self.connections.epoch(target);

        // 2. Resolve address; missing / bad record = offline. No dial, so
        //    no path established → transport Unknown.
        let started = Instant::now();
        let span = network_span(DiagnosticOperation::ClipboardAddressResolve);
        let addr = self.resolve_addr(target).instrument(span.clone()).await;
        span.in_scope(|| {
            network_completion(
                DiagnosticOperation::ClipboardAddressResolve,
                DiagnosticErrorType::AddressUnavailable,
                started,
                addr.is_some(),
            )
        });
        drop(span);
        let addr = match addr {
            Some(a) => a,
            None => {
                return DispatchReport {
                    transport: ConnectionChannel::Unknown,
                    outcome: Err(ClipboardDispatchError::Offline),
                }
            }
        };
        // 3. Dial via the per-peer single-flight slot so a concurrent
        //    dispatch storm against the same offline peer collapses to one
        //    staggered-retry batch + one `report_communication_failure`. Dial failure =
        //    offline (no typed iroh error leaks up); the leader has
        //    already fed the verdict to PeerReachabilityPort so this branch only
        //    has to surface the public error. No path established →
        //    transport Unknown.
        let started = Instant::now();
        let span = network_span(DiagnosticOperation::ClipboardConnect);
        let connection = self
            .dial_single_flight(target, addr)
            .instrument(span.clone())
            .await;
        span.in_scope(|| {
            network_completion(
                DiagnosticOperation::ClipboardConnect,
                DiagnosticErrorType::ConnectFailed,
                started,
                connection.is_ok(),
            )
        });
        drop(span);
        let connection = match connection {
            Ok(connection) => connection,
            Err(err) => {
                uc_debug!(
                    error_kind = "dial_failed",
                    io_error_kind = io_error_kind(&err),
                    "clipboard dispatch: dial failed (single-flight), treating as Offline"
                );
                return DispatchReport {
                    transport: ConnectionChannel::Unknown,
                    outcome: Err(ClipboardDispatchError::Offline),
                };
            }
        };

        // 4. Write the frame + read the ack on a fresh bi-stream. The
        //    registration lets a member removal close this connection and
        //    abort the send instead of waiting for the ack timeout.
        let Some(_registration) =
            self.connections
                .register_since(target, &connection, revocation_epoch)
        else {
            // 拨号期间成员已被移除：丢弃刚建立的连接，不发送内容。
            drop(connection);
            return DispatchReport {
                transport: ConnectionChannel::Unknown,
                outcome: Err(ClipboardDispatchError::Offline),
            };
        };
        let outcome = self.send_and_ack(&connection, header, &payload).await;

        // 5. Classify the path this connection selected while it is still
        //    alive; the snapshot is taken after the send/ack settles and does
        //    not prove the path stayed unchanged during the exchange.
        let transport = connection_channel(&connection);

        // 6. Close the connection. Adapter owns the lifecycle — dropping
        //    here signals `Connection::closed` on the peer. The Q4
        //    per-dispatch-stream decision (see plan §3.1) means we do not
        //    cache connections.
        drop(connection);

        DispatchReport { transport, outcome }
    }
}

fn network_span(operation: DiagnosticOperation) -> tracing::Span {
    operation_span(OperationContext {
        domain: DiagnosticDomain::Clipboard,
        operation,
        role: DiagnosticRole::Client,
        kind: DiagnosticSpanKind::Internal,
    })
}

fn network_completion(
    operation: DiagnosticOperation,
    error: DiagnosticErrorType,
    started: Instant,
    succeeded: bool,
) {
    complete_operation(if succeeded {
        OperationCompletion::succeeded(
            DiagnosticDomain::Clipboard,
            operation,
            DiagnosticRole::Client,
            started.elapsed(),
        )
    } else {
        OperationCompletion::failed(
            DiagnosticDomain::Clipboard,
            operation,
            DiagnosticRole::Client,
            error,
            started.elapsed(),
        )
    });
}

/// Map wire-encoding failures into the public error type without leaking
/// postcard / io internals upward. `HeaderTooLarge` is an adapter-side bug
/// (the header we built is oversized), so we surface it as `Internal` even
/// though the wire type name suggests otherwise.
fn map_encode_err(err: WireEncodeError) -> ClipboardDispatchError {
    match err {
        WireEncodeError::Io(ioerr) => {
            ClipboardDispatchError::Io(anyhow::Error::new(ioerr).context("frame write").into())
        }
        WireEncodeError::PayloadTooLarge { size, max } => {
            // wire codec also enforces MAX_PAYLOAD_SIZE; if we somehow get
            // here (the upstream early-reject in `dispatch` should have
            // caught it first), surface the same `LocalPolicyExceeded`
            // semantics — peer was never reached.
            ClipboardDispatchError::LocalPolicyExceeded(format!(
                "wire codec payload {size} bytes exceeds local maximum {max}"
            ))
        }
        WireEncodeError::HeaderTooLarge { size, max } => ClipboardDispatchError::Internal(
            format!("self-built header {size} bytes exceeds {max}").into(),
        ),
        WireEncodeError::Postcard(err) => ClipboardDispatchError::Internal(
            anyhow::Error::from(err).context("header encode").into(),
        ),
        WireEncodeError::UnsupportedVersion(version) => ClipboardDispatchError::Internal(
            format!("unsupported outbound wire version {version}").into(),
        ),
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
#[path = "clipboard_dispatch_tests.rs"]
mod tests;
