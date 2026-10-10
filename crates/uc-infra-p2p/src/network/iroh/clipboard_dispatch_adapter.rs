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
mod tests {
    use super::*;

    use async_trait::async_trait;
    use bytes::Bytes;
    use chrono::Utc;
    use iroh::protocol::{AcceptError, ProtocolHandler, Router};
    use iroh::{Endpoint, RelayMode};
    use tokio::sync::broadcast;

    use uc_core::ports::{
        PeerAddressError, PeerAddressRecord, PeerReachabilityChanged, PeerReachabilityError,
        ReachabilityState,
    };

    // PeerReachabilityPort mock for the dispatch tests. None of the four tests in
    // this module reach the dial-failure path (happy ack, duplicate ack,
    // missing peer_addr short-circuit, oversized local reject), so the
    // adapter never invokes `report_communication_failure`. The mock therefore needs no
    // expectations — any accidental call surfaces as a mockall panic, which
    // is exactly the regression guard we want for "dispatch tests shouldn't
    // be touching peer_reachability state."
    //
    // `report_communication_failure` is omitted intentionally: it has a default impl on the
    // trait (noop), so leaving it off the mock keeps that default in play
    // without forcing every test to wire an empty expectation.
    mockall::mock! {
        PeerReachability {}

        #[async_trait]
        impl PeerReachabilityPort for PeerReachability {
            async fn ensure_reachable(
                &self,
                device: &DeviceId,
            ) -> Result<ReachabilityState, PeerReachabilityError>;
            async fn current_state(&self, device: &DeviceId) -> ReachabilityState;
            fn subscribe(&self) -> broadcast::Receiver<PeerReachabilityChanged>;
        }
    }

    fn peer_reachability_mock() -> Arc<dyn PeerReachabilityPort> {
        Arc::new(MockPeerReachability::new())
    }

    /// In-memory peer_addr_repo the tests use to inject an address blob
    /// for a target device. Mirrors the surface the adapter needs without
    /// pulling in the Diesel implementation.
    #[derive(Default)]
    struct MemRepo {
        inner: tokio::sync::Mutex<std::collections::HashMap<String, PeerAddressRecord>>,
    }

    #[async_trait]
    impl PeerAddressRepositoryPort for MemRepo {
        async fn get(
            &self,
            device: &DeviceId,
        ) -> Result<Option<PeerAddressRecord>, PeerAddressError> {
            Ok(self.inner.lock().await.get(device.as_str()).cloned())
        }
        async fn upsert(&self, record: &PeerAddressRecord) -> Result<(), PeerAddressError> {
            self.inner
                .lock()
                .await
                .insert(record.device_id.as_str().to_string(), record.clone());
            Ok(())
        }
        async fn list(&self) -> Result<Vec<PeerAddressRecord>, PeerAddressError> {
            Ok(self.inner.lock().await.values().cloned().collect())
        }
        async fn remove(&self, device: &DeviceId) -> Result<(), PeerAddressError> {
            self.inner.lock().await.remove(device.as_str());
            Ok(())
        }
    }

    async fn bind_endpoint() -> Arc<Endpoint> {
        Arc::new(
            Endpoint::builder(iroh::endpoint::presets::N0)
                .alpns(vec![CLIPBOARD_ALPN.to_vec()])
                .relay_mode(RelayMode::Disabled)
                .bind()
                .await
                .expect("bind endpoint"),
        )
    }

    async fn wait_for_direct_addrs(endpoint: &Endpoint) {
        for _ in 0..100 {
            if !endpoint.addr().addrs.is_empty() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("endpoint never published direct addresses");
    }

    /// Stash an iroh `EndpointAddr` into the mem repo under the given
    /// device id, postcard-encoded the same way the pairing wire does.
    async fn seed_addr(repo: &MemRepo, device: &DeviceId, addr: &EndpointAddr) {
        let blob = postcard::to_stdvec(addr).expect("postcard encode EndpointAddr");
        repo.upsert(&PeerAddressRecord {
            device_id: device.clone(),
            addr_blob: blob,
            observed_at: Utc::now(),
        })
        .await
        .expect("upsert");
    }

    fn sample_header() -> ClipboardHeader {
        ClipboardHeader {
            version: ClipboardHeader::CURRENT_VERSION,
            snapshot_hash: "c".repeat(64),
            captured_at_ms: 1_700_000_000_000,
            origin_device_id: "sender-001".to_string(),
            origin_device_name: "Sender".to_string(),
            payload_version: 3,
        }
    }

    /// Test-only handler that echoes a fixed ack byte after draining the
    /// frame. Stored as `Arc<[u8; 1]>` so each spawned task pulls the same
    /// configured ack.
    #[derive(Clone)]
    struct FixedAckHandler {
        ack_byte: u8,
    }

    impl std::fmt::Debug for FixedAckHandler {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("FixedAckHandler")
                .field("ack", &format!("0x{:02X}", self.ack_byte))
                .finish()
        }
    }

