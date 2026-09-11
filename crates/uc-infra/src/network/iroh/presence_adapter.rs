//! Iroh-backed implementation of [`PresencePort`] (Slice 2 Phase 1 · T3b).
//!
//! ## Design summary
//!
//! T3a's probe (see `uc-infra/tests/iroh_presence_probe.rs`) established two
//! load-bearing facts about iroh 0.95:
//!
//! 1. [`iroh::Endpoint::conn_type`] is a **cache**, not a liveness probe.
//!    It keeps returning `Direct(SocketAddr)` for seconds after the peer
//!    tears its endpoint down. Using it as an "offline" signal misses the
//!    Phase 1 budget (≤ 10 s) by a wide margin.
//! 2. [`iroh::endpoint::Connection::closed`] resolves within ~100 ms of the
//!    peer disappearing on loopback. This is the reliable offline signal.
//!
//! The adapter therefore:
//!
//! * Holds every successfully-dialed [`Connection`] alive inside a
//!   [`TrackedPeer`] entry keyed by [`DeviceId`].
//! * Spawns a **watchdog task per tracked peer** that awaits
//!   `connection.closed()` and, on completion, removes the entry and
//!   broadcasts a `PeerReachabilityChanged { state: Offline, .. }`.
//! * Exposes a second "last observed state" map so `current_state` can
//!   return `Offline` for a peer whose dial failed (that peer is *not* in
//!   the tracked map). `current_state` therefore reads from the last-state
//!   cache first, falling back to the tracked-connection map, and only
//!   yielding `Unknown` when neither knows anything.
//!
//! ## ALPN
//!
//! [`PRESENCE_ALPN`] = `uniclipboard/presence/1`. The accept side runs
//! [`IrohPresenceHandler`], which holds each incoming connection open until
//! the peer closes it after confirming current-space admission.
//! The dial side is invoked from [`IrohPresenceAdapter::ensure_reachable`].
//!
//! ## Inbound-driven Online flip
//!
//! Holding the connection open is necessary but not sufficient: a peer that
//! recovers needs us to mark *it* Online without waiting for our next own
//! dial. The handler therefore reverse-resolves `Connection::remote_id()`
//! into a `DeviceId` (same `IdentityFingerprintFactoryPort` + `MemberRepo`
//! lookup the clipboard receiver uses) and, on a Offline → Online
//! transition, writes `last_state[device]=Online` and broadcasts a single
//! `Online` event. Repeat inbound dials from the same peer (every keepalive
//! tick) are idempotent — they don't re-broadcast.
//!
//! 入站与出站共同观察最后一条有效连接的关闭。确认写入尚未完成的入站连接
//! 单独受撤销管理，不作为在线证据；等待网络期间不持有共享状态锁。
//! 每台设备的在途尝试分别受撤销约束，完整停用空间才让全部尝试失效。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh::Endpoint;
#[cfg(test)]
use iroh::EndpointAddr;
use tokio::sync::{broadcast, Mutex};
use tokio::task::JoinHandle;
use tracing::{debug, info, instrument, warn};

use uc_core::ids::DeviceId;
use uc_core::membership::{MemberRepositoryPort, PeerAdmissionPort};
use uc_core::ports::security::IdentityFingerprintFactoryPort;
use uc_core::ports::{
    ClockPort, PeerAddressRepositoryPort, PeerReachabilityChanged, PeerReachabilityPort,
    PresenceError, ReachabilityState,
};
use uc_core::security::IdentityFingerprint;
use uc_observability_contract::diagnostics::connectivity::{
    ConfirmationFailure, DialFailure, PresenceCheckObservation, PresenceCheckResult,
};

use super::connect::connect_with_staggered_retry_classified;
use super::net_recovery::{
    DemandRecoveryCoordinator, NetworkRecoveryObservation, NetworkRecoveryObservationSource,
};
use super::peer_address_resolver::PeerAddressResolver;

/// ALPN identifier for the Slice 2 presence protocol. The accept-side
/// handler confirms current-space admission before the dial side publishes
/// Online, then keeps the connection open so the watchdog can observe peer
/// teardown via [`Connection::closed`].
pub const PRESENCE_ALPN: &[u8] = b"uniclipboard/presence/1";

/// Capacity of the [`broadcast`] channel that fans `PeerReachabilityChanged`s out to
/// subscribers. 64 sits comfortably above expected burst width (N ≤ 10
/// members flipping state on an unlock); lagging subscribers recover via
/// [`PresencePort::current_state`] per the broadcast contract.
const EVENT_CHANNEL_CAPACITY: usize = 64;
const RECOVERY_CONFIRMATION_BUDGET: Duration = Duration::from_secs(2);
const PRESENCE_ADMISSION_IO_TIMEOUT: Duration = Duration::from_secs(5);
const ADMISSION_CONFIRMATION_REQUEST: u8 = 1;
const ADMISSION_ACCEPTED: u8 = 1;
const ADMISSION_REJECTED: u8 = 2;

/// 已有连接的快速复用证据最多保留 30 秒；超过后重新确认可达性。
/// 无关闭帧的进程崩溃或网络黑洞可能晚于实际断线才被传输层发现，
/// 因此发送前不能无限依赖旧的在线观察。自动连接与重试周期由 Application 负责。
const FAST_PATH_TTL: Duration = Duration::from_secs(30);

/// 实际发送确认不可达后，短暂优先返回 Offline，统一查询和发送预检的判断。
/// 新的认证连接成功会清除此观察；该有效期不承担重试调度，也不阻止主动确认恢复。
const MARK_OFFLINE_STICKY_TTL: Duration = Duration::from_secs(30);

// ============================================================================
// ProtocolHandler (accept side)
// ============================================================================

/// Shared state between the dial-side adapter and the accept-side handler.
///
/// Both sides write `last_state` and emit `event_tx` events; sharing a
/// single `Arc<Mutex<HashMap>>` for `last_state` is what makes the inbound
/// Online flip race-safe with the outbound watchdog's Offline write — every
/// state mutation goes through the same lock.
#[derive(Default)]
struct ConnectionObservations {
    generation: u64,
    next_success: u64,
    successes: HashMap<DeviceId, u64>,
    // 只保留在途尝试的弱引用；移除设备无需留下永久的撤销墓碑。
    peer_epochs: HashMap<DeviceId, Weak<AttemptEpoch>>,
    pending_inbound: HashMap<usize, (DeviceId, Connection)>,
}

#[derive(Default)]
struct AttemptEpoch {
    // 保留整段重叠窗口的事实，核验刚结束也不能丢失随后的新成功通知。
    has_verification: AtomicBool,
}

#[derive(Clone)]
struct AttemptObservation {
    generation: u64,
    peer_epoch: Arc<AttemptEpoch>,
    success: Option<u64>,
}

impl ConnectionObservations {
    fn begin(&mut self, device: DeviceId) -> AttemptObservation {
        self.peer_epochs.retain(|_, epoch| epoch.strong_count() > 0);
        let peer_epoch = self
            .peer_epochs
            .get(&device)
            .and_then(Weak::upgrade)
            .unwrap_or_else(|| {
                let epoch = Arc::new(AttemptEpoch::default());
                self.peer_epochs.insert(device, Arc::downgrade(&epoch));
                epoch
            });
        AttemptObservation {
            generation: self.generation,
            peer_epoch,
            success: self.successes.get(&device).copied(),
        }
    }

    fn begin_verification(&mut self, device: DeviceId) -> AttemptObservation {
        let attempt = self.begin(device);
        attempt
            .peer_epoch
            .has_verification
            .store(true, Ordering::Release);
        attempt
    }

    fn is_current(&self, device: DeviceId, attempt: &AttemptObservation) -> bool {
        self.generation == attempt.generation
            && self
                .peer_epochs
                .get(&device)
                .and_then(Weak::upgrade)
                .is_some_and(|epoch| Arc::ptr_eq(&epoch, &attempt.peer_epoch))
    }

    fn succeeded(&mut self, device: DeviceId) {
        self.next_success = self.next_success.wrapping_add(1);
        self.successes.insert(device, self.next_success);
    }
}

struct HandlerState {
    observations: Arc<Mutex<ConnectionObservations>>,
    peers: Arc<Mutex<HashMap<DeviceId, TrackedPeer>>>,
    member_repo: Arc<dyn MemberRepositoryPort>,
    peer_admission: Arc<dyn PeerAdmissionPort>,
    fingerprint_factory: Arc<dyn IdentityFingerprintFactoryPort>,
    last_state: Arc<Mutex<HashMap<DeviceId, ReachabilityState>>>,
    /// Shared with the dial-side adapter so an inbound presence ping from
    /// a peer that was previously stamped Offline by `mark_offline` clears
    /// the negative window in the same map the adapter reads from in
    /// `current_state`. See [`MARK_OFFLINE_STICKY_TTL`].
    last_offline_at: Arc<Mutex<HashMap<DeviceId, Instant>>>,
    inbound_connections: Arc<Mutex<HashMap<usize, (DeviceId, Connection)>>>,
    accepting: AtomicBool,
    event_tx: broadcast::Sender<PeerReachabilityChanged>,
    clock: Arc<dyn ClockPort>,
}

impl HandlerState {
    async fn mark_offline_if_disconnected(&self, device: DeviceId) {
        if !self.accepting.load(Ordering::Acquire) {
            return;
        }
        if self
            .peers
            .lock()
            .await
            .get(&device)
            .is_some_and(|peer| peer.connection.close_reason().is_none())
        {
            return;
        }
        if self
            .inbound_connections
            .lock()
            .await
            .values()
            .any(|(id, connection)| *id == device && connection.close_reason().is_none())
        {
            return;
        }
        let mut states = self.last_state.lock().await;
        if states.get(&device) != Some(&ReachabilityState::Online) {
            return;
        }
        states.insert(device, ReachabilityState::Offline);
        let _ = self.event_tx.send(PeerReachabilityChanged {
            device_id: device,
            state: ReachabilityState::Offline,
            at: self.now(),
        });
    }

    /// Resolve `remote_pubkey_bytes` (iroh `EndpointId` 32-byte public key)
    /// back to a `SpaceMember.device_id` via the same fingerprint factory
    /// the receiver adapter uses. `None` means "unknown peer" — handler
    /// holds the connection open but does not mutate presence state.
    ///
    /// `member_repo.list()` is acceptable per the Slice 2 N ≤ 10 roster
    /// assumption (see `clipboard_receiver_adapter.rs` for the same
    /// rationale). A dedicated lookup-by-fingerprint index is a Phase 3
    /// concern.
    async fn resolve_device(&self, remote_pubkey_bytes: &[u8; 32]) -> Option<DeviceId> {
        let derived = match self
            .fingerprint_factory
            .from_public_key(remote_pubkey_bytes)
        {
            Ok(fp) => fp,
            Err(_error) => {
                warn!(
                    error.type = "unavailable",
                    "presence accept: fingerprint derivation failed — cannot resolve peer",
                );
                return None;
            }
        };

        let members = match self.member_repo.list().await {
            Ok(ms) => ms,
            Err(_error) => {
                warn!(
                    error.type = "storage",
                    "presence accept: member_repo.list failed; treating peer as unknown",
                );
                return None;
            }
        };

        members
            .into_iter()
            .find(|m| fingerprints_equal(&m.identity_fingerprint, &derived))
            .map(|m| m.device_id)
    }

    async fn is_admitted(&self, device_id: &DeviceId) -> bool {
        match self.peer_admission.is_admitted(device_id).await {
            Ok(admitted) => admitted,
            Err(_error) => {
                warn!(error.type = "unavailable", "presence accept: peer admission check failed");
                false
            }
        }
    }

    fn now(&self) -> DateTime<Utc> {
        let ms = self.clock.now_ms();
        Utc.timestamp_millis_opt(ms).single().unwrap_or_else(|| {
            warn!(
                ms,
                "ClockPort returned out-of-range epoch millis; falling back to Utc::now",
            );
            Utc::now()
        })
    }
}

/// Accept-side handler for [`PRESENCE_ALPN`].
///
/// Holds each inbound connection open until the peer closes it. Beyond
/// holding (the original liveness contract), it also reverse-resolves the
/// remote endpoint id to a `DeviceId` and flips
/// `last_state[device] = Online` on the first inbound dial that wasn't
/// already Online — the recovery path that makes peer-keepalive backoff
/// safe to extend.
#[derive(Clone)]
pub struct IrohPresenceHandler {
    state: Arc<HandlerState>,
}

