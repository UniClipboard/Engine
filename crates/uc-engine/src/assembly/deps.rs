//! Wiring output bundles.
//!
//! The internal data types produced by engine dependency assembly:
//! the process-resident `WiredDependencies` plus the consumer-grouped bundles
//! (`SyncEngineDeps` / `DaemonRuntimeDeps` / `SharedRuntimeDeps`) and the
//! one-shot `BackgroundRuntimeDeps`. These carry no behavior — the wiring logic
//! that fills them lives in `wiring::wire`.

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::broadcast;

use uc_application::deps::{
    ClearProfileStatePort, CurrentMemberSignaturePort, ProfileLifecycleRepositoryPort,
    WipeProfileKeysPort,
};
use uc_core::clipboard::ActiveClipboardState;
use uc_core::ports::blob::BlobReferenceRepositoryPort;
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

    #[error("profile storage upgrade did not reach a runnable state")]
    StorageUpgradePending,

    #[error("profile storage upgrade failed")]
    StorageUpgrade {
        #[source]
        source: uc_infra::security::ProfileStorageUpgradeError,
    },

    #[error("profile storage upgrade prerequisite failed")]
    StorageUpgradePrerequisite {
        #[source]
        source: anyhow::Error,
    },
}

/// P2P / iroh sync-engine assembly inputs. Sole consumer:
/// The internal sync-engine assembly consumes these ports and paths. They never
/// flow through `ApplicationDeps` — the `SpaceFacade` they assemble lives in
/// uc-application and is injected by this bundle at wire time, not by the
/// AppFacade path.
#[derive(Clone)]
pub struct SyncEngineDeps {
    /// Engine-owned primitive ports needed to construct concrete Iroh adapters.
    pub device_identity: Arc<dyn uc_core::ports::DeviceIdentityPort>,
    pub settings: Arc<dyn uc_core::ports::SettingsPort>,
    pub member_repo: Arc<dyn uc_core::membership::MemberRepositoryPort>,
    pub trusted_peer_repo: Arc<dyn uc_core::trusted_peer::TrustedPeerRepositoryPort>,
    pub fingerprint: Arc<dyn uc_core::ports::security::IdentityFingerprintFactoryPort>,
    pub clock: Arc<dyn uc_core::ports::ClockPort>,
    pub space_access: uc_application::deps::SpaceAccessPorts,
    #[cfg(test)]
    pub analytics: Arc<dyn uc_observability_contract::analytics::AnalyticsPort>,
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
    /// Independent member signatures from the current OpenMLS member tree.
    pub current_member_signatures: Arc<dyn CurrentMemberSignaturePort>,
    /// The same unlocked session used by space access and encrypted storage.
    pub membership_session: Arc<uc_infra::space::InMemorySession>,
    /// MasterKey-encrypted single membership ledger used by the new Space application.
    pub membership_ledger: Arc<
        uc_infra::space::SqliteMembershipLedger<Arc<uc_infra::db::executor::DieselSqliteExecutor>>,
    >,
    /// MasterKey-encrypted aggregate repository shared by all admission roles.
    pub admission_state: Arc<
        uc_infra::space::SqliteSpaceAdmissionState<
            Arc<uc_infra::db::executor::DieselSqliteExecutor>,
        >,
    >,
    /// Space-generation-bound OPAQUE setup and registration lifecycle.
    pub admission_credentials: Arc<
        uc_infra::space::SqliteSpaceAdmissionCredentials<
            Arc<uc_infra::db::executor::DieselSqliteExecutor>,
        >,
    >,
    pub admission_space_transition: Arc<dyn uc_application::deps::AdmissionSpaceTransitionPort>,
    pub re_pairing_state_store: Arc<dyn uc_application::deps::RePairingStateStorePort>,
    pub membership_branch_transition_executor:
        Arc<dyn uc_application::deps::AdvanceMembershipBranchTransitionPort>,
    pub active_generation_manifest_store:
        Arc<uc_infra::security::ActiveSpaceGenerationManifestStore>,
    pub device_management_reset_data: Arc<dyn uc_application::deps::DeviceManagementResetDataPort>,
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
    /// The same Arc is also coerced into `ApplicationDeps.mobile_sync.endpoint_info`
    /// (facade read side), sharing one allocation — daemon writes, facade reads
    /// (ports.md §8.3 single-adapter-reuse).
    pub mobile_sync_endpoint_info:
        Arc<uc_infra::mobile_sync::InMemoryMobileSyncEndpointInfoAdapter>,
}

/// LAN compatibility 组合根所需的 Application 被动端口。
#[cfg(feature = "lan-compat")]
#[derive(Clone)]
pub struct MobileSyncApplicationDeps {
    pub clock: Arc<dyn uc_core::ports::ClockPort>,
    pub settings: Arc<dyn uc_core::ports::SettingsPort>,
    pub mobile_consumable_load:
        Arc<dyn uc_core::ports::clipboard::LoadMobileConsumableClipboardPort>,
    pub entry_repo: Arc<dyn uc_core::ports::clipboard::GetClipboardEntryPort>,
    pub selection_repo: Arc<dyn uc_core::ports::clipboard::ClipboardSelectionRepositoryPort>,
    pub representation_repo: Arc<dyn uc_core::ports::clipboard::GetRepresentationPort>,
    pub payload_resolver: Arc<dyn uc_core::ports::clipboard::ClipboardPayloadResolverPort>,
    pub blob_reader: Arc<dyn uc_core::blob::ports::BlobReaderPort>,
    pub analytics: Arc<dyn uc_observability_contract::analytics::AnalyticsPort>,
    pub find_entry_by_snapshot_hash:
        Arc<dyn uc_core::ports::clipboard::FindEntryIdBySnapshotHashPort>,
    pub check_entry_availability: Arc<dyn uc_core::ports::clipboard::CheckEntryAvailabilityPort>,
}

/// Process-level handles shared by ≥2 assembly targets (space-setup,
/// daemon-runtime, CLI-appfacade). Grouped into a named "shared" bundle rather
/// than left top-level because "shared by multiple targets" is itself the
/// meaningful boundary; mirrors the [`BackgroundRuntimeDeps`] precedent.
#[derive(Clone)]
pub struct SharedRuntimeDeps {
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
    /// Application 唯一顶层对象图；Engine 只能读取 adapter 输入或启动
    /// 一个具体 Application runtime，不再分发领域 deps。
    pub application: uc_application::facade::ApplicationAssembly,
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
    #[cfg(feature = "lan-compat")]
    pub mobile_sync_application: MobileSyncApplicationDeps,
    /// Process-level handles shared by ≥2 assembly targets (see
    /// [`SharedRuntimeDeps`]).
    pub shared: SharedRuntimeDeps,
}