    impl ProtocolHandler for FixedAckHandler {
        async fn accept(&self, connection: iroh::endpoint::Connection) -> Result<(), AcceptError> {
            let ack = self.ack_byte;
            let (mut send, mut recv) = connection
                .accept_bi()
                .await
                .map_err(|err| AcceptError::from_err(err))?;
            // Consume one frame; failure here is fine in negative tests
            // that send bad headers.
            let _ = clipboard_wire::read_frame(&mut recv).await;
            // Emit the configured ack byte regardless.
            send.write_all(&[ack])
                .await
                .map_err(|err| AcceptError::from_err(err))?;
            send.finish().map_err(|err| AcceptError::from_err(err))?;
            let _ = connection.closed().await;
            Ok(())
        }
    }

    async fn spawn_ack_endpoint(ack: u8) -> (Arc<Endpoint>, Router) {
        let endpoint = bind_endpoint().await;
        wait_for_direct_addrs(&endpoint).await;
        let router = Router::builder((*endpoint).clone())
            .accept(CLIPBOARD_ALPN, FixedAckHandler { ack_byte: ack })
            .spawn();
        (endpoint, router)
    }

    /// Verdict 1 — happy path. Peer is reachable + accepts the frame.
    /// Dispatch returns `DispatchAck::Accepted`.
    #[tokio::test]
    async fn dispatch_returns_accepted_on_peer_ack_0x01() {
        let (peer_endpoint, peer_router) = spawn_ack_endpoint(AckCode::Accepted.as_byte()).await;
        let peer_addr = peer_endpoint.addr();

        let sender_endpoint = bind_endpoint().await;
        wait_for_direct_addrs(&sender_endpoint).await;
        let repo = Arc::new(MemRepo::default());
        let target = DeviceId::new("target-alpha");
        seed_addr(&repo, &target, &peer_addr).await;

        let adapter =
            IrohClipboardDispatchAdapter::new(sender_endpoint, repo, peer_reachability_mock());
        let payload = SyncPayload {
            ciphertext: Bytes::from(vec![0x11, 0x22, 0x33, 0x44]),
        };

        let ack = adapter
            .dispatch(&target, &sample_header(), payload)
            .await
            .outcome
            .expect("dispatch succeeds");
        assert_eq!(ack, DispatchAck::Accepted);

        peer_router.shutdown().await.expect("router shutdown");
    }

    /// Verdict 2 — peer returns the duplicate-ignored ack. Dispatch still
    /// returns `Ok`, and the specific variant propagates so the use case
    /// can report it distinct from `Accepted`.
    #[tokio::test]
    async fn dispatch_returns_duplicate_on_peer_ack_0x02() {
        let (peer_endpoint, peer_router) =
            spawn_ack_endpoint(AckCode::DuplicateIgnored.as_byte()).await;
        let peer_addr = peer_endpoint.addr();

        let sender_endpoint = bind_endpoint().await;
        wait_for_direct_addrs(&sender_endpoint).await;
        let repo = Arc::new(MemRepo::default());
        let target = DeviceId::new("target-beta");
        seed_addr(&repo, &target, &peer_addr).await;

        let adapter =
            IrohClipboardDispatchAdapter::new(sender_endpoint, repo, peer_reachability_mock());
        let payload = SyncPayload {
            ciphertext: Bytes::from(vec![0xAA; 16]),
        };

        let ack = adapter
            .dispatch(&target, &sample_header(), payload)
            .await
            .outcome
            .expect("dispatch succeeds");
        assert_eq!(ack, DispatchAck::DuplicateIgnored);

        peer_router.shutdown().await.expect("router shutdown");
    }