impl std::fmt::Debug for IrohPresenceHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IrohPresenceHandler")
            .finish_non_exhaustive()
    }
}

impl ProtocolHandler for IrohPresenceHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let generation = self.state.observations.lock().await.generation;
        let remote = connection.remote_id();
        let connection_id = connection.stable_id();
        debug!("presence connection accepted; holding open until peer closes");

        let (mut send, mut receive) =
            match tokio::time::timeout(PRESENCE_ADMISSION_IO_TIMEOUT, connection.accept_bi()).await
            {
                Ok(Ok(streams)) => streams,
                _ => {
                    connection.close(0u32.into(), b"admission_confirmation_missing");
                    return Ok(());
                }
            };
        let mut request = [0u8; 1];
        if !matches!(
            tokio::time::timeout(
                PRESENCE_ADMISSION_IO_TIMEOUT,
                receive.read_exact(&mut request)
            )
            .await,
            Ok(Ok(_))
        ) || request[0] != ADMISSION_CONFIRMATION_REQUEST
        {
            connection.close(0u32.into(), b"admission_confirmation_invalid");
            return Ok(());
        }

        let remote_bytes: [u8; 32] = *remote.as_bytes();
        let admitted_device = self.state.resolve_device(&remote_bytes).await;
        if let Some(device_id) = admitted_device {
            let before = self.state.observations.lock().await.begin(device_id);
            if !self.state.is_admitted(&device_id).await {
                warn!(error.type = "peer_rejected", "presence accept: peer is not admitted by current space protection");
                let _ = send.write_all(&[ADMISSION_REJECTED]).await;
                let _ = send.finish();
                let _ = connection.closed().await;
                return Ok(());
            }
            {
                let mut observation = self.state.observations.lock().await;
                if observation.generation != generation
                    || !observation.is_current(device_id, &before)
                    || !self.state.accepting.load(Ordering::Acquire)
                {
                    drop(observation);
                    let _ = send.write_all(&[ADMISSION_REJECTED]).await;
                    let _ = send.finish();
                    let _ = connection.closed().await;
                    return Ok(());
                }
                // 等待确认期间也纳入撤销管理，但不能作为已在线的证据。
                observation
                    .pending_inbound
                    .insert(connection_id, (device_id, connection.clone()));
            }
            // 网络等待不占用任何设备共用的状态锁。
            let confirmed = matches!(
                tokio::time::timeout(
                    PRESENCE_ADMISSION_IO_TIMEOUT,
                    send.write_all(&[ADMISSION_ACCEPTED])
                )
                .await,
                Ok(Ok(()))
            ) && send.finish().is_ok();
            let mut observation = self.state.observations.lock().await;
            observation.pending_inbound.remove(&connection_id);
            if !confirmed
                || !observation.is_current(device_id, &before)
                || !self.state.accepting.load(Ordering::Acquire)
            {
                connection.close(0u32.into(), b"admission_confirmation_failed");
                return Ok(());
            }
            self.state
                .inbound_connections
                .lock()
                .await
                .insert(connection_id, (device_id, connection.clone()));
            observation.succeeded(device_id);
            let now_at = self.state.now();

            // Acquire `last_state` only long enough to insert and observe
            // the previous value; broadcast and logging happen after the
            // lock drops to avoid holding it across `.send`.
            let prev = {
                let mut last = self.state.last_state.lock().await;
                last.insert(device_id, ReachabilityState::Online)
            };

            // 普通重复入站仍去重；与主动核验重叠的新成功必须交给上层结算。
            if prev != Some(ReachabilityState::Online)
                || before.peer_epoch.has_verification.load(Ordering::Acquire)
            {
                // Inbound presence ping is first-hand evidence the peer is
                // back online, so drop any sticky Offline window an
                // earlier `mark_offline` may have armed.
                {
                    let mut stamps = self.state.last_offline_at.lock().await;
                    stamps.remove(&device_id);
                }
                let _ = self.state.event_tx.send(PeerReachabilityChanged {
                    device_id,
                    state: ReachabilityState::Online,
                    at: now_at,
                });
                info!("inbound presence connection: peer marked Online",);
            } else {
                debug!("inbound presence connection: peer already Online (no event)",);
            }
        } else {
            // A peer that is no longer in the local space must not keep a
            // successful presence connection after leave or switch-space.
            let _ = send.write_all(&[ADMISSION_REJECTED]).await;
            let _ = send.finish();
            let _ = connection.closed().await;
            debug!("inbound presence connection from unresolved peer; closing",);
            return Ok(());
        }

        connection.closed().await;
        let _observation = self.state.observations.lock().await;
        self.state
            .inbound_connections
            .lock()
            .await
            .remove(&connection_id);
        if let Some(device) = admitted_device {
            self.state.mark_offline_if_disconnected(device).await;
        }
        debug!("presence connection closed by peer",);
        Ok(())
    }
}

/// `IdentityFingerprint` comparison surface — kept as a free function so
/// future swaps to a normalised form land in one place.
fn fingerprints_equal(a: &IdentityFingerprint, b: &IdentityFingerprint) -> bool {
    a == b
}

// ============================================================================
// Adapter (dial side)
// ============================================================================

/// Iroh-backed [`PresencePort`] implementation.
pub struct IrohPresenceAdapter {
    observations: Arc<Mutex<ConnectionObservations>>,
    endpoint: Arc<Endpoint>,
    demand_recovery: Option<Arc<DemandRecoveryCoordinator>>,
    network_recovery_observations: Option<Arc<NetworkRecoveryObservationSource>>,
    peer_address_resolver: PeerAddressResolver,
    clock: Arc<dyn ClockPort>,
    /// Live iroh connections keyed by `DeviceId`. `DeviceId` is `Copy +
    /// Hash` (a 64-byte inline `ArrayString`), so it can be used directly
    /// as a map key without a stringified projection.
    peers: Arc<Mutex<HashMap<DeviceId, TrackedPeer>>>,
    /// Remember the last observed outcome for every device the adapter has
    /// ever probed. Distinct from `peers` because a failed dial should
    /// surface as `Offline` on `current_state` without leaving a live
    /// connection entry behind. Shared with [`HandlerState`] so inbound
    /// connections can flip a peer to Online under the same lock the
    /// outbound watchdog uses to flip to Offline.
    last_state: Arc<Mutex<HashMap<DeviceId, ReachabilityState>>>,
    /// Monotonic `Instant` of the most recent first-hand dial failure per
    /// device. `current_state` projects any stamp inside
    /// [`MARK_OFFLINE_STICKY_TTL`] as `Offline`. Cleared on successful outbound dials
    /// (`dial_and_track`'s `Ok` branch) and on inbound presence pings that
    /// flip the peer Online (see [`HandlerState`]).
    ///
    /// Owned here, cloned into [`HandlerState`] so the accept side can
    /// clear stamps under the same `Arc<Mutex>` the adapter reads from.
    last_offline_at: Arc<Mutex<HashMap<DeviceId, Instant>>>,
    event_tx: broadcast::Sender<PeerReachabilityChanged>,
    /// Cheap-clone state for [`IrohPresenceHandler`]. Constructed once in
    /// [`IrohPresenceAdapter::new`] and handed out via
    /// [`IrohPresenceAdapter::handler`].
    handler_state: Arc<HandlerState>,
}

/// Per-device bookkeeping: the live connection we hold open, plus the
/// watchdog task that awaits its demise.
struct TrackedPeer {
    connection: Connection,
    watchdog: JoinHandle<()>,
    /// Monotonic timestamp of the most recent confirmed-live observation
    /// for this entry. Set on insert in [`IrohPresenceAdapter::
    /// dial_and_track`] (dial just succeeded) and refreshed when a fresh
    /// dial races against an already-tracked entry and finds it still
    /// alive (the redial itself is fresh evidence). Consumed by
    /// [`IrohPresenceAdapter::ensure_reachable`]'s fast-path to refuse
    /// returning Online from an entry that has aged past
    /// [`FAST_PATH_TTL`], forcing a re-dial. See [`FAST_PATH_TTL`] for the
    /// silent-death problem this guards against.
    last_verified_at: Instant,
}

impl Drop for TrackedPeer {
    fn drop(&mut self) {
        // Dropping the connection is the caller's signal to close; aborting
        // the watchdog prevents it from racing on the now-dropped entry.
        self.watchdog.abort();
    }
}

impl IrohPresenceAdapter {
    /// Construct an adapter wired to the given iroh endpoint, peer address
    /// repository, member repository, fingerprint factory, and clock.
    /// Returns an owned value; the caller wraps it in `Arc` before
    /// publishing it as `Arc<dyn PresencePort>` so shutdown semantics match
    /// the rest of the iroh adapter family.
    ///
    /// `member_repo` and `fingerprint_factory` are needed by the inbound
    /// handler to reverse-resolve a remote `EndpointId` into a known
    /// `DeviceId`; the same pair is consumed by `IrohClipboardReceiverAdapter`.
    pub fn new(
        endpoint: Arc<Endpoint>,
        peer_addr_repo: Arc<dyn PeerAddressRepositoryPort>,
        member_repo: Arc<dyn MemberRepositoryPort>,
        peer_admission: Arc<dyn PeerAdmissionPort>,
        fingerprint_factory: Arc<dyn IdentityFingerprintFactoryPort>,
        clock: Arc<dyn ClockPort>,
    ) -> Self {
        Self::build(
            endpoint,
            peer_addr_repo,
            member_repo,
            peer_admission,
            fingerprint_factory,
            clock,
            None,
            None,
        )
    }

    pub(crate) fn new_with_recovery(
        endpoint: Arc<Endpoint>,
        peer_addr_repo: Arc<dyn PeerAddressRepositoryPort>,
        member_repo: Arc<dyn MemberRepositoryPort>,
        peer_admission: Arc<dyn PeerAdmissionPort>,
        fingerprint_factory: Arc<dyn IdentityFingerprintFactoryPort>,
        clock: Arc<dyn ClockPort>,
        demand_recovery: Arc<DemandRecoveryCoordinator>,
        network_recovery_observations: Arc<NetworkRecoveryObservationSource>,
    ) -> Self {
        Self::build(
            endpoint,
            peer_addr_repo,
            member_repo,
            peer_admission,
            fingerprint_factory,
            clock,
            Some(demand_recovery),
            Some(network_recovery_observations),
        )
    }

