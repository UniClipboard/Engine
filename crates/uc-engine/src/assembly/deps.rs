//! Wiring output bundles.
//!
//! The internal data types produced by engine dependency assembly:
//! the process-resident `WiredDependencies` plus the consumer-grouped bundles
//! (`SyncEngineDeps` / `DaemonRuntimeDeps` / `SharedRuntimeDeps`) and the
//! one-shot `BackgroundRuntimeDeps`. These carry no behavior — the wiring logic
//! that fills them lives in `wiring::wire`.

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::{broadcast, mpsc};

use uc_application::deps::{
    AppDeps, ClearProfileStatePort, CurrentMemberSignaturePort, MembershipHistoryRepositoryPort,
    ProfileLifecycleRepositoryPort, SpaceMembershipStateRepositoryPort, WipeProfileKeysPort,
};
use uc_core::clipboard::ActiveClipboardState;
use uc_core::ids::RepresentationId;
use uc_core::ports::blob::BlobReferenceRepositoryPort;
use uc_infra::clipboard::{RepresentationCache, SpoolManager};
use uc_observability_contract::analytics::AnalyticsFacade;

/// Result type for wiring operations
pub type WiringResult<T> = Result<T, WiringError>;

/// Errors during dependency injection
#[derive(Debug, thiserror::Error)]
pub enum WiringError {
    #[error("Database initialization failed: {0}")]
    DatabaseInit(String),

    #[error("Clipboard initialization failed: {0}")]
    ClipboardInit(String),

    #[error("Blob storage initialization failed: {0}")]
    BlobStorageInit(String),

    #[error("Settings repository initialization failed: {0}")]
    SettingsInit(String),

    #[error("Thumbnail generator initialization failed: {0}")]
    ThumbnailInit(String),
}

/// Background runtime components that must be started after async runtime is ready.
pub struct BackgroundRuntimeDeps {
    pub representation_cache: Arc<RepresentationCache>,
    pub spool_manager: Arc<SpoolManager>,
    pub worker_rx: mpsc::Receiver<RepresentationId>,
    pub spool_dir: PathBuf,
    pub spool_ttl_days: u64,
    pub worker_retry_max_attempts: u32,
    pub worker_retry_backoff_ms: u64,
}

/// P2P / iroh sync-engine assembly inputs. Sole consumer:
/// The internal sync-engine assembly consumes these ports and paths. They never
/// flow through `AppDeps` — the `SpaceFacade` they assemble lives in
/// uc-application and is injected by this bundle at wire time, not by the
/// AppFacade path.
#[derive(Clone)]
pub struct SyncEngineDeps {
    /// Dedicated file-backed storage for the long-lived iroh network identity.
    pub iroh_identity_storage: Arc<dyn uc_core::ports::SecureStoragePort>,
    /// Authoritative authorization check used by every inbound Iroh handler
    /// after it resolves an endpoint identity to a known device.
    pub peer_admission: Arc<dyn uc_core::membership::PeerAdmissionPort>,
    /// peer address repo — best-effort transport-address writes after pairing,
    /// dialed by F1 `ensure_reachable_all`.
    pub peer_addr_repo: Arc<dyn uc_core::ports::PeerAddressRepositoryPort>,
    /// Whole-table reset used when switching away from the active space.
    pub relationship_reset: Arc<dyn uc_core::membership::RelationshipStateResetPort>,
    /// Removes persisted security state from the prior space after a successful switch.
    pub space_security_reset: Arc<dyn uc_core::membership::SpaceSecurityStateResetPort>,
    /// Encrypted, active-space-scoped candidate address book.
    pub membership_candidate_repo: Arc<dyn uc_core::membership::MembershipCandidateRepositoryPort>,
    /// Atomic persistence boundary for a fully verified peer relationship.
    pub verified_peer_promotion: Arc<dyn uc_core::membership::VerifiedPeerPromotionPort>,
    /// Encrypted self-signed membership announcements for digest exchange.
    pub membership_announcement_repo:
        Arc<dyn uc_core::membership::MembershipAnnouncementRepositoryPort>,
    /// Encrypted pending membership batches for offline recipients.
    pub membership_outbox_repo: Arc<dyn uc_core::membership::MembershipOutboxRepositoryPort>,
    /// Encrypted security updates this device has applied and can relay.
    pub membership_applied_security_update_repo:
        Arc<dyn uc_core::membership::MembershipAppliedSecurityUpdateRepositoryPort>,
    /// Independent member signatures from the current OpenMLS member tree.
    pub current_member_signatures: Arc<dyn CurrentMemberSignaturePort>,
    /// The same unlocked session used by space access and encrypted storage.
    pub membership_session: Arc<uc_infra::security::InMemorySession>,
    /// Encrypted persistence for the unified workspace convergence state.
    pub workspace_convergence_repository: Arc<dyn SpaceMembershipStateRepositoryPort>,
    /// Profile-scoped encrypted persistence for durable admission attempts.
    pub admission_attempt_repository: Arc<dyn uc_application::deps::SpaceJoinRecordStorePort>,
    pub membership_history_repository: Arc<dyn MembershipHistoryRepositoryPort>,
    pub admission_space_transition: Arc<dyn uc_application::deps::AdmissionSpaceTransitionPort>,
    pub device_management_reset_data: Arc<dyn uc_application::deps::DeviceManagementResetDataPort>,
    pub legacy_migration_recovery: Arc<dyn uc_core::ports::setup::LegacyMigrationRecoveryPort>,
    /// plaintext-hash → ciphertext-digest dedupe cache (Slice 3 Phase 1).
    pub blob_reference_repo: Arc<dyn BlobReferenceRepositoryPort>,
    /// iroh-blobs store dir, used when assembling the iroh blob handler.
    pub iroh_blob_store_dir: PathBuf,
    /// Application-facing analytics entry point (pairing / switch-space events).
    pub analytics_facade: Arc<dyn AnalyticsFacade>,
}