    #[tokio::test]
    async fn dispatch_maps_incompatible_ack_to_the_public_upgrade_error() {
        let (peer_endpoint, peer_router) =
            spawn_ack_endpoint(AckCode::Incompatible.as_byte()).await;
        let peer_addr = peer_endpoint.addr();
        let sender_endpoint = bind_endpoint().await;
        wait_for_direct_addrs(&sender_endpoint).await;
        let repo = Arc::new(MemRepo::default());
        let target = DeviceId::new("old-engine-target");
        seed_addr(&repo, &target, &peer_addr).await;
        let adapter =
            IrohClipboardDispatchAdapter::new(sender_endpoint, repo, peer_reachability_mock());

        let outcome = adapter
            .dispatch(
                &target,
                &sample_header(),
                SyncPayload {
                    ciphertext: Bytes::from_static(b"ciphertext"),
                },
            )
            .await
            .outcome;

        assert!(matches!(
            outcome,
            Err(ClipboardDispatchError::PeerIncompatible)
        ));
        peer_router.shutdown().await.expect("router shutdown");
    }

    // 流程：B 只监听 0.19 的旧内容入口；A 发送内容时只尝试 1.1 当前入口，不回退或发送旧请求。
    #[tokio::test]
    async fn dispatch_does_not_fall_back_to_the_v019_clipboard_endpoint() {
        let peer_endpoint = bind_endpoint().await;
        wait_for_direct_addrs(&peer_endpoint).await;
        let peer_router = Router::builder((*peer_endpoint).clone())
            .accept(
                LEGACY_CLIPBOARD_ALPN,
                FixedAckHandler {
                    ack_byte: AckCode::Accepted.as_byte(),
                },
            )
            .spawn();
        let peer_addr = peer_endpoint.addr();

        let sender_endpoint = bind_endpoint().await;
        wait_for_direct_addrs(&sender_endpoint).await;
        let repo = Arc::new(MemRepo::default());
        let target = DeviceId::new("legacy-target");
        seed_addr(&repo, &target, &peer_addr).await;

        let adapter =
            IrohClipboardDispatchAdapter::new(sender_endpoint, repo, peer_reachability_mock());
        let result = adapter
            .dispatch(
                &target,
                &sample_header(),
                SyncPayload {
                    ciphertext: Bytes::from_static(b"must-not-reach-legacy-ingest"),
                },
            )
            .await
            .outcome;

        assert!(matches!(result, Err(ClipboardDispatchError::Offline)));
        peer_router.shutdown().await.expect("router shutdown");
    }

    /// Verdict 3 — missing peer_addr entry (peer never paired, or entry
    /// removed). Dispatch returns `Offline` without touching the network.
    #[tokio::test]
    async fn dispatch_returns_offline_when_peer_addr_missing() {
        let sender_endpoint = bind_endpoint().await;
        let repo = Arc::new(MemRepo::default());
        let adapter =
            IrohClipboardDispatchAdapter::new(sender_endpoint, repo, peer_reachability_mock());

        let result = adapter
            .dispatch(
                &DeviceId::new("never-paired"),
                &sample_header(),
                SyncPayload {
                    ciphertext: Bytes::from_static(b"irrelevant"),
                },
            )
            .await
            .outcome;
        match result {
            Err(ClipboardDispatchError::Offline) => {}
            other => panic!("expected Offline, got {other:?}"),
        }
    }

    /// Presence fake that counts `report_communication_failure` calls instead of asserting
    /// they never happen. Reserved for the single-flight tests below
    /// (verdicts 5 and 6) — the happy-path tests above keep using
    /// `peer_reachability_mock()` so any accidental call still panics.
    struct CountingPeerReachability {
        communication_failure_calls: Arc<std::sync::atomic::AtomicUsize>,
        events: broadcast::Sender<PeerReachabilityChanged>,
    }

    impl CountingPeerReachability {
        fn new() -> Self {
            let (events, _) = broadcast::channel(8);
            Self {
                communication_failure_calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                events,
            }
        }
    }

    #[async_trait]
    impl PeerReachabilityPort for CountingPeerReachability {
        async fn ensure_reachable(
            &self,
            _device: &DeviceId,
        ) -> Result<ReachabilityState, PeerReachabilityError> {
            Ok(ReachabilityState::Unknown)
        }
        async fn current_state(&self, _device: &DeviceId) -> ReachabilityState {
            ReachabilityState::Unknown
        }
        async fn report_communication_failure(&self, _device: &DeviceId) {
            self.communication_failure_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        fn subscribe(&self) -> broadcast::Receiver<PeerReachabilityChanged> {
            self.events.subscribe()
        }
    }