    fn build(
        endpoint: Arc<Endpoint>,
        peer_addr_repo: Arc<dyn PeerAddressRepositoryPort>,
        member_repo: Arc<dyn MemberRepositoryPort>,
        peer_admission: Arc<dyn PeerAdmissionPort>,
        fingerprint_factory: Arc<dyn IdentityFingerprintFactoryPort>,
        clock: Arc<dyn ClockPort>,
        demand_recovery: Option<Arc<DemandRecoveryCoordinator>>,
        network_recovery_observations: Option<Arc<NetworkRecoveryObservationSource>>,
    ) -> Self {
        let (event_tx, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        let last_state = Arc::new(Mutex::new(HashMap::new()));
        let last_offline_at = Arc::new(Mutex::new(HashMap::new()));
        let inbound_connections = Arc::new(Mutex::new(HashMap::new()));
        let observations = Arc::new(Mutex::new(ConnectionObservations::default()));
        let peers = Arc::new(Mutex::new(HashMap::new()));
        let handler_state = Arc::new(HandlerState {
            observations: Arc::clone(&observations),
            peers: Arc::clone(&peers),
            member_repo,
            peer_admission,
            fingerprint_factory,
            last_state: Arc::clone(&last_state),
            last_offline_at: Arc::clone(&last_offline_at),
            inbound_connections,
            accepting: AtomicBool::new(true),
            event_tx: event_tx.clone(),
            clock: Arc::clone(&clock),
        });
        Self {
            endpoint,
            demand_recovery,
            network_recovery_observations,
            peer_address_resolver: PeerAddressResolver::new(peer_addr_repo),
            clock,
            observations,
            peers,
            last_state,
            last_offline_at,
            event_tx,
            handler_state,
        }
    }

    /// Cheap clone-able handle registered with iroh's `RouterBuilder`. Each
    /// inbound connection runs [`IrohPresenceHandler::accept`], which
    /// shares this adapter's `last_state` map and broadcast `Sender` via
    /// `Arc<HandlerState>`.
    pub fn handler(&self) -> IrohPresenceHandler {
        IrohPresenceHandler {
            state: Arc::clone(&self.handler_state),
        }
    }

    fn now(&self) -> DateTime<Utc> {
        let ms = self.clock.now_ms();
        // `Utc.timestamp_millis_opt` rejects out-of-range values. Any
        // ClockPort implementation feeding out-of-range epoch millis is a
        // defect, but there is no recourse from this code path — fall back
        // to the current wall clock so presence timestamps stay monotonic
        // rather than panic the watchdog.
        match Utc.timestamp_millis_opt(ms).single() {
            Some(dt) => dt,
            None => {
                warn!(
                    ms,
                    "ClockPort returned out-of-range epoch millis; falling back to Utc::now"
                );
                Utc::now()
            }
        }
    }

    fn broadcast(&self, device_id: DeviceId, state: ReachabilityState, at: DateTime<Utc>) {
        // Ignoring `SendError` is intentional: a `broadcast::Sender::send`
        // failure just means no one is subscribed yet. Subscribers catch up
        // via `current_state` which is always in sync with `last_state`.
        let _ = self.event_tx.send(PeerReachabilityChanged {
            device_id,
            state,
            at,
        });
    }
}

impl IrohPresenceAdapter {
    /// 共享拨号路径：被 `ensure_reachable`（fast-path miss 后）和
    /// `verify_reachable`（强制路径）复用。负责：
    ///
    /// 1. 从 `peer_addr_repo` 读地址 → 解码 `EndpointAddr`
    /// 2. 通过 `connect_with_staggered_retry` 发起 iroh 拨号
    /// 3. 成功：spawn watchdog、写 `peers` map、broadcast `Online`
    ///    （若已存在 alive 条目则丢弃新拨连接，保留旧的——避免主动重连
    ///    扰动同步链路）
    /// 4. 失败：写 `last_state = Offline`、broadcast `Offline`
    ///    （**不**清理已存在的 stale 条目——业务路径下拨号可能因临时网络
    ///    抖动失败，旧连接其实还可用；`verify_reachable` 在外层补偿
    ///    "把假装活着的旧连接 close 掉"的清理动作）
    async fn dial_and_track(&self, device: &DeviceId) -> Result<ReachabilityState, PresenceError> {
        let observation = PresenceCheckObservation::begin();
        let mut failure = PresenceCheckResult::Interrupted;
        let result = self.dial_and_track_inner(device, &mut failure).await;
        observation.finish(if matches!(&result, Ok(ReachabilityState::Online)) {
            PresenceCheckResult::Reachable
        } else {
            failure
        });
        result
    }

    async fn dial_and_track_inner(
        &self,
        device: &DeviceId,
        failure: &mut PresenceCheckResult,
    ) -> Result<ReachabilityState, PresenceError> {
        let before = self.observations.lock().await.begin_verification(*device);
        // Look up the stored transport address.
        let endpoint_addr =
            match self
                .peer_address_resolver
                .resolve(device)
                .await
                .map_err(|error| {
                    *failure = PresenceCheckResult::AddressUnavailable;
                    PresenceError::internal(error)
                })? {
                Some(address) => address,
                None => {
                    *failure = PresenceCheckResult::AddressMissing;
                    debug!("dial_and_track: no address record; returning NoAddress");
                    return Err(PresenceError::NoAddress(*device));
                }
            };
        let was_online = self
            .last_state
            .lock()
            .await
            .get(device)
            .is_some_and(|state| *state == ReachabilityState::Online);

        if let Some(recovery) = &self.demand_recovery {
            recovery.recover_for_demand().await;
        }

        let recovery_confirmation = was_online
            && self
                .network_recovery_observations
                .as_ref()
                .is_some_and(|observations| observations.local_relay_recovered_recently());
        let dial = if recovery_confirmation {
            let deadline = Instant::now() + RECOVERY_CONFIRMATION_BUDGET;
            let first = match tokio::time::timeout(
                deadline.saturating_duration_since(Instant::now()),
                connect_with_staggered_retry_classified(
                    Arc::clone(&self.endpoint),
                    endpoint_addr.clone(),
                    PRESENCE_ALPN,
                    Vec::new(),
                    "network_recovery_confirmation",
                    uc_observability_contract::diagnostics::connectivity::AddressInputSource::Stored,
                ),
            )
            .await
            {
                Ok(result) => result,
                Err(_) => Err((
                    "network recovery confirmation timed out".to_string(),
                    DialFailure::TimedOut,
                )),
            };
            match first {
                Ok(connection) => Ok(connection),
                Err(_) => {
                    if let Some(recovery) = &self.demand_recovery {
                        recovery.recover_after_confirmed_path_failure().await;
                    }
                    match tokio::time::timeout(
                        deadline.saturating_duration_since(Instant::now()),
                        connect_with_staggered_retry_classified(
                            Arc::clone(&self.endpoint),
                            endpoint_addr,
                            PRESENCE_ALPN,
                            Vec::new(),
                            "network_recovery_confirmation",
                            uc_observability_contract::diagnostics::connectivity::AddressInputSource::Stored,
                        ),
                    )
                    .await
                    {
                        Ok(result) => result,
                        Err(_) => Err((
                            "network recovery confirmation timed out".to_string(),
                            DialFailure::TimedOut,
                        )),
                    }
                }
            }
        } else {
            connect_with_staggered_retry_classified(
                Arc::clone(&self.endpoint),
                endpoint_addr,
                PRESENCE_ALPN,
                Vec::new(),
                "presence",
                uc_observability_contract::diagnostics::connectivity::AddressInputSource::Stored,
            )
            .await
        };

        match dial {
            Ok(connection) => {
                let admission_confirmed = async {
                    let (mut send, mut receive) = connection.open_bi().await.map_err(|_| ())?;
                    send.write_all(&[ADMISSION_CONFIRMATION_REQUEST])
                        .await
                        .map_err(|_| ())?;
                    send.finish().map_err(|_| ())?;
                    let mut acknowledgement = [0u8; 1];
                    receive
                        .read_exact(&mut acknowledgement)
                        .await
                        .map_err(|_| ())?;
                    Ok::<bool, ()>(acknowledgement[0] == ADMISSION_ACCEPTED)
                };
                let confirmation =
                    tokio::time::timeout(PRESENCE_ADMISSION_IO_TIMEOUT, admission_confirmed).await;
                if !matches!(confirmation, Ok(Ok(true))) {
                    *failure = PresenceCheckResult::Confirmation(match confirmation {
                        Err(_) => ConfirmationFailure::TimedOut,
                        Ok(Err(())) => ConfirmationFailure::TransportFailed,
                        _ => ConfirmationFailure::PeerNotAdmitted,
                    });
                    connection.close(0u32.into(), b"peer_not_admitted");
                    return Ok(self.record_failed_dial(device, before).await);
                }
                let mut observation = self.observations.lock().await;
                if !observation.is_current(*device, &before)
                    || !self.handler_state.accepting.load(Ordering::Acquire)
                {
                    connection.close(0u32.into(), b"stale_attempt");
                    return Ok(ReachabilityState::Offline);
                }
                observation.succeeded(*device);
                let now = self.now();
                let watchdog =
                    spawn_watchdog(Arc::clone(&self.handler_state), *device, connection.clone());

                {
                    let mut peers = self.peers.lock().await;
                    // If an alive entry exists already (concurrent insert
                    // raced, or `verify_reachable` redialed against a
                    // tracked-but-stale-looking peer), abort our own
                    // watchdog and keep theirs — single connection slot
                    // per device. Refresh `last_verified_at` on the kept
                    // entry: our just-completed dial is fresh evidence
                    // that the peer is reachable *right now*, so the
                    // fast-path TTL clock should reset even though we're
                    // discarding the new connection in favour of the old.
                    if let Some(existing) = peers.get_mut(device) {
                        if existing.connection.close_reason().is_none() {
                            existing.last_verified_at = Instant::now();
                            debug!(
                                "dial_and_track: alive tracked entry exists; \
                                 discarding freshly dialed connection",
                            );
                            watchdog.abort();
                            drop(connection);

                            // 仍旧 broadcast Online — verify_reachable 调用方
                            // 期望"拨号成功 ⇒ Online 信号回传"。
                            let mut last = self.last_state.lock().await;
                            last.insert(*device, ReachabilityState::Online);
                            drop(last);
                            // Dial succeeded → cancel any sticky Offline
                            // window left over from an earlier
                            // `mark_offline` so consumers don't keep seeing
                            // the stale negative verdict.
                            {
                                let mut stamps = self.last_offline_at.lock().await;
                                stamps.remove(device);
                            }
                            self.broadcast(*device, ReachabilityState::Online, now);
                            if let Some(observations) = &self.network_recovery_observations {
                                observations
                                    .publish(NetworkRecoveryObservation::FreshPeerDialSucceeded);
                            }
                            return Ok(ReachabilityState::Online);
                        }
                    }
                    peers.insert(
                        *device,
                        TrackedPeer {
                            connection,
                            watchdog,
                            last_verified_at: Instant::now(),
                        },
                    );
                }

                {
                    let mut last = self.last_state.lock().await;
                    last.insert(*device, ReachabilityState::Online);
                }
                // Dial succeeded → cancel any sticky Offline window left
                // over from an earlier `mark_offline`.
                {
                    let mut stamps = self.last_offline_at.lock().await;
                    stamps.remove(device);
                }
                info!("dial_and_track: dial succeeded, peer marked Online");
                self.broadcast(*device, ReachabilityState::Online, now);
                if let Some(observations) = &self.network_recovery_observations {
                    observations.publish(NetworkRecoveryObservation::FreshPeerDialSucceeded);
                }
                Ok(ReachabilityState::Online)
            }
            Err((_error, category)) => {
                *failure = PresenceCheckResult::Dial(category);
                let state = self.record_failed_dial(device, before).await;
                if was_online && state == ReachabilityState::Offline {
                    if let Some(observations) = &self.network_recovery_observations {
                        observations
                            .publish(NetworkRecoveryObservation::PreviouslyOnlinePeerPathExhausted);
                    }
                }
                Ok(state)
            }
        }
    }
    async fn record_failed_dial(
        &self,
        device: &DeviceId,
        before: AttemptObservation,
    ) -> ReachabilityState {
        let observation = self.observations.lock().await;
        if !observation.is_current(*device, &before)
            || observation.successes.get(device).copied() != before.success
        {
            return self
                .last_state
                .lock()
                .await
                .get(device)
                .copied()
                .unwrap_or(ReachabilityState::Unknown);
        }
        if let Some(stale) = self.peers.lock().await.remove(device) {
            stale.connection.close(0u32.into(), b"unreachable");
        }
        let previous = self
            .last_state
            .lock()
            .await
            .insert(*device, ReachabilityState::Offline);
        self.last_offline_at
            .lock()
            .await
            .insert(*device, Instant::now());
        if previous != Some(ReachabilityState::Offline) {
            self.broadcast(*device, ReachabilityState::Offline, self.now());
        }
        ReachabilityState::Offline
    }
}

#[async_trait]
impl PeerReachabilityPort for IrohPresenceAdapter {
    #[instrument(skip_all)]
    async fn ensure_reachable(
        &self,
        device: &DeviceId,
    ) -> Result<ReachabilityState, PresenceError> {
        // Step 1: fast-path on an already-tracked live connection.
        //
        // Both predicates must hold to return Online without a fresh dial:
        //
        // * `close_reason().is_none()` — quinn has not (yet) observed the
        //   connection close. This is the original liveness signal from
        //   T3a but it lags silent-death scenarios by up to
        //   `max_idle_timeout = 60s`.
        //
        // * `last_verified_at.elapsed() < FAST_PATH_TTL` — we have *recent*
        //   first-hand evidence the peer is reachable (a successful dial
        //   landed inside the TTL window). Without this, an entry whose
        //   peer silently died can sit in the map looking alive for the
        //   full quinn idle window, lying "Online" to every caller.
        //
        // Either predicate failing routes through eviction + re-dial. The
        // miss is logged with both flags so a stale-TTL eviction is
        // distinguishable from a closed-conn eviction in production.
        {
            let mut peers = self.peers.lock().await;
            if let Some(entry) = peers.get(device) {
                let still_alive = entry.connection.close_reason().is_none();
                let recently_verified = entry.last_verified_at.elapsed() < FAST_PATH_TTL;
                if still_alive && recently_verified {
                    debug!("ensure_reachable: already tracked and alive");
                    return Ok(ReachabilityState::Online);
                }
                // Stale entry — either quinn has closed it, or the entry
                // has aged past FAST_PATH_TTL without re-verification.
                // Evict so the re-dial path below starts from a clean
                // slate (and so `dial_and_track`'s "alive entry already
                // exists" branch doesn't accidentally preserve a corpse).
                if let Some(stale) = peers.remove(device) {
                    stale.watchdog.abort();
                    debug!(
                        still_alive,
                        recently_verified,
                        "ensure_reachable: evicted stale tracked entry before re-dial",
                    );
                }
            }
        }

        // Step 2-4: dial via shared path.
        self.dial_and_track(device).await
    }