/// daemon main-loop-only bypass deps.
#[cfg(feature = "lan-compat")]
#[derive(Clone)]
pub struct DaemonRuntimeDeps {
    /// Mobile-sync LAN endpoint-state singleton. **Concrete type**, not a trait
    /// object: the daemon LAN listener calls inherent `set` / `clear` on it
    /// (write side), which are not on the read-only `MobileSyncEndpointInfoPort`.
    /// The same Arc is also coerced into `AppDeps.mobile_sync.endpoint_info`
    /// (facade read side), sharing one allocation — daemon writes, facade reads
    /// (ports.md §8.3 single-adapter-reuse).
    pub mobile_sync_endpoint_info:
        Arc<uc_infra::mobile_sync::InMemoryMobileSyncEndpointInfoAdapter>,
}

/// Process-level handles shared by ≥2 assembly targets (space-setup,
/// daemon-runtime, CLI-appfacade). Grouped into a named "shared" bundle rather
/// than left top-level because "shared by multiple targets" is itself the
/// meaningful boundary; mirrors the [`BackgroundRuntimeDeps`] precedent.
#[derive(Clone)]
pub struct SharedRuntimeDeps {
    /// Shared receive-readiness gate: the same coordinator is injected into
    /// the file-transfer lifecycle (which opens it) and the clipboard
    /// inbound apply path (which waits on it), so one open gate unblocks
    /// every receiver.
    pub receive_readiness: Arc<uc_application::facade::ReceiveReadinessCoordinator>,
    /// Shared host-event bus created at wire time with the "logging" emitter
    /// already registered (event type names → `tracing::debug`), so non-GUI /
    /// CLI processes have a sensible default transport. Callers register their
    /// own transports on top; all consumers fan out into whatever transports
    /// are currently registered.
    pub host_event_bus: Arc<uc_application::facade::HostEventBus>,
    /// Delivery-result repo: `ClipboardSyncFacade` writes on fan-out completion,
    /// the view side reads.
    pub entry_delivery_repo: Arc<dyn uc_core::ports::EntryDeliveryRepositoryPort>,
    /// Read port over the same Diesel impl as
    /// `AppDeps.clipboard.clipboard_event_repo`; the view layer resolves the
    /// source device through it.
    pub clipboard_event_reader_repo: Arc<dyn uc_core::ports::ClipboardEventRepositoryPort>,
    /// Application entry point for the file-transfer lifecycle actions + seed +
    /// link. Shared by daemon runtime, `MobileSyncFacade` assembly, and the iroh
    /// blob path in `build_sync_engine_assembly`.
    pub file_transfer_facade: Arc<uc_application::facade::FileTransferFacade>,
    /// Single write boundary for all programmatic clipboard writes (guard
    /// registration + write + cleanup-on-error). Shared so the active-clipboard
    /// inbound worker and the restore/capture path keep one circuit-breaker +
    /// origin-guard state.
    pub clipboard_write_coordinator:
        Arc<uc_application::facade::clipboard_write::ClipboardWriteCoordinator>,
    /// Local cache dir for inbound blob materialization
    /// (`<file_cache_dir>/iroh-blobs/<entry_id>/`).
    pub file_cache_dir: PathBuf,
    /// Trusted-peer repository — pairing persist boundary (D19), roster trust
    /// checks, dispatch target filtering, CLI resend source lookup. Read by
    /// space-setup, daemon runtime, and the CLI AppFacade path, hence shared.
    pub trusted_peer_repo: Arc<dyn uc_core::TrustedPeerRepositoryPort>,
    /// Fan-out source for active-clipboard register advances. `BroadcastingAdvance`
    /// (wired into `active_clipboard_register`) is the sole publisher; the
    /// mobile-sync LAN SSE endpoint is the sole subscriber, cloning a `Receiver`
    /// per connection. Shared because both the wire-time decorator and the
    /// daemon's mobile-sync listener assembly need a handle to the same sender.
    pub active_clipboard_sse_source: broadcast::Sender<ActiveClipboardState>,
}