    /// Verdict 5 — single-flight collapse. Two dispatches racing against
    /// the same offline peer must produce exactly one `report_communication_failure` call
    /// (the leader's) — followers inherit the verdict through the
    /// broadcast and surface their own `Offline` without re-stamping
    /// peer_reachability. This is the storm-axis half of #886's acceptance table.
    #[tokio::test]
    async fn concurrent_dispatch_to_offline_peer_reports_communication_failure_once() {
        let sender_endpoint = bind_endpoint().await;
        wait_for_direct_addrs(&sender_endpoint).await;

        // Seed an unroutable addr blob: a fresh random EndpointId with
        // no transport addrs and relay disabled on the sender, so the
        // staggered retry exhausts its three attempts without ever
        // dialling a live peer.
        let dead_id = iroh::SecretKey::generate().public();
        let dead_addr = EndpointAddr::new(dead_id);

        let target = DeviceId::new("offline-storm-target");
        let repo = Arc::new(MemRepo::default());
        seed_addr(&repo, &target, &dead_addr).await;

        let peer_reachability = Arc::new(CountingPeerReachability::new());
        let communication_failure_calls =
            Arc::clone(&peer_reachability.communication_failure_calls);

        let adapter = Arc::new(IrohClipboardDispatchAdapter::new(
            sender_endpoint.clone(),
            repo,
            peer_reachability,
        ));

        let header = sample_header();
        let payload_a = SyncPayload {
            ciphertext: Bytes::from_static(b"first"),
        };
        let payload_b = SyncPayload {
            ciphertext: Bytes::from_static(b"second"),
        };

        let adapter_a = Arc::clone(&adapter);
        let adapter_b = Arc::clone(&adapter);
        let target_a = target.clone();
        let target_b = target.clone();
        let header_a = header.clone();
        let header_b = header.clone();

        let (result_a, result_b) = tokio::join!(
            async move {
                adapter_a
                    .dispatch(&target_a, &header_a, payload_a)
                    .await
                    .outcome
            },
            async move {
                adapter_b
                    .dispatch(&target_b, &header_b, payload_b)
                    .await
                    .outcome
            },
        );

        match (result_a, result_b) {
            (Err(ClipboardDispatchError::Offline), Err(ClipboardDispatchError::Offline)) => {}
            other => panic!("both dispatches should report Offline; got {other:?}"),
        }

        let calls = communication_failure_calls.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            calls, 1,
            "single-flight must collapse report_communication_failure to one call; got {calls}",
        );

        // Slot must be empty after the leader finishes so a later
        // dispatch starts a fresh dial cycle (no stale in-flight entry).
        assert!(
            adapter.in_flight_dials.lock().unwrap().is_empty(),
            "in_flight_dials slot must be released after the leader broadcasts",
        );
    }

    /// 领头方的 future 在拨号中途被丢弃后，同一目标的后续发送不能永远等一个
    /// 不会再有结果的广播，而应重新竞争并给出自己的结果。
    #[tokio::test]
    async fn dropped_dial_leader_does_not_block_later_dispatches() {
        let sender_endpoint = bind_endpoint().await;
        wait_for_direct_addrs(&sender_endpoint).await;
        let dead_addr = EndpointAddr::new(iroh::SecretKey::generate().public());
        let target = DeviceId::new("dropped-leader-target");
        let repo = Arc::new(MemRepo::default());
        seed_addr(&repo, &target, &dead_addr).await;
        let adapter = Arc::new(IrohClipboardDispatchAdapter::new(
            sender_endpoint,
            repo,
            Arc::new(CountingPeerReachability::new()),
        ));

        let leader = {
            let adapter = Arc::clone(&adapter);
            let target = target.clone();
            tokio::spawn(async move {
                adapter
                    .dispatch(
                        &target,
                        &sample_header(),
                        SyncPayload {
                            ciphertext: Bytes::from_static(b"leader"),
                        },
                    )
                    .await
            })
        };
        // 等领头方登记了单飞条目、进入拨号后再丢弃它。
        for _ in 0..200 {
            if !adapter.in_flight_dials.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            !adapter.in_flight_dials.lock().unwrap().is_empty(),
            "leader never registered its dial"
        );
        leader.abort();
        let _ = leader.await;

        let later = tokio::time::timeout(
            Duration::from_secs(12),
            adapter.dispatch(
                &target,
                &sample_header(),
                SyncPayload {
                    ciphertext: Bytes::from_static(b"later"),
                },
            ),
        )
        .await
        .expect("a later dispatch must not wait on a dropped leader");
        assert!(matches!(
            later.outcome,
            Err(ClipboardDispatchError::Offline)
        ));
    }