    #[instrument(skip_all)]
    async fn verify_reachable(
        &self,
        device: &DeviceId,
    ) -> Result<ReachabilityState, PresenceError> {
        // 跳过 fast-path —— 即便已有 alive 连接也强制重拨验证可达性。
        self.dial_and_track(device).await
    }

    #[instrument(skip_all)]
    async fn mark_offline(&self, device: &DeviceId) {
        let _observation = self.observations.lock().await;
        // 1) Evict the live-connection slot. The peer is held to be dead by
        //    an external observer — anything we cached as alive is now a lie
        //    that the fast-path in `ensure_reachable` would happily serve.
        //    Close the connection explicitly so the watchdog's
        //    `connection.closed().await` resolves and its cleanup runs
        //    (remove already noop, last_state already Offline below, the
        //    redundant Offline event is idempotent).
        //
        //    Order matches `verify_reachable`'s failure path: remove from
        //    map first, then close — avoids the watchdog cleanup half-killed
        //    race (TrackedPeer::drop will abort the watchdog if it hasn't
        //    fired yet, and that's fine; we don't depend on it running).
        let stale = {
            let mut peers = self.peers.lock().await;
            peers.remove(device)
        };
        if let Some(stale) = stale {
            stale.connection.close(0u32.into(), b"mark_offline");
            debug!("mark_offline: closed stale tracked connection");
        }

        // 2) Persist Offline in last_state. Skip the broadcast if the device
        //    was already Offline (idempotency contract). Hold the lock across
        //    the prev-vs-new compare-and-set so a racing inbound Online flip
        //    doesn't slip a duplicate Offline through.
        let should_broadcast = {
            let mut last = self.last_state.lock().await;
            let prev = last.insert(*device, ReachabilityState::Offline);
            prev != Some(ReachabilityState::Offline)
        };

        // 3) Stamp the negative window so `current_state` keeps reporting
        //    Offline for MARK_OFFLINE_STICKY_TTL even if some future code
        //    path resets `last_state[device]` back to Unknown. Today nothing
        //    on the dial side clears `last_state`; the stamp exists so #886
        //    can drop `DispatchClipboardEntryUseCase::recent_dial_failures`
        //    and let every consumer of `PresencePort::current_state` read
        //    the same truth window without each maintaining its own cache.
        //    Refresh unconditionally so repeated `mark_offline` calls
        //    re-arm the window from the latest verdict.
        {
            let mut stamps = self.last_offline_at.lock().await;
            stamps.insert(*device, Instant::now());
        }

        if should_broadcast {
            let now = self.now();
            debug!("mark_offline: peer marked Offline");
            self.broadcast(*device, ReachabilityState::Offline, now);
        }
    }

    async fn forget(&self, device: &DeviceId) {
        let mut observation = self.observations.lock().await;
        observation.peer_epochs.remove(device);
        observation.successes.remove(device);
        observation.pending_inbound.retain(|_, (id, connection)| {
            if id == device {
                connection.close(0u32.into(), b"forgotten");
                false
            } else {
                true
            }
        });
        self.handler_state
            .inbound_connections
            .lock()
            .await
            .retain(|_, (id, connection)| {
                if id == device {
                    connection.close(0u32.into(), b"forgotten");
                    false
                } else {
                    true
                }
            });
        let stale = self.peers.lock().await.remove(device);
        if let Some(stale) = stale {
            stale.connection.close(0u32.into(), b"forgotten");
        }
        self.last_state.lock().await.remove(device);
        self.last_offline_at.lock().await.remove(device);
    }

    async fn disconnect_all(&self) {
        let mut observation = self.observations.lock().await;
        observation.generation = observation.generation.wrapping_add(1);
        observation.successes.clear();
        observation.peer_epochs.clear();
        self.handler_state.accepting.store(false, Ordering::Release);
        for (_, (_, connection)) in observation.pending_inbound.drain() {
            connection.close(0u32.into(), b"space_left");
        }

        let outbound = {
            let mut peers = self.peers.lock().await;
            peers.drain().map(|(_, peer)| peer).collect::<Vec<_>>()
        };
        for peer in outbound {
            peer.connection.close(0u32.into(), b"space_left");
        }

        let inbound = {
            let mut connections = self.handler_state.inbound_connections.lock().await;
            connections
                .drain()
                .map(|(_, (_, connection))| connection)
                .collect::<Vec<_>>()
        };
        for connection in inbound {
            connection.close(0u32.into(), b"space_left");
        }

        self.last_state.lock().await.clear();
        self.last_offline_at.lock().await.clear();
    }

    async fn activate(&self) {
        self.handler_state.accepting.store(true, Ordering::Release);
    }

    // 故意不挂 `#[instrument]`:`current_state()` 仅做 in-memory map
    // lookup(`last_state` / `peers`),没有外部 I/O,但被 roster /
    // list_with_presence / ensure_reachable_all 在热路径上反复调用,
    // 14 天观测到 ~20 万次 span 落到 Sentry。`ensure_reachable` /
    // `verify_reachable` 真做拨号,继续保留 instrument(uc-infra §10.1
    // 强制要求关键 adapter 有 tracing)。
    async fn current_state(&self, device: &DeviceId) -> ReachabilityState {
        // Online remains authoritative until a watchdog or dial updates it.
        // Offline only short-circuits dispatch after a recent first-hand
        // dial failure. A watchdog close still emits Offline for roster
        // consumers, while the next clipboard change may dial again.
        if matches!(
            self.last_state.lock().await.get(device).copied(),
            Some(ReachabilityState::Online)
        ) {
            return ReachabilityState::Online;
        }
        // A recent dial failure keeps the existing storm-control window.
        {
            let stamps = self.last_offline_at.lock().await;
            if let Some(stamped_at) = stamps.get(device) {
                if stamped_at.elapsed() < MARK_OFFLINE_STICKY_TTL {
                    return ReachabilityState::Offline;
                }
            }
        }
        // Fall back to the tracked-connection map in case something
        // bypassed `last_state` bookkeeping. Under the current API surface
        // this branch is unreachable, but the check is cheap.
        let peers = self.peers.lock().await;
        match peers.get(device) {
            Some(entry) if entry.connection.close_reason().is_none() => ReachabilityState::Online,
            Some(_) => ReachabilityState::Unknown,
            None => ReachabilityState::Unknown,
        }
    }