#[derive(Clone)]
pub struct ProfileResetDeps {
    pub lifecycle_repository: Arc<dyn ProfileLifecycleRepositoryPort>,
    pub keys: Arc<dyn WipeProfileKeysPort>,
    pub state: Arc<dyn ClearProfileStatePort>,
}

/// 进程级一次性装配产出的"持久"部分:进程内常驻的 `deps` 与按消费者归类的
/// 旁路 bundle(`sync_engine` / `daemon_runtime` / `shared`)。
///
/// 一次性消费的 [`BackgroundRuntimeDeps`](含 blob worker receiver)通过
/// 通过 tuple 返回值单独移交,不嵌在这里 —— 因为 mpsc
/// `Receiver` 不可 Clone。
///
/// 只被 daemon 进程路径消费(`apps/daemon` process_bootstrap → host →
/// bootstrap,加上 uc-bootstrap 的两个 assembler)。GUI/Tauri shell 走
/// `uc_desktop::gui_wiring::build_gui_client_context` 的 daemon HTTP client 路径,**不**碰
/// `WiredDependencies`;fan-out 是进程内 `ProcessRuntimeHandles` clone。
///
/// `Clone` 派生:所有字段都是 `Arc<dyn Port>` / `PathBuf` / Clone-able 嵌套
/// bundle,clone 廉价。
#[derive(Clone)]
pub struct WiredDependencies {
    /// 应用层 facade 装配输入(查询/历史/加密/搜索)。喂给
    /// `build_app_facade_from_deps`;CLI 与 daemon 路径共用。
    pub deps: AppDeps,
    /// P2P / iroh sync-engine assembly inputs (see [`SyncEngineDeps`]).
    pub sync_engine: SyncEngineDeps,
    pub profile_reset: ProfileResetDeps,
    /// daemon main-loop-only bypass deps (see [`DaemonRuntimeDeps`]).
    #[cfg(feature = "lan-compat")]
    pub daemon_runtime: DaemonRuntimeDeps,
    /// Mobile-sync port bundle assembled at wire time (ADR-018 stage 4);
    /// carried on the wiring so `uc-application` stays free of LAN-only
    /// port types.
    #[cfg(feature = "lan-compat")]
    pub mobile_sync_ports: uc_mobile_lan::MobileSyncPorts,
    /// Process-level handles shared by ≥2 assembly targets (see
    /// [`SharedRuntimeDeps`]).
    pub shared: SharedRuntimeDeps,
}