    /// 对端收下帧却永不回执时，发送必须在回执上限内以 I/O 失败返回。
    #[tokio::test]
    async fn dispatch_returns_when_the_peer_never_acks() {
        #[derive(Debug, Clone)]
        struct SilentHandler;
        impl ProtocolHandler for SilentHandler {
            async fn accept(
                &self,
                connection: iroh::endpoint::Connection,
            ) -> Result<(), AcceptError> {
                let (_send, mut recv) = connection
                    .accept_bi()
                    .await
                    .map_err(|err| AcceptError::from_err(err))?;
                let _ = clipboard_wire::read_frame(&mut recv).await;
                let _ = connection.closed().await;
                Ok(())
            }
        }

        let peer_endpoint = bind_endpoint().await;
        wait_for_direct_addrs(&peer_endpoint).await;
        let peer_router = Router::builder((*peer_endpoint).clone())
            .accept(CLIPBOARD_ALPN, SilentHandler)
            .spawn();
        let sender_endpoint = bind_endpoint().await;
        wait_for_direct_addrs(&sender_endpoint).await;
        let repo = Arc::new(MemRepo::default());
        let target = DeviceId::new("silent-target");
        seed_addr(&repo, &target, &peer_endpoint.addr()).await;
        let adapter =
            IrohClipboardDispatchAdapter::new(sender_endpoint, repo, peer_reachability_mock())
                .with_ack_timeout(Duration::from_millis(500));

        let report = tokio::time::timeout(
            Duration::from_secs(8),
            adapter.dispatch(
                &target,
                &sample_header(),
                SyncPayload {
                    ciphertext: Bytes::from_static(b"never acked"),
                },
            ),
        )
        .await
        .expect("dispatch must give up when no ack arrives");
        assert!(matches!(report.outcome, Err(ClipboardDispatchError::Io(_))));
        peer_router.shutdown().await.expect("router shutdown");
    }

    /// 在 `before_connect` 处计数并可暂停拨号，用来观察拨号尝试并在中途撤销成员。
    #[derive(Debug, Clone)]
    struct GatedDials {
        started: Arc<std::sync::atomic::AtomicUsize>,
        open: tokio::sync::watch::Receiver<bool>,
    }