    fn subscribe(&self) -> broadcast::Receiver<PeerReachabilityChanged> {
        self.event_tx.subscribe()
    }
}

// ============================================================================
// Watchdog
// ============================================================================

/// Spawn the per-peer watchdog task.
///
/// The task awaits `connection.closed()` — the reliable offline signal
/// established by T3a — then:
///
/// * Removes the `TrackedPeer` entry (which aborts the watchdog's own
///   `JoinHandle` via `Drop`, but since we're the watchdog itself at that
///   point the abort is a no-op).
/// * Writes `Offline` into the `last_state` cache.
/// * Broadcasts a `PeerReachabilityChanged { state: Offline, .. }`.
///
/// Errors on the broadcast send are ignored (no subscriber is a valid
/// state; consumers recover via `current_state`).
fn spawn_watchdog(
    state: Arc<HandlerState>,
    device_id: DeviceId,
    connection: Connection,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        connection.closed().await;
        let _observation = state.observations.lock().await;
        let retired = {
            let mut peers = state.peers.lock().await;
            if peers
                .get(&device_id)
                .is_some_and(|peer| peer.connection.stable_id() == connection.stable_id())
            {
                peers.remove(&device_id)
            } else {
                None
            }
        };
        state.mark_offline_if_disconnected(device_id).await;
        // 结算后才释放自身 JoinHandle，不能在后续 await 之前中止自己。
        drop(retired);
    })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap as StdHashMap;
    use std::sync::Mutex as StdMutex;
    use std::time::Duration;

    use chrono::Utc;
    use iroh::protocol::Router;
    use iroh::RelayMode;
    use tokio::time::timeout;

    use uc_core::ids::DeviceId;
    use uc_core::membership::{MemberRepositoryPort, MembershipError, SpaceMember};
    use uc_core::ports::{PeerAddressError, PeerAddressRecord};
    use uc_core::MemberSyncPreferences;

    use crate::security::Sha256IdentityFingerprintFactory;

    const DIAL_BUDGET: Duration = Duration::from_secs(5);
    const OFFLINE_BUDGET: Duration = Duration::from_secs(10);

    // -- Fakes ---------------------------------------------------------------

    #[derive(Default)]
    struct FakePeerAddressRepo {
        inner: StdMutex<StdHashMap<String, PeerAddressRecord>>,
        delay: StdMutex<Option<Arc<DelayedAdmission>>>,
    }

    impl FakePeerAddressRepo {
        fn seed(&self, record: PeerAddressRecord) {
            self.inner
                .lock()
                .unwrap()
                .insert(record.device_id.as_str().to_string(), record);
        }
    }

    #[async_trait]
    impl PeerAddressRepositoryPort for FakePeerAddressRepo {
        async fn get(
            &self,
            device: &DeviceId,
        ) -> Result<Option<PeerAddressRecord>, PeerAddressError> {
            let delay = self.delay.lock().unwrap().clone();
            if let Some(delay) = delay {
                delay.checking.notify_one();
                delay.proceed.acquire().await.unwrap().forget();
            }
            Ok(self.inner.lock().unwrap().get(device.as_str()).cloned())
        }

        async fn upsert(&self, record: &PeerAddressRecord) -> Result<(), PeerAddressError> {
            self.inner
                .lock()
                .unwrap()
                .insert(record.device_id.as_str().to_string(), record.clone());
            Ok(())
        }

        async fn list(&self) -> Result<Vec<PeerAddressRecord>, PeerAddressError> {
            Ok(self.inner.lock().unwrap().values().cloned().collect())
        }

        async fn remove(&self, device: &DeviceId) -> Result<(), PeerAddressError> {
            self.inner.lock().unwrap().remove(device.as_str());
            Ok(())
        }
    }

    /// In-memory `MemberRepositoryPort` for handler-side identity
    /// resolution. Tests that exercise dial-only paths leave it empty;
    /// tests that exercise inbound `Online` flips seed it with a member
    /// whose `identity_fingerphint` matches the dialing endpoint's pubkey.
    #[derive(Default)]
    struct MemMemberRepo {
        inner: StdMutex<StdHashMap<String, SpaceMember>>,
    }

    impl MemMemberRepo {
        fn seed(&self, member: SpaceMember) {
            self.inner
                .lock()
                .unwrap()
                .insert(member.device_id.as_str().to_string(), member);
        }
    }

    #[async_trait]
    impl MemberRepositoryPort for MemMemberRepo {
        async fn get(&self, device: &DeviceId) -> Result<Option<SpaceMember>, MembershipError> {
            Ok(self.inner.lock().unwrap().get(device.as_str()).cloned())
        }
        async fn list(&self) -> Result<Vec<SpaceMember>, MembershipError> {
            Ok(self.inner.lock().unwrap().values().cloned().collect())
        }
        async fn save(&self, member: &SpaceMember) -> Result<(), MembershipError> {
            self.inner
                .lock()
                .unwrap()
                .insert(member.device_id.as_str().to_string(), member.clone());
            Ok(())
        }
        async fn remove(&self, device_id: &DeviceId) -> Result<bool, MembershipError> {
            Ok(self
                .inner
                .lock()
                .unwrap()
                .remove(device_id.as_str())
                .is_some())
        }
    }

    struct FixedClock;
    impl ClockPort for FixedClock {
        fn now_ms(&self) -> i64 {
            // 2026-01-01T00:00:00Z — chosen so `at` is always the same in
            // every test for easy assertions.
            1_767_225_600_000
        }
    }

    // -- Helpers -------------------------------------------------------------

    async fn bound_endpoint() -> Arc<Endpoint> {
        // Discovery is cleared so dials must rely solely on the
        // `EndpointAddr` blob: an empty `transport_addrs` with relays
        // disabled then has no fallback, which is what
        // `ensure_reachable_after_offline_redials_successfully` relies on.
        // Without this, iroh's default n0/pkarr DNS discovery can resolve
        // the live peer's id back to its real direct addrs and the dial
        // unexpectedly succeeds on environments with outbound DNS (CI).
        Arc::new(
            Endpoint::builder(iroh::endpoint::presets::N0)
                .alpns(vec![PRESENCE_ALPN.to_vec()])
                .relay_mode(RelayMode::Disabled)
                .clear_address_lookup()
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

    /// Build endpoint A (dialer), endpoint B (acceptor) with a spawned
    /// `Router` registering [`IrohPresenceHandler`] on [`PRESENCE_ALPN`].
    /// Returns both endpoints, B's encoded blob for the repo, B's
    /// `DeviceId`, and B's `Router` so the test can shut it down later.
    ///
    /// The B-side adapter is a decoy used solely to produce a working
    /// inbound handler — its `last_state` map is not observed by the
    /// dial-side tests below. Tests that exercise the inbound Online flip
    /// build their own adapter explicitly via
    /// [`build_adapter_with_member_repo`].
    async fn setup_two_endpoints() -> (Arc<Endpoint>, Arc<Endpoint>, Vec<u8>, DeviceId, Router) {
        let endpoint_b = bound_endpoint().await;
        wait_for_direct_addrs(&endpoint_b).await;
        let b_addr = endpoint_b.addr();
        let b_blob = postcard::to_stdvec(&b_addr).expect("postcard encode EndpointAddr");
        let b_device_id = DeviceId::new(format!("endpoint-b-{}", endpoint_b.id().fmt_short()));

        let endpoint_a = bound_endpoint().await;
        wait_for_direct_addrs(&endpoint_a).await;
        let member_repo = Arc::new(MemMemberRepo::default());
        member_repo.seed(member_for_endpoint(&endpoint_a, "endpoint-a"));
        let decoy_adapter = IrohPresenceAdapter::new(
            Arc::clone(&endpoint_b),
            Arc::new(FakePeerAddressRepo::default()),
            member_repo,
            Arc::new(crate::network::iroh::StaticPeerAdmission(true)),
            Arc::new(Sha256IdentityFingerprintFactory),
            Arc::new(FixedClock),
        );
        let router_b = Router::builder((*endpoint_b).clone())
            .accept(PRESENCE_ALPN, decoy_adapter.handler())
            .spawn();

        (endpoint_a, endpoint_b, b_blob, b_device_id, router_b)
    }

    fn record(device: &DeviceId, blob: Vec<u8>) -> PeerAddressRecord {
        PeerAddressRecord {
            device_id: device.clone(),
            addr_blob: blob,
            observed_at: Utc::now(),
        }
    }

    async fn request_admission_confirmation(connection: &Connection) -> u8 {
        let (mut send, mut receive) = connection.open_bi().await.expect("open admission stream");
        send.write_all(&[ADMISSION_CONFIRMATION_REQUEST])
            .await
            .expect("write admission request");
        send.finish().expect("finish admission request");
        let mut acknowledgement = [0u8; 1];
        receive
            .read_exact(&mut acknowledgement)
            .await
            .expect("read admission confirmation");
        if acknowledgement[0] != ADMISSION_ACCEPTED {
            connection.close(0u32.into(), b"peer_not_admitted");
        }
        acknowledgement[0]
    }

    /// Build an adapter for the dial-side tests. Inbound resolution is
    /// not exercised here — the empty `MemMemberRepo` is enough to keep
    /// the handler constructible.
    fn build_adapter(
        endpoint: Arc<Endpoint>,
        repo: Arc<dyn PeerAddressRepositoryPort>,
    ) -> IrohPresenceAdapter {
        IrohPresenceAdapter::new(
            endpoint,
            repo,
            Arc::new(MemMemberRepo::default()),
            Arc::new(crate::network::iroh::StaticPeerAdmission(true)),
            Arc::new(Sha256IdentityFingerprintFactory),
            Arc::new(FixedClock),
        )
    }

    /// Build an adapter wired to a caller-supplied `MemberRepositoryPort`
    /// so inbound-flip tests can seed the dialing endpoint's identity.
    fn build_adapter_with_member_repo(
        endpoint: Arc<Endpoint>,
        repo: Arc<dyn PeerAddressRepositoryPort>,
        member_repo: Arc<dyn MemberRepositoryPort>,
    ) -> IrohPresenceAdapter {
        build_adapter_with_member_repo_and_admission(endpoint, repo, member_repo, true)
    }

    fn build_adapter_with_member_repo_and_admission(
        endpoint: Arc<Endpoint>,
        repo: Arc<dyn PeerAddressRepositoryPort>,
        member_repo: Arc<dyn MemberRepositoryPort>,
        admitted: bool,
    ) -> IrohPresenceAdapter {
        IrohPresenceAdapter::new(
            endpoint,
            repo,
            member_repo,
            Arc::new(crate::network::iroh::StaticPeerAdmission(admitted)),
            Arc::new(Sha256IdentityFingerprintFactory),
            Arc::new(FixedClock),
        )
    }

    // -- Tests ---------------------------------------------------------------

    #[tokio::test]
    async fn forgetting_device_clears_cached_state() {
        let endpoint = bound_endpoint().await;
        let adapter = build_adapter(endpoint.clone(), Arc::new(FakePeerAddressRepo::default()));
        let device = DeviceId::new("old-space-device");

        adapter.mark_offline(&device).await;
        assert_eq!(
            adapter.current_state(&device).await,
            ReachabilityState::Offline
        );

        adapter.forget(&device).await;

        assert_eq!(
            adapter.current_state(&device).await,
            ReachabilityState::Unknown
        );
        endpoint.close().await;
    }

    #[tokio::test]
    async fn disconnecting_all_closes_connections_held_for_the_old_space() {
        let endpoint_a = bound_endpoint().await;
        wait_for_direct_addrs(&endpoint_a).await;
        let endpoint_b = bound_endpoint().await;
        wait_for_direct_addrs(&endpoint_b).await;

        let a_member = member_for_endpoint(&endpoint_a, "device-a");
        let b_member_repo = Arc::new(MemMemberRepo::default());
        b_member_repo.seed(a_member);
        let b_adapter = build_adapter_with_member_repo(
            Arc::clone(&endpoint_b),
            Arc::new(FakePeerAddressRepo::default()),
            b_member_repo,
        );
        let router_b = Router::builder((*endpoint_b).clone())
            .accept(PRESENCE_ALPN, b_adapter.handler())
            .spawn();

        let b_device_id = DeviceId::new("device-b");
        let a_repo = Arc::new(FakePeerAddressRepo::default());
        let b_blob = postcard::to_stdvec(&endpoint_b.addr()).expect("encode B address");
        a_repo.seed(record(&b_device_id, b_blob));
        let a_adapter = build_adapter(Arc::clone(&endpoint_a), a_repo);
        let mut a_events = a_adapter.subscribe();

        assert_eq!(
            a_adapter
                .ensure_reachable(&b_device_id)
                .await
                .expect("initial dial succeeds"),
            ReachabilityState::Online
        );
        let online = timeout(Duration::from_secs(1), a_events.recv())
            .await
            .expect("online event arrives")
            .expect("event channel open");
        assert_eq!(online.state, ReachabilityState::Online);

        b_adapter.disconnect_all().await;

        let offline = timeout(Duration::from_secs(1), a_events.recv())
            .await
            .expect("leaving peer closes the old-space presence connection")
            .expect("event channel open");
        assert_eq!(offline.state, ReachabilityState::Offline);
        assert_eq!(offline.device_id, b_device_id);

        router_b.shutdown().await.ok();
        endpoint_a.close().await;
    }

    struct DelayedAdmission {
        checking: tokio::sync::Notify,
        proceed: tokio::sync::Semaphore,
    }
    #[async_trait]
    impl PeerAdmissionPort for DelayedAdmission {
        async fn is_admitted(
            &self,
            _device: &DeviceId,
        ) -> Result<bool, uc_core::membership::PeerAdmissionError> {
            self.checking.notify_one();
            self.proceed.acquire().await.unwrap().forget();
            Ok(true)
        }
    }

    #[tokio::test]
    async fn stalled_confirmation_does_not_block_other_peers_or_shutdown() {
        for close_all in [false, true] {
            let blocked = Arc::new(
                Endpoint::builder(iroh::endpoint::presets::N0)
                    .alpns(vec![PRESENCE_ALPN.to_vec()])
                    .relay_mode(RelayMode::Disabled)
                    .clear_address_lookup()
                    .transport_config(
                        iroh::endpoint::QuicTransportConfig::builder()
                            .stream_receive_window(0u32.into())
                            .build(),
                    )
                    .bind()
                    .await
                    .unwrap(),
            );
            let healthy = bound_endpoint().await;
            let server = bound_endpoint().await;
            wait_for_direct_addrs(&server).await;
            let members = Arc::new(MemMemberRepo::default());
            members.seed(member_for_endpoint(&blocked, "blocked"));
            members.seed(member_for_endpoint(&healthy, "healthy"));
            let gate = Arc::new(DelayedAdmission {
                checking: tokio::sync::Notify::new(),
                proceed: tokio::sync::Semaphore::new(2),
            });
            let adapter = IrohPresenceAdapter::new(
                server.clone(),
                Arc::new(FakePeerAddressRepo::default()),
                members,
                gate.clone(),
                Arc::new(Sha256IdentityFingerprintFactory),
                Arc::new(FixedClock),
            );
            let router = Router::builder((*server).clone())
                .accept(PRESENCE_ALPN, adapter.handler())
                .spawn();
            let stalled = blocked.connect(server.addr(), PRESENCE_ALPN).await.unwrap();
            let (mut send, _receive) = stalled.open_bi().await.unwrap();
            send.write_all(&[ADMISSION_CONFIRMATION_REQUEST])
                .await
                .unwrap();
            send.finish().unwrap();
            timeout(Duration::from_secs(1), gate.checking.notified())
                .await
                .unwrap();
            assert_eq!(
                adapter.current_state(&DeviceId::new("blocked")).await,
                ReachabilityState::Unknown,
                "an unfinished confirmation is not online evidence"
            );
            let normal = healthy.connect(server.addr(), PRESENCE_ALPN).await.unwrap();
            assert_eq!(
                timeout(
                    Duration::from_millis(500),
                    request_admission_confirmation(&normal)
                )
                .await
                .expect("one stalled confirmation must not block another device"),
                ADMISSION_ACCEPTED
            );
            if close_all {
                timeout(Duration::from_millis(500), adapter.disconnect_all())
                    .await
                    .expect("shutdown must not wait for the peer's receive credit");
            } else {
                timeout(
                    Duration::from_millis(500),
                    adapter.forget(&DeviceId::new("blocked")),
                )
                .await
                .expect("revocation must not wait for the peer's receive credit");
                assert_eq!(
                    adapter.current_state(&DeviceId::new("healthy")).await,
                    ReachabilityState::Online
                );
            }
            timeout(Duration::from_millis(500), stalled.closed())
                .await
                .unwrap();
            timeout(Duration::from_millis(500), adapter.disconnect_all())
                .await
                .unwrap();
            router.shutdown().await.unwrap();
            blocked.close().await;
            healthy.close().await;
        }
    }

    #[tokio::test]
    async fn forgetting_one_peer_preserves_another_in_flight_connection() {
        let a = bound_endpoint().await;
        let c = bound_endpoint().await;
        wait_for_direct_addrs(&c).await;
        let gate = Arc::new(DelayedAdmission {
            checking: tokio::sync::Notify::new(),
            proceed: tokio::sync::Semaphore::new(0),
        });
        let members = Arc::new(MemMemberRepo::default());
        members.seed(member_for_endpoint(&a, "a"));
        let c_adapter = IrohPresenceAdapter::new(
            c.clone(),
            Arc::new(FakePeerAddressRepo::default()),
            members,
            gate.clone(),
            Arc::new(Sha256IdentityFingerprintFactory),
            Arc::new(FixedClock),
        );
        let router = Router::builder((*c).clone())
            .accept(PRESENCE_ALPN, c_adapter.handler())
            .spawn();
        let addresses = Arc::new(FakePeerAddressRepo::default());
        let c_id = DeviceId::new("c");
        addresses.seed(record(&c_id, postcard::to_stdvec(&c.addr()).unwrap()));
        let adapter = Arc::new(build_adapter(a.clone(), addresses));
        adapter.mark_offline(&DeviceId::new("b")).await;
        let dialing = tokio::spawn({
            let adapter = adapter.clone();
            async move { adapter.verify_reachable(&c_id).await }
        });
        timeout(Duration::from_secs(1), gate.checking.notified())
            .await
            .unwrap();
        adapter.forget(&DeviceId::new("b")).await;
        gate.proceed.add_permits(1);
        assert_eq!(
            timeout(DIAL_BUDGET, dialing)
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
            ReachabilityState::Online,
            "removing B must not invalidate C's successful connection"
        );
        adapter.disconnect_all().await;
        router.shutdown().await.unwrap();
        a.close().await;
    }

    #[tokio::test]
    async fn forgotten_peer_cannot_complete_an_old_inbound_admission() {
        delayed_inbound_revocation_case(DeviceId::new("a"), ADMISSION_REJECTED).await;
    }

    #[tokio::test]
    async fn forgetting_other_peer_preserves_inbound_admission() {
        delayed_inbound_revocation_case(DeviceId::new("other"), ADMISSION_ACCEPTED).await;
    }

    async fn delayed_inbound_revocation_case(removed: DeviceId, expected: u8) {
        let a = bound_endpoint().await;
        let b = bound_endpoint().await;
        wait_for_direct_addrs(&a).await;
        wait_for_direct_addrs(&b).await;
        let members = Arc::new(MemMemberRepo::default());
        members.seed(member_for_endpoint(&a, "a"));
        let gate = Arc::new(DelayedAdmission {
            checking: tokio::sync::Notify::new(),
            proceed: tokio::sync::Semaphore::new(0),
        });
        let adapter = IrohPresenceAdapter::new(
            b.clone(),
            Arc::new(FakePeerAddressRepo::default()),
            members,
            gate.clone(),
            Arc::new(Sha256IdentityFingerprintFactory),
            Arc::new(FixedClock),
        );
        let router = Router::builder((*b).clone())
            .accept(PRESENCE_ALPN, adapter.handler())
            .spawn();
        let connection = a.connect(b.addr(), PRESENCE_ALPN).await.unwrap();
        let confirmation = tokio::spawn({
            let connection = connection.clone();
            async move { request_admission_confirmation(&connection).await }
        });
        timeout(Duration::from_secs(1), gate.checking.notified())
            .await
            .unwrap();
        adapter.forget(&removed).await;
        gate.proceed.add_permits(1);
        assert_eq!(
            timeout(Duration::from_secs(1), confirmation)
                .await
                .unwrap()
                .unwrap(),
            expected
        );
        assert_eq!(
            adapter
                .last_state
                .lock()
                .await
                .get(&DeviceId::new("a"))
                .copied(),
            if expected == ADMISSION_ACCEPTED {
                Some(ReachabilityState::Online)
            } else {
                None
            }
        );
        connection.close(0u32.into(), b"test_finished");
        router.shutdown().await.unwrap();
        a.close().await;
    }

    #[tokio::test]
    async fn stale_failure_cannot_overwrite_new_inbound_success_or_revocation() {
        let a = bound_endpoint().await;
        let b = bound_endpoint().await;
        wait_for_direct_addrs(&a).await;
        wait_for_direct_addrs(&b).await;
        let members = Arc::new(MemMemberRepo::default());
        members.seed(member_for_endpoint(&a, "a"));
        let adapter = build_adapter_with_member_repo(
            b.clone(),
            Arc::new(FakePeerAddressRepo::default()),
            members,
        );
        let device = DeviceId::new("a");
        let before = adapter.observations.lock().await.begin(device);
        let router = Router::builder((*b).clone())
            .accept(PRESENCE_ALPN, adapter.handler())
            .spawn();
        let connection = a.connect(b.addr(), PRESENCE_ALPN).await.unwrap();
        assert_eq!(
            request_admission_confirmation(&connection).await,
            ADMISSION_ACCEPTED
        );
        // 将旧拨号失败的结算精确排在真实新入站成功之后。
        assert_eq!(
            adapter.record_failed_dial(&device, before.clone()).await,
            ReachabilityState::Online
        );
        assert_eq!(
            adapter.current_state(&device).await,
            ReachabilityState::Online
        );
        adapter.forget(&device).await;
        assert_eq!(
            adapter.record_failed_dial(&device, before).await,
            ReachabilityState::Unknown
        );
        assert!(adapter.last_state.lock().await.get(&device).is_none());
        timeout(Duration::from_secs(1), connection.closed())
            .await
            .unwrap();
        router.shutdown().await.unwrap();
        a.close().await;
    }

    #[tokio::test]
    async fn fresh_inbound_success_notifies_an_already_online_peer_with_a_pending_check() {
        let a = bound_endpoint().await;
        let b = bound_endpoint().await;
        wait_for_direct_addrs(&a).await;
        let members = Arc::new(MemMemberRepo::default());
        members.seed(member_for_endpoint(&b, "b"));
        let addresses = Arc::new(FakePeerAddressRepo::default());
        let adapter = Arc::new(build_adapter_with_member_repo(
            a.clone(),
            addresses.clone(),
            members,
        ));
        let router = Router::builder((*a).clone())
            .accept(PRESENCE_ALPN, adapter.handler())
            .spawn();
        let mut events = adapter.subscribe();
        let first = b.connect(a.addr(), PRESENCE_ALPN).await.unwrap();
        assert_eq!(
            request_admission_confirmation(&first).await,
            ADMISSION_ACCEPTED
        );
        assert_eq!(
            timeout(Duration::from_secs(1), events.recv())
                .await
                .unwrap()
                .unwrap()
                .state,
            ReachabilityState::Online
        );
        let delay = Arc::new(DelayedAdmission {
            checking: tokio::sync::Notify::new(),
            proceed: tokio::sync::Semaphore::new(0),
        });
        *addresses.delay.lock().unwrap() = Some(delay.clone());
        let check = tokio::spawn({
            let adapter = adapter.clone();
            async move { adapter.verify_reachable(&DeviceId::new("b")).await }
        });
        timeout(Duration::from_secs(1), delay.checking.notified())
            .await
            .unwrap();
        let second = b.connect(a.addr(), PRESENCE_ALPN).await.unwrap();
        assert_eq!(
            request_admission_confirmation(&second).await,
            ADMISSION_ACCEPTED
        );
        assert_eq!(
            timeout(Duration::from_millis(500), events.recv())
                .await
                .expect("the pending check must be told about the newer success")
                .unwrap()
                .state,
            ReachabilityState::Online
        );
        check.abort();
        assert!(check.await.unwrap_err().is_cancelled());
        adapter.disconnect_all().await;
        router.shutdown().await.unwrap();
        b.close().await;
    }

    #[tokio::test]
    async fn simultaneous_bidirectional_presence_dials_remain_online() {
        let a = bound_endpoint().await;
        let b = bound_endpoint().await;
        wait_for_direct_addrs(&a).await;
        wait_for_direct_addrs(&b).await;
        let a_members = Arc::new(MemMemberRepo::default());
        let b_members = Arc::new(MemMemberRepo::default());
        a_members.seed(member_for_endpoint(&b, "b"));
        b_members.seed(member_for_endpoint(&a, "a"));
        let a_addresses = Arc::new(FakePeerAddressRepo::default());
        let b_addresses = Arc::new(FakePeerAddressRepo::default());
        a_addresses.seed(record(
            &DeviceId::new("b"),
            postcard::to_stdvec(&b.addr()).unwrap(),
        ));
        b_addresses.seed(record(
            &DeviceId::new("a"),
            postcard::to_stdvec(&a.addr()).unwrap(),
        ));
        let a_adapter = build_adapter_with_member_repo(a.clone(), a_addresses, a_members);
        let b_adapter = build_adapter_with_member_repo(b.clone(), b_addresses, b_members);
        let a_router = Router::builder((*a).clone())
            .accept(PRESENCE_ALPN, a_adapter.handler())
            .spawn();
        let b_router = Router::builder((*b).clone())
            .accept(PRESENCE_ALPN, b_adapter.handler())
            .spawn();
        let a_id = DeviceId::new("a");
        let b_id = DeviceId::new("b");
        for _ in 0..3 {
            let (left, right) = tokio::join!(
                a_adapter.verify_reachable(&b_id),
                b_adapter.verify_reachable(&a_id)
            );
            assert_eq!(left.unwrap(), ReachabilityState::Online);
            assert_eq!(right.unwrap(), ReachabilityState::Online);
        }
        assert_eq!(
            a_adapter.current_state(&DeviceId::new("b")).await,
            ReachabilityState::Online
        );
        assert_eq!(
            b_adapter.current_state(&DeviceId::new("a")).await,
            ReachabilityState::Online
        );
        a_adapter.disconnect_all().await;
        b_adapter.disconnect_all().await;
        a_router.shutdown().await.unwrap();
        b_router.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn last_inbound_connection_close_reports_offline() {
        let a = bound_endpoint().await;
        let b = bound_endpoint().await;
        wait_for_direct_addrs(&a).await;
        wait_for_direct_addrs(&b).await;
        let members = Arc::new(MemMemberRepo::default());
        members.seed(member_for_endpoint(&a, "a"));
        let adapter = build_adapter_with_member_repo(
            b.clone(),
            Arc::new(FakePeerAddressRepo::default()),
            members,
        );
        let mut events = adapter.subscribe();
        let router = Router::builder((*b).clone())
            .accept(PRESENCE_ALPN, adapter.handler())
            .spawn();
        let connection = a.connect(b.addr(), PRESENCE_ALPN).await.unwrap();
        assert_eq!(
            request_admission_confirmation(&connection).await,
            ADMISSION_ACCEPTED
        );
        assert_eq!(
            timeout(Duration::from_secs(1), events.recv())
                .await
                .unwrap()
                .unwrap()
                .state,
            ReachabilityState::Online
        );
        connection.close(0u32.into(), b"test_close");
        assert_eq!(
            timeout(Duration::from_secs(1), events.recv())
                .await
                .expect("last inbound presence connection must report offline")
                .unwrap()
                .state,
            ReachabilityState::Offline
        );
        router.shutdown().await.unwrap();
        a.close().await;
    }

    #[tokio::test]
    async fn closing_old_inbound_connection_keeps_new_connection_online() {
        let a = bound_endpoint().await;
        let b = bound_endpoint().await;
        wait_for_direct_addrs(&a).await;
        wait_for_direct_addrs(&b).await;
        let members = Arc::new(MemMemberRepo::default());
        members.seed(member_for_endpoint(&a, "a"));
        let adapter = build_adapter_with_member_repo(
            b.clone(),
            Arc::new(FakePeerAddressRepo::default()),
            members,
        );
        let mut events = adapter.subscribe();
        let router = Router::builder((*b).clone())
            .accept(PRESENCE_ALPN, adapter.handler())
            .spawn();
        let old = a.connect(b.addr(), PRESENCE_ALPN).await.unwrap();
        assert_eq!(
            request_admission_confirmation(&old).await,
            ADMISSION_ACCEPTED
        );
        assert_eq!(
            timeout(Duration::from_secs(1), events.recv())
                .await
                .unwrap()
                .unwrap()
                .state,
            ReachabilityState::Online
        );
        let new = a.connect(b.addr(), PRESENCE_ALPN).await.unwrap();
        assert_eq!(
            request_admission_confirmation(&new).await,
            ADMISSION_ACCEPTED
        );
        old.close(0u32.into(), b"old_connection");
        assert!(timeout(Duration::from_millis(100), events.recv())
            .await
            .is_err());
        assert_eq!(
            adapter.current_state(&DeviceId::new("a")).await,
            ReachabilityState::Online
        );
        new.close(0u32.into(), b"last_connection");
        assert_eq!(
            timeout(Duration::from_secs(1), events.recv())
                .await
                .unwrap()
                .unwrap()
                .state,
            ReachabilityState::Offline
        );
        router.shutdown().await.unwrap();
        a.close().await;
    }

    #[tokio::test]
    async fn unknown_peer_is_not_kept_online_after_membership_is_cleared() {
        let endpoint_a = bound_endpoint().await;
        wait_for_direct_addrs(&endpoint_a).await;
        let endpoint_b = bound_endpoint().await;
        wait_for_direct_addrs(&endpoint_b).await;

        let b_adapter = build_adapter(
            Arc::clone(&endpoint_b),
            Arc::new(FakePeerAddressRepo::default()),
        );
        let router_b = Router::builder((*endpoint_b).clone())
            .accept(PRESENCE_ALPN, b_adapter.handler())
            .spawn();

        let connection = endpoint_a
            .connect(endpoint_b.addr(), PRESENCE_ALPN)
            .await
            .expect("transport connection succeeds before membership rejection");
        assert_eq!(
            request_admission_confirmation(&connection).await,
            ADMISSION_REJECTED
        );

        timeout(Duration::from_secs(1), connection.closed())
            .await
            .expect("unknown peer connection is closed promptly");

        router_b.shutdown().await.ok();
        endpoint_a.close().await;
    }

    #[tokio::test]
    async fn ensure_reachable_on_known_address_returns_online() {
        let (endpoint_a, endpoint_b, b_blob, b_device_id, router_b) = setup_two_endpoints().await;

        let repo = Arc::new(FakePeerAddressRepo::default());
        repo.seed(record(&b_device_id, b_blob));

        let adapter = build_adapter(endpoint_a.clone(), repo.clone());
        let mut subscriber = adapter.subscribe();

        let state = timeout(DIAL_BUDGET, adapter.ensure_reachable(&b_device_id))
            .await
            .expect("ensure_reachable within budget")
            .expect("ensure_reachable succeeded");
        assert_eq!(state, ReachabilityState::Online);

        assert_eq!(
            adapter.current_state(&b_device_id).await,
            ReachabilityState::Online,
        );

        let event = timeout(Duration::from_secs(1), subscriber.recv())
            .await
            .expect("subscriber received within 1s")
            .expect("event channel not closed");
        assert_eq!(event.device_id, b_device_id);
        assert_eq!(event.state, ReachabilityState::Online);

        // Teardown.
        router_b.shutdown().await.expect("router_b shutdown clean");
        endpoint_a.close().await;
        drop(endpoint_b);
    }

    #[tokio::test]
    async fn third_party_relayed_address_connects_after_relaying_peer_stops() {
        let endpoint_a = bound_endpoint().await;
        wait_for_direct_addrs(&endpoint_a).await;
        let endpoint_b = bound_endpoint().await;
        wait_for_direct_addrs(&endpoint_b).await;
        let endpoint_c = bound_endpoint().await;
        wait_for_direct_addrs(&endpoint_c).await;

        let c_member_repo = Arc::new(MemMemberRepo::default());
        c_member_repo.seed(member_for_endpoint(&endpoint_a, "device-a"));
        let c_adapter = build_adapter_with_member_repo(
            Arc::clone(&endpoint_c),
            Arc::new(FakePeerAddressRepo::default()),
            c_member_repo,
        );
        let router_c = Router::builder((*endpoint_c).clone())
            .accept(PRESENCE_ALPN, c_adapter.handler())
            .spawn();

        let c_device_id = DeviceId::new("device-c");
        let address_observed_by_b = endpoint_c.addr();
        let relayed_blob = postcard::to_stdvec(&address_observed_by_b)
            .expect("encode address observed by third party");

        endpoint_b.close().await;

        let a_repo = Arc::new(FakePeerAddressRepo::default());
        a_repo.seed(record(&c_device_id, relayed_blob));
        let a_adapter = build_adapter(Arc::clone(&endpoint_a), a_repo);

        let state = timeout(DIAL_BUDGET, a_adapter.ensure_reachable(&c_device_id))
            .await
            .expect("third-party address dial completes within budget")
            .expect("third-party address dial succeeds");
        assert_eq!(state, ReachabilityState::Online);

        router_c.shutdown().await.expect("router_c shutdown clean");
        endpoint_a.close().await;
    }

    #[tokio::test]
    async fn ensure_reachable_on_unknown_device_returns_no_address() {
        let endpoint_a = bound_endpoint().await;
        let repo = Arc::new(FakePeerAddressRepo::default());
        let adapter = build_adapter(endpoint_a.clone(), repo);

        let ghost = DeviceId::new("device-with-no-record");
        match adapter.ensure_reachable(&ghost).await {
            Err(PresenceError::NoAddress(id)) => assert_eq!(id.as_str(), ghost.as_str()),
            other => panic!("expected NoAddress, got {other:?}"),
        }

        endpoint_a.close().await;
    }

    #[tokio::test]
    async fn peer_shutdown_triggers_offline_event_within_budget() {
        let (endpoint_a, endpoint_b, b_blob, b_device_id, router_b) = setup_two_endpoints().await;

        let repo = Arc::new(FakePeerAddressRepo::default());
        repo.seed(record(&b_device_id, b_blob));

        let adapter = build_adapter(endpoint_a.clone(), repo);
        let mut subscriber = adapter.subscribe();

        let state = adapter
            .ensure_reachable(&b_device_id)
            .await
            .expect("initial dial succeeded");
        assert_eq!(state, ReachabilityState::Online);

        // Drain the Online event before we force teardown so the next
        // `subscriber.recv()` is guaranteed to be the Offline transition.
        let first = timeout(Duration::from_secs(1), subscriber.recv())
            .await
            .expect("initial online event arrives")
            .expect("event channel open");
        assert_eq!(first.state, ReachabilityState::Online);

        // Tear the acceptor side down.
        router_b.shutdown().await.expect("router_b shutdown clean");
        endpoint_b.close().await;

        let offline = timeout(OFFLINE_BUDGET, subscriber.recv())
            .await
            .expect("offline event within 10s budget")
            .expect("event channel open");
        assert_eq!(offline.state, ReachabilityState::Offline);
        assert_eq!(offline.device_id, b_device_id);

        assert_eq!(
            adapter.current_state(&b_device_id).await,
            ReachabilityState::Unknown,
            "a closed presence connection must allow the next clipboard dispatch to redial",
        );

        endpoint_a.close().await;
    }

    #[tokio::test]
    async fn current_state_defaults_to_unknown_before_probe() {
        let endpoint_a = bound_endpoint().await;
        let repo = Arc::new(FakePeerAddressRepo::default());
        let adapter = build_adapter(endpoint_a.clone(), repo);

        let never_seen = DeviceId::new("never-probed");
        assert_eq!(
            adapter.current_state(&never_seen).await,
            ReachabilityState::Unknown,
        );

        endpoint_a.close().await;
    }

    #[tokio::test]
    async fn ensure_reachable_after_offline_redials_successfully() {
        // Simpler coverage for the recovery half: dial against a peer
        // whose addr record points at a well-formed but unreachable
        // `EndpointAddr` (no route back), observe `Offline`, then swap the
        // repo entry for a live peer and redial — expect `Online`.
        //
        // This sidesteps the iroh-secret-identity plumbing that a full
        // restart-on-same-endpoint test would need (keypairs are not
        // rebindable once an endpoint is dropped). See plan §8 for the
        // test-strategy note.
        let (endpoint_a, endpoint_b, b_blob, b_device_id, router_b) = setup_two_endpoints().await;

        let repo = Arc::new(FakePeerAddressRepo::default());

        // Seed with an unroutable address first: craft an `EndpointAddr`
        // whose id is B's but whose transport addr list is empty (relays
        // disabled → no fallback → dial fails quickly).
        let dead_addr = EndpointAddr::new(endpoint_b.id());
        let dead_blob = postcard::to_stdvec(&dead_addr).expect("encode");
        repo.seed(record(&b_device_id, dead_blob));

        let adapter = build_adapter(endpoint_a.clone(), repo.clone());

        let first = timeout(OFFLINE_BUDGET, adapter.ensure_reachable(&b_device_id))
            .await
            .expect("dial resolves within budget")
            .expect("ensure_reachable completed (Offline is Ok)");
        assert_eq!(first, ReachabilityState::Offline);
        assert_eq!(
            adapter.current_state(&b_device_id).await,
            ReachabilityState::Offline,
        );

        // Now swap in the live blob and redial.
        repo.seed(record(&b_device_id, b_blob));
        let second = timeout(DIAL_BUDGET, adapter.ensure_reachable(&b_device_id))
            .await
            .expect("re-dial within budget")
            .expect("re-dial succeeded");
        assert_eq!(second, ReachabilityState::Online);
        assert_eq!(
            adapter.current_state(&b_device_id).await,
            ReachabilityState::Online,
        );

        router_b.shutdown().await.expect("router_b shutdown clean");
        endpoint_a.close().await;
    }

    // -- mark_offline sticky window ------------------------------------------

    /// Verdict — `mark_offline` arms a sticky window in `last_offline_at`
    /// that survives a wipe of `last_state`. This is what lets #886 collapse
    /// `DispatchClipboardEntryUseCase::recent_dial_failures` onto the
    /// adapter: the negative window lives here, not in the use case.
    ///
    /// The test forces `last_state` empty after `mark_offline` to isolate
    /// the stamp's projection path; under normal flow `last_state` already
    /// reports `Offline` and `current_state` returns from the early branch.
    #[tokio::test]
    async fn mark_offline_arms_sticky_window_projected_by_current_state() {
        let endpoint_a = bound_endpoint().await;
        let repo = Arc::new(FakePeerAddressRepo::default());
        let adapter = build_adapter(endpoint_a.clone(), repo);

        let device = DeviceId::new("sticky-target");
        adapter.mark_offline(&device).await;

        // Under normal flow current_state returns Offline via `last_state`.
        assert_eq!(
            adapter.current_state(&device).await,
            ReachabilityState::Offline,
        );

        // Simulate a future code path that drops `last_state[device]`
        // before the sticky window has expired. `current_state` must still
        // project the stamp as `Offline`.
        adapter.last_state.lock().await.remove(&device);
        assert_eq!(
            adapter.current_state(&device).await,
            ReachabilityState::Offline,
        );

        endpoint_a.close().await;
    }

    /// Verdict — a successful outbound dial clears the sticky window left
    /// over from an earlier `mark_offline`, so a peer that recovers does
    /// not get pinned to Offline for the rest of `MARK_OFFLINE_STICKY_TTL`.
    #[tokio::test]
    async fn successful_dial_clears_mark_offline_sticky_window() {
        let (endpoint_a, _endpoint_b, b_blob, b_device_id, router_b) = setup_two_endpoints().await;

        let repo = Arc::new(FakePeerAddressRepo::default());
        repo.seed(record(&b_device_id, b_blob));

        let adapter = build_adapter(endpoint_a.clone(), repo);

        // Stamp the negative window first, as the dispatch adapter would
        // after a failed dial.
        adapter.mark_offline(&b_device_id).await;
        assert!(
            adapter
                .last_offline_at
                .lock()
                .await
                .contains_key(&b_device_id),
            "mark_offline must record a stamp",
        );

        // Now succeed a dial against B. The Ok branch in `dial_and_track`
        // must remove the stamp; otherwise consumers reading
        // `current_state` after a `last_state` reset would still see
        // Offline for ~30s after recovery.
        let state = timeout(DIAL_BUDGET, adapter.ensure_reachable(&b_device_id))
            .await
            .expect("dial within budget")
            .expect("dial succeeded");
        assert_eq!(state, ReachabilityState::Online);
        assert!(
            !adapter
                .last_offline_at
                .lock()
                .await
                .contains_key(&b_device_id),
            "successful dial must clear the sticky Offline stamp",
        );

        router_b.shutdown().await.ok();
        endpoint_a.close().await;
    }

    #[tokio::test]
    async fn outbound_dial_rejected_by_remote_admission_never_reports_online() {
        let endpoint_a = bound_endpoint().await;
        wait_for_direct_addrs(&endpoint_a).await;
        let endpoint_b = bound_endpoint().await;
        wait_for_direct_addrs(&endpoint_b).await;

        let a_member = member_for_endpoint(&endpoint_a, "device-a");
        let b_member_repo = Arc::new(MemMemberRepo::default());
        b_member_repo.seed(a_member);
        let b_adapter = build_adapter_with_member_repo_and_admission(
            Arc::clone(&endpoint_b),
            Arc::new(FakePeerAddressRepo::default()),
            b_member_repo,
            false,
        );
        let router_b = Router::builder((*endpoint_b).clone())
            .accept(PRESENCE_ALPN, b_adapter.handler())
            .spawn();

        let b_device_id = DeviceId::new("device-b");
        let a_repo = Arc::new(FakePeerAddressRepo::default());
        a_repo.seed(record(
            &b_device_id,
            postcard::to_stdvec(&endpoint_b.addr()).expect("encode receiver address"),
        ));
        let a_adapter = build_adapter(Arc::clone(&endpoint_a), a_repo);
        let mut subscriber = a_adapter.subscribe();

        let state = timeout(DIAL_BUDGET, a_adapter.verify_reachable(&b_device_id))
            .await
            .expect("verification finishes within budget")
            .expect("admission rejection is a reachability verdict");

        assert_eq!(state, ReachabilityState::Offline);
        assert_eq!(
            a_adapter.current_state(&b_device_id).await,
            ReachabilityState::Offline
        );
        if let Ok(Ok(event)) = timeout(Duration::from_millis(500), subscriber.recv()).await {
            assert_eq!(
                event.state,
                ReachabilityState::Offline,
                "remote admission rejection must not emit a transient Online event"
            );
        }

        router_b.shutdown().await.ok();
        endpoint_a.close().await;
    }

    // -- Inbound-driven Online flip ------------------------------------------

    /// Build a `SpaceMember` whose `identity_fingerprint` matches the
    /// pubkey of `endpoint`, so the presence handler can reverse-resolve an
    /// inbound `Connection::remote_id()` from that endpoint back to
    /// `device_id`.
    fn member_for_endpoint(endpoint: &Endpoint, device_id: &str) -> SpaceMember {
        let factory = Sha256IdentityFingerprintFactory;
        let fp = factory
            .from_public_key(endpoint.id().as_bytes())
            .expect("derive fingerprint from endpoint pubkey");
        SpaceMember {
            device_id: DeviceId::new(device_id),
            device_name: device_id.to_string(),
            identity_fingerprint: fp,
            joined_at: Utc::now(),
            sync_preferences: MemberSyncPreferences::default(),
        }
    }

    /// Verdict — when an Offline (or Unknown) peer dials us at
    /// `PRESENCE_ALPN`, the handler reverse-resolves the remote pubkey to
    /// the seeded `DeviceId`, writes `last_state[device]=Online`, and
    /// emits exactly one `Online` event.
    #[tokio::test]
    async fn accept_from_known_peer_flips_offline_to_online() {
        let endpoint_a = bound_endpoint().await;
        wait_for_direct_addrs(&endpoint_a).await;
        let endpoint_b = bound_endpoint().await;
        wait_for_direct_addrs(&endpoint_b).await;

        let a_member = member_for_endpoint(&endpoint_a, "device-a");
        let a_device_id = a_member.device_id.clone();
        let b_member_repo = Arc::new(MemMemberRepo::default());
        b_member_repo.seed(a_member);

        let b_peer_addr_repo: Arc<dyn PeerAddressRepositoryPort> =
            Arc::new(FakePeerAddressRepo::default());
        let b_member_repo_dyn: Arc<dyn MemberRepositoryPort> = b_member_repo;
        let b_adapter = build_adapter_with_member_repo(
            Arc::clone(&endpoint_b),
            b_peer_addr_repo,
            b_member_repo_dyn,
        );
        let mut subscriber = b_adapter.subscribe();

        // Before any inbound dial, B has no opinion on A's reachability.
        assert_eq!(
            b_adapter.current_state(&a_device_id).await,
            ReachabilityState::Unknown,
        );

        let router_b = Router::builder((*endpoint_b).clone())
            .accept(PRESENCE_ALPN, b_adapter.handler())
            .spawn();

        // A dials B directly — this exercises B's accept handler without
        // pulling in A's own adapter. The connection is held open by the
        // handler until the test drops it.
        let b_addr = endpoint_b.addr();
        let conn = timeout(DIAL_BUDGET, endpoint_a.connect(b_addr, PRESENCE_ALPN))
            .await
            .expect("connect within budget")
            .expect("A dial B succeeds");
        assert_eq!(
            request_admission_confirmation(&conn).await,
            ADMISSION_ACCEPTED
        );

        let event = timeout(Duration::from_secs(3), subscriber.recv())
            .await
            .expect("inbound Online event arrives within 3s")
            .expect("event channel open");
        assert_eq!(event.device_id, a_device_id);
        assert_eq!(event.state, ReachabilityState::Online);

        assert_eq!(
            b_adapter.current_state(&a_device_id).await,
            ReachabilityState::Online,
        );

        drop(conn);
        router_b.shutdown().await.ok();
        endpoint_a.close().await;
    }

    /// Verdict — repeated inbound dials from the same already-Online peer
    /// must NOT re-broadcast. Each keepalive tick from a stable peer would
    /// otherwise spam subscribers with duplicate events.
    #[tokio::test]
    async fn accept_already_online_does_not_rebroadcast() {
        let endpoint_a = bound_endpoint().await;
        wait_for_direct_addrs(&endpoint_a).await;
        let endpoint_b = bound_endpoint().await;
        wait_for_direct_addrs(&endpoint_b).await;

        let a_member = member_for_endpoint(&endpoint_a, "device-a");
        let b_member_repo = Arc::new(MemMemberRepo::default());
        b_member_repo.seed(a_member);

        let b_adapter = build_adapter_with_member_repo(
            Arc::clone(&endpoint_b),
            Arc::new(FakePeerAddressRepo::default()) as Arc<dyn PeerAddressRepositoryPort>,
            b_member_repo as Arc<dyn MemberRepositoryPort>,
        );
        let mut subscriber = b_adapter.subscribe();

        let router_b = Router::builder((*endpoint_b).clone())
            .accept(PRESENCE_ALPN, b_adapter.handler())
            .spawn();

        let b_addr = endpoint_b.addr();
        let conn1 = timeout(
            DIAL_BUDGET,
            endpoint_a.connect(b_addr.clone(), PRESENCE_ALPN),
        )
        .await
        .expect("first connect within budget")
        .expect("first dial succeeds");
        assert_eq!(
            request_admission_confirmation(&conn1).await,
            ADMISSION_ACCEPTED
        );
        let first = timeout(Duration::from_secs(3), subscriber.recv())
            .await
            .expect("first event arrives")
            .expect("channel open");
        assert_eq!(first.state, ReachabilityState::Online);

        // Second dial — B's `last_state[A]` is already `Online`, so the
        // handler must skip the broadcast.
        let conn2 = timeout(DIAL_BUDGET, endpoint_a.connect(b_addr, PRESENCE_ALPN))
            .await
            .expect("second connect within budget")
            .expect("second dial succeeds");
        assert_eq!(
            request_admission_confirmation(&conn2).await,
            ADMISSION_ACCEPTED
        );

        // Drain attempt with a tight deadline: any re-broadcast would
        // arrive within milliseconds of the second connection landing.
        let no_event = timeout(Duration::from_millis(500), subscriber.recv()).await;
        assert!(
            no_event.is_err(),
            "expected no second event, got {:?}",
            no_event.ok().map(|r| r.map(|e| (e.device_id, e.state))),
        );

        drop(conn1);
        drop(conn2);
        router_b.shutdown().await.ok();
        endpoint_a.close().await;
    }

    /// Verdict — a roster-resolved peer rejected by the current MLS group
    /// must not alter presence state or emit an online event.
    #[tokio::test]
    async fn accept_known_but_unadmitted_peer_does_not_touch_state() {
        let endpoint_a = bound_endpoint().await;
        wait_for_direct_addrs(&endpoint_a).await;
        let endpoint_b = bound_endpoint().await;
        wait_for_direct_addrs(&endpoint_b).await;

        let a_member = member_for_endpoint(&endpoint_a, "revoked-device");
        let a_device_id = a_member.device_id.clone();
        let b_member_repo = Arc::new(MemMemberRepo::default());
        b_member_repo.seed(a_member);
        let b_adapter = build_adapter_with_member_repo_and_admission(
            Arc::clone(&endpoint_b),
            Arc::new(FakePeerAddressRepo::default()),
            b_member_repo,
            false,
        );
        let mut subscriber = b_adapter.subscribe();
        let router_b = Router::builder((*endpoint_b).clone())
            .accept(PRESENCE_ALPN, b_adapter.handler())
            .spawn();

        let conn = timeout(
            DIAL_BUDGET,
            endpoint_a.connect(endpoint_b.addr(), PRESENCE_ALPN),
        )
        .await
        .expect("connect within budget")
        .expect("dial succeeds");
        assert_eq!(
            request_admission_confirmation(&conn).await,
            ADMISSION_REJECTED
        );
        assert!(
            timeout(Duration::from_millis(500), subscriber.recv())
                .await
                .is_err(),
            "an unadmitted peer must not emit an online event"
        );
        assert_eq!(
            b_adapter.current_state(&a_device_id).await,
            ReachabilityState::Unknown
        );

        drop(conn);
        router_b.shutdown().await.ok();
        endpoint_a.close().await;
    }

    /// Verdict — an inbound dial from a peer whose pubkey is NOT in
    /// `member_repo` must hold the connection but leave presence state
    /// untouched and emit no event. Mirrors the receiver adapter's
    /// "unknown peer" tolerance.
    #[tokio::test]
    async fn accept_unknown_peer_does_not_touch_state() {
        let endpoint_a = bound_endpoint().await;
        wait_for_direct_addrs(&endpoint_a).await;
        let endpoint_b = bound_endpoint().await;
        wait_for_direct_addrs(&endpoint_b).await;

        // member_repo deliberately empty — A's fingerprint will not
        // resolve to any DeviceId.
        let b_adapter = build_adapter_with_member_repo(
            Arc::clone(&endpoint_b),
            Arc::new(FakePeerAddressRepo::default()) as Arc<dyn PeerAddressRepositoryPort>,
            Arc::new(MemMemberRepo::default()) as Arc<dyn MemberRepositoryPort>,
        );
        let mut subscriber = b_adapter.subscribe();

        let router_b = Router::builder((*endpoint_b).clone())
            .accept(PRESENCE_ALPN, b_adapter.handler())
            .spawn();

        let b_addr = endpoint_b.addr();
        let conn = timeout(DIAL_BUDGET, endpoint_a.connect(b_addr, PRESENCE_ALPN))
            .await
            .expect("connect within budget")
            .expect("dial succeeds");
        assert_eq!(
            request_admission_confirmation(&conn).await,
            ADMISSION_REJECTED
        );

        // Give the handler time to run resolve_device, then assert no
        // event landed.
        let no_event = timeout(Duration::from_millis(500), subscriber.recv()).await;
        assert!(
            no_event.is_err(),
            "unknown peer must not produce a presence event",
        );

        // No DeviceId was ever associated with A, so any current_state
        // probe returns Unknown — the canonical "no opinion yet" verdict.
        assert_eq!(
            b_adapter
                .current_state(&DeviceId::new("not-a-real-id"))
                .await,
            ReachabilityState::Unknown,
        );

        drop(conn);
        router_b.shutdown().await.ok();
        endpoint_a.close().await;
    }
}