    impl iroh::endpoint::EndpointHooks for GatedDials {
        fn before_connect<'a>(
            &'a self,
            _: &'a EndpointAddr,
            _: &'a [u8],
        ) -> impl std::future::Future<Output = iroh::endpoint::BeforeConnectOutcome> + Send + 'a
        {
            async move {
                self.started
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let mut open = self.open.clone();
                let _ = open.wait_for(|open| *open).await;
                iroh::endpoint::BeforeConnectOutcome::Accept
            }
        }
    }

    async fn bind_gated_sender(gate: GatedDials) -> Arc<Endpoint> {
        let endpoint = Arc::new(
            Endpoint::builder(iroh::endpoint::presets::N0)
                .alpns(vec![CLIPBOARD_ALPN.to_vec()])
                .relay_mode(RelayMode::Disabled)
                .hooks(gate)
                .bind()
                .await
                .expect("bind sender"),
        );
        wait_for_direct_addrs(&endpoint).await;
        endpoint
    }

    /// 一次发送到不可达对端最多产生 3 次拨号尝试（现有错峰拨号的次数）。
    #[tokio::test]
    async fn failed_dispatch_makes_at_most_three_dial_attempts() {
        let started = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (_open_tx, open) = tokio::sync::watch::channel(true);
        let sender = bind_gated_sender(GatedDials {
            started: Arc::clone(&started),
            open,
        })
        .await;
        let dead_addr = EndpointAddr::new(iroh::SecretKey::generate().public());
        let target = DeviceId::new("unreachable-target");
        let repo = Arc::new(MemRepo::default());
        seed_addr(&repo, &target, &dead_addr).await;
        let adapter = IrohClipboardDispatchAdapter::new(
            sender,
            repo,
            Arc::new(CountingPeerReachability::new()),
        );

        let report = adapter
            .dispatch(
                &target,
                &sample_header(),
                SyncPayload {
                    ciphertext: Bytes::from_static(b"unreachable"),
                },
            )
            .await;

        assert!(matches!(
            report.outcome,
            Err(ClipboardDispatchError::Offline)
        ));
        let attempts = started.load(std::sync::atomic::Ordering::SeqCst);
        assert!(
            (1..=3).contains(&attempts),
            "one failed send made {attempts} dial attempts"
        );
    }

    /// 拨号进行到一半时成员被移除：拨号即使随后成功，也不能把内容发给已移除的成员。
    #[tokio::test]
    async fn dial_finishing_after_member_removal_sends_nothing() {
        #[derive(Clone)]
        struct CountingAckHandler(Arc<std::sync::atomic::AtomicUsize>);
        impl std::fmt::Debug for CountingAckHandler {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct("CountingAckHandler").finish()
            }
        }
        impl ProtocolHandler for CountingAckHandler {
            async fn accept(
                &self,
                connection: iroh::endpoint::Connection,
            ) -> Result<(), AcceptError> {
                let (mut send, mut recv) = connection
                    .accept_bi()
                    .await
                    .map_err(|err| AcceptError::from_err(err))?;
                if clipboard_wire::read_frame(&mut recv).await.is_ok() {
                    self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
                send.write_all(&[AckCode::Accepted.as_byte()])
                    .await
                    .map_err(|err| AcceptError::from_err(err))?;
                send.finish().map_err(|err| AcceptError::from_err(err))?;
                let _ = connection.closed().await;
                Ok(())
            }
        }

        let frames = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let peer_endpoint = bind_endpoint().await;
        wait_for_direct_addrs(&peer_endpoint).await;
        let peer_router = Router::builder((*peer_endpoint).clone())
            .accept(CLIPBOARD_ALPN, CountingAckHandler(Arc::clone(&frames)))
            .spawn();

        let started = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (open_tx, open) = tokio::sync::watch::channel(false);
        let sender = bind_gated_sender(GatedDials {
            started: Arc::clone(&started),
            open,
        })
        .await;
        let target = DeviceId::new("removed-during-dial");
        let repo = Arc::new(MemRepo::default());
        seed_addr(&repo, &target, &peer_endpoint.addr()).await;
        let connections = AdmittedConnections::default();
        let adapter = Arc::new(
            IrohClipboardDispatchAdapter::new(sender, repo, peer_reachability_mock())
                .with_connections(connections.clone()),
        );

        let sending = tokio::spawn({
            let adapter = Arc::clone(&adapter);
            let target = target.clone();
            async move {
                adapter
                    .dispatch(
                        &target,
                        &sample_header(),
                        SyncPayload {
                            ciphertext: Bytes::from_static(b"must not arrive"),
                        },
                    )
                    .await
            }
        });
        for _ in 0..200 {
            if started.load(std::sync::atomic::Ordering::SeqCst) > 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            started.load(std::sync::atomic::Ordering::SeqCst) > 0,
            "the dial never started"
        );

        // 成员在拨号中途被移除（连接 owner 的 `forget` 即调用这一入口），随后拨号放行。
        connections.close_device(&target, b"forgotten");
        open_tx.send(true).expect("open the dial gate");

        let report = tokio::time::timeout(Duration::from_secs(10), sending)
            .await
            .expect("dispatch settles")
            .expect("dispatch task joins");
        assert!(report.outcome.is_err(), "a removed member was sent to");
        assert_eq!(
            frames.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "the removed member received the frame"
        );
        peer_router.shutdown().await.expect("router shutdown");
    }

    /// Verdict 4 — oversized payload. The adapter short-circuits before
    /// even dialing, returning `LocalPolicyExceeded` with a message that
    /// mentions the wire MAX_PAYLOAD_SIZE. Protects against wasted QUIC
    /// handshake on a payload that would fail the wire boundary anyway,
    /// and uses the dedicated local-policy variant so callers don't
    /// misread this as a peer-side rejection.
    #[tokio::test]
    async fn dispatch_rejects_oversized_payload_locally_without_dialing() {
        // No peer_addr seeded — proves the rejection is local; otherwise
        // we would hit Offline first.
        let sender_endpoint = bind_endpoint().await;
        let repo = Arc::new(MemRepo::default());
        let adapter =
            IrohClipboardDispatchAdapter::new(sender_endpoint, repo, peer_reachability_mock());

        let oversized = vec![0u8; clipboard_wire::MAX_PAYLOAD_SIZE as usize + 1];
        let result = adapter
            .dispatch(
                &DeviceId::new("irrelevant"),
                &sample_header(),
                SyncPayload {
                    ciphertext: Bytes::from(oversized),
                },
            )
            .await
            .outcome;
        match result {
            Err(ClipboardDispatchError::LocalPolicyExceeded(msg)) => {
                assert!(
                    msg.contains("exceeds wire MAX_PAYLOAD_SIZE"),
                    "unexpected reject msg: {msg}"
                );
            }
            other => panic!("expected LocalPolicyExceeded, got {other:?}"),
        }
    }
}
