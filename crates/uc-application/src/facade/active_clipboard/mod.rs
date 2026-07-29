//! `ActiveClipboardFacade` — application entry point for the cross-device
//! active-clipboard register convergence (issue #1017).
//!
//! Owns the inbound state use case, the background worker topology that drives
//! convergence, and the mobile-push activation announce
//! ([`ActiveClipboardFacade::announce_local_activation`]). Bootstrap crosses a
//! single lifecycle seam for worker startup, late restore-source attachment,
//! and coordinated shutdown.

mod reconcile;

pub use reconcile::{
    ActiveClipboardReconcileDeps, ActiveClipboardReconcileFacade, ActiveClipboardReconcileOutcome,
};

use std::{
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use async_trait::async_trait;
use thiserror::Error;
use tokio::sync::{
    broadcast,
    mpsc::{self, UnboundedReceiver},
    oneshot,
};
use tokio::task::{JoinHandle, JoinSet};
use tracing::{debug, instrument, warn};

use uc_core::clipboard::{ActiveClipboardState, ClipboardContentCategorySet};
use uc_core::ids::{DeviceId, EntryId};
use uc_core::ports::clipboard::{
    ActiveClipboardDispatchPort, ActiveClipboardPullClientPort, ActiveClipboardPullServePort,
    ActiveClipboardReceiverPort, AdvanceActiveClipboardPort, CheckEntryAvailabilityPort,
    ClipboardPayloadResolverPort, ClipboardSelectionRepositoryPort, EntryFileSetRepositoryPort,
    FindEntryIdBySnapshotHashPort, GetClipboardEntryPort, GetRepresentationPort,
    LoadActiveClipboardPort, TouchClipboardEntryPort, UpdateRepresentationProcessingResultPort,
};
use uc_core::ports::security::TransferCipherPort;
use uc_core::ports::space::IsSpaceUnlockedPort;
use uc_core::ports::{
    ClockPort, DeviceIdentityPort, PeerAddressRepositoryPort, PresencePort, SettingsPort,
};
use uc_core::{blob::ports::BlobReaderPort, MemberRepositoryPort};

use crate::clipboard_write::{
    ClipboardWriteCoordinator, ClipboardWriteIntent, LocalActiveRegisterAdvancer,
    MobileConsumabilityProbe, RestoreBroadcastRequest,
};
use crate::facade::blob_transfer::{BlobTransferFacade, SharedHostEventEmitter};
use crate::facade::clipboard_inbound::{
    InboundClipboardApplyInput, InboundClipboardApplyOutcome, InboundClipboardApplyPort,
};
use crate::facade::clipboard_outbound::OutboundBlobPublishGateway;
use crate::facade::host_event::{ClipboardHostEvent, ClipboardOriginKind, HostEvent};
use crate::usecases::clipboard_sync::active_state::apply_inbound::{
    ActiveClipboardConvergedEvent, ApplyInboundActiveClipboardStateUseCase,
    InboundPulledContentStore, InboundPulledContentStoreError,
};
use crate::usecases::clipboard_sync::active_state::fanout::fan_out_active_state;
use crate::usecases::clipboard_sync::active_state::peer_online_resync_worker::PeerOnlineResyncWorker;
use crate::usecases::clipboard_sync::active_state::restore_broadcast_worker::RestoreBroadcastWorker;
use crate::usecases::clipboard_sync::active_state::serve_pull::{
    ActiveClipboardPullServeDeps, ActiveClipboardPullServeUseCase,
};
use crate::usecases::clipboard_sync::send_gate::MemberSendGate;
use crate::usecases::clipboard_sync::snapshot_from_entry::SnapshotReconstructor;

/// The six repository / resolver ports needed to rebuild a
/// `SystemClipboardSnapshot` from a local entry id. Bundled so callers wire one
/// dependency instead of threading six identical ports; folded into a
/// `SnapshotReconstructor` at facade construction. Shared by the
/// inbound / resend / restore paths ([`ActiveClipboardDeps`]) and the pull
/// serve path ([`ActiveClipboardPullServeFacadeDeps`]).
pub struct ClipboardSnapshotDeps {
    pub entry_repo: Arc<dyn GetClipboardEntryPort>,
    pub selection_repo: Arc<dyn ClipboardSelectionRepositoryPort>,
    pub representation_repo: Arc<dyn GetRepresentationPort>,
    pub rep_processing_repo: Arc<dyn UpdateRepresentationProcessingResultPort>,
    pub payload_resolver: Arc<dyn ClipboardPayloadResolverPort>,
    pub blob_store: Arc<dyn BlobReaderPort>,
}

impl ClipboardSnapshotDeps {
    /// Fold the bundled ports into the shared `SnapshotReconstructor`. The free
    /// function `reconstruct_snapshot_from_entry` stays the single source of
    /// truth; this just owns the ports.
    pub(crate) fn into_reconstructor(self) -> SnapshotReconstructor {
        SnapshotReconstructor::new(
            self.entry_repo,
            self.selection_repo,
            self.representation_repo,
            self.rep_processing_repo,
            self.payload_resolver,
            self.blob_store,
        )
    }
}

/// Wiring dependencies for [`ActiveClipboardFacade`]. Assembled by bootstrap.
pub struct ActiveClipboardDeps {
    pub receiver: Arc<dyn ActiveClipboardReceiverPort>,
    pub dispatch: Arc<dyn ActiveClipboardDispatchPort>,
    pub is_unlocked: Arc<dyn IsSpaceUnlockedPort>,
    pub load_register: Arc<dyn LoadActiveClipboardPort>,
    pub advance_register: Arc<dyn AdvanceActiveClipboardPort>,
    pub mobile_consumability: MobileConsumabilityProbe,
    pub member_repo: Arc<dyn MemberRepositoryPort>,
    pub peer_addr_repo: Arc<dyn PeerAddressRepositoryPort>,
    /// Presence stream for the peer-online resync worker: an "online"
    /// transition triggers a resend of the current register to that peer.
    pub presence: Arc<dyn PresencePort>,
    pub entry_lookup: Arc<dyn FindEntryIdBySnapshotHashPort>,
    /// Live availability query. When set, a hash match against a partial entry
    /// is pulled and completed before converging instead of writing its
    /// `uniclip-missing://` placeholder to the OS clipboard. `None` keeps the
    /// prior "any hash match converges" behavior.
    pub availability: Option<Arc<dyn CheckEntryAvailabilityPort>>,
    pub coordinator: Arc<ClipboardWriteCoordinator>,
    pub clock: Arc<dyn ClockPort>,
    /// Identity of this device, used to stamp a locally-originated activation
    /// (`activated_by = self`) when announcing a fresh active-clipboard state.
    pub device_identity: Arc<dyn DeviceIdentityPort>,
    /// Settings reader for the restore-broadcast feature gate
    /// (`sync.sync_on_restore`).
    pub settings: Arc<dyn SettingsPort>,
    /// Snapshot reconstruction ports (shared with restore / resend), folded
    /// into a `SnapshotReconstructor` at construction.
    pub snapshot: ClipboardSnapshotDeps,
    // ---- On-demand pull subsystem (issue #1017 PR8) ----
    /// Transfer cipher shared with the bulk sync path. The inbound store side
    /// decrypts a pulled envelope before persisting it.
    pub transfer_cipher: Arc<dyn TransferCipherPort>,
    /// Outbound pull client. `None` when the pull subsystem is unwired (e.g.
    /// the GUI/CLI client paths) — the inbound "content missing" branch then
    /// logs and returns. Paired with `pull_apply`.
    pub pull_client: Option<Arc<dyn ActiveClipboardPullClientPort>>,
    /// Store-only inbound apply path used to persist a pulled envelope. Must
    /// **not** advance the active-clipboard register itself — the inbound
    /// convergence tail owns the register advance (coupled to OS-write
    /// success). Paired with `pull_client`.
    pub pull_apply: Option<Arc<dyn InboundClipboardApplyPort>>,
    /// Resurfaces the converged entry in clipboard history.
    pub touch_entry: Arc<dyn TouchClipboardEntryPort>,
    /// Host event bus for notifying the frontend after a resurface.
    pub host_event_emitter: SharedHostEventEmitter,
    /// Wall clock for stamping the resurface time.
    pub resurface_clock: Arc<dyn ClockPort>,
}

/// Dependencies for the standalone pull serve port
/// ([`build_active_clipboard_pull_serve_port`]). Built separately from the
/// facade because the serve port must be registered on the pull accept handler
/// before the node spawns, whereas the facade (which owns the inbound loop) is
/// assembled after.
pub struct ActiveClipboardPullServeFacadeDeps {
    pub entry_lookup: Arc<dyn FindEntryIdBySnapshotHashPort>,
    pub settings: Arc<dyn SettingsPort>,
    pub transfer_cipher: Arc<dyn TransferCipherPort>,
    /// Blob transfer facade. The serve side publishes large/image reps and
    /// free-standing files into this device's blob store through it, re-issuing
    /// tickets pinned to this device (D3) before encoding the V3 envelope.
    pub blob_publisher: Arc<BlobTransferFacade>,
    /// File-set manifest store — the single source of truth for a file-class
    /// entry's member list on every outbound path (dispatch / resend / pull
    /// serve, issue #1327). The serve side resolves directory entries through
    /// it so pulled payloads carry the same UCDS manifest as dispatched ones.
    pub entry_file_set_repo: Arc<dyn EntryFileSetRepositoryPort>,
    /// Snapshot reconstruction ports (shared with restore / resend), folded
    /// into a `SnapshotReconstructor` at construction.
    pub snapshot: ClipboardSnapshotDeps,
}

/// Build the active-clipboard pull serve port (issue #1017 PR8). Reuses the
/// resend crypto chain (reconstruct → publish blobs (re-issues self-pinned
/// tickets, D3) → encode V3 → encrypt with a fresh transfer identity, D4).
///
/// Standalone (not a facade method) so bootstrap can register it on the pull
/// accept handler before the node spawns.
pub fn build_active_clipboard_pull_serve_port(
    deps: ActiveClipboardPullServeFacadeDeps,
) -> Arc<dyn ActiveClipboardPullServePort> {
    let reconstructor = deps.snapshot.into_reconstructor();
    let blob_publisher: Arc<dyn OutboundBlobPublishGateway> = deps.blob_publisher;
    Arc::new(ActiveClipboardPullServeUseCase::new(
        ActiveClipboardPullServeDeps {
            entry_lookup: deps.entry_lookup,
            reconstructor,
            settings: deps.settings,
            blob_publisher,
            entry_file_set_repo: deps.entry_file_set_repo,
            cipher: deps.transfer_cipher,
        },
    ))
}

/// Thin facade over the inbound active-clipboard state use case plus the
/// outbound origination workers — restore broadcast and peer-online resync
/// (issue #1017).
pub struct ActiveClipboardFacade {
    inbound_uc: Arc<ApplyInboundActiveClipboardStateUseCase>,
    dispatch: Arc<dyn ActiveClipboardDispatchPort>,
    peer_addr_repo: Arc<dyn PeerAddressRepositoryPort>,
    member_repo: Arc<dyn MemberRepositoryPort>,
    settings: Arc<dyn SettingsPort>,
    presence: Arc<dyn PresencePort>,
    load_register: Arc<dyn LoadActiveClipboardPort>,
    reconstructor: SnapshotReconstructor,
    local_advancer: LocalActiveRegisterAdvancer,
    send_gate: MemberSendGate,
    // Resurface deps — used by the converged-event subscriber worker.
    touch_entry: Arc<dyn TouchClipboardEntryPort>,
    host_event_emitter: SharedHostEventEmitter,
    resurface_clock: Arc<dyn ClockPort>,
}

impl ActiveClipboardFacade {
    pub fn new(deps: ActiveClipboardDeps) -> Self {
        let reconstructor = deps.snapshot.into_reconstructor();
        let mobile_consumability = deps.mobile_consumability;
        let local_advancer = LocalActiveRegisterAdvancer::new(
            Arc::clone(&deps.advance_register),
            deps.device_identity,
            Arc::clone(&deps.clock),
            mobile_consumability.clone(),
        );
        let send_gate = MemberSendGate::new(Arc::clone(&deps.member_repo));

        let (converged_tx, _) = broadcast::channel::<ActiveClipboardConvergedEvent>(16);

        let mut inbound_uc = ApplyInboundActiveClipboardStateUseCase::new(
            deps.receiver,
            deps.is_unlocked,
            Arc::clone(&deps.load_register),
            deps.advance_register,
            Arc::clone(&deps.member_repo),
            deps.entry_lookup,
            reconstructor.clone(),
            deps.coordinator,
            Arc::clone(&deps.dispatch),
            Arc::clone(&deps.peer_addr_repo),
            Arc::clone(&deps.presence),
            deps.clock,
            mobile_consumability,
            converged_tx,
        );

        match (&deps.pull_client, &deps.pull_apply) {
            (Some(_), None) | (None, Some(_)) => {
                warn!("active clipboard: partial pull dependency — both pull_client and pull_apply must be provided together; pull disabled");
            }
            _ => {}
        }
        if let (Some(pull_client), Some(pull_apply)) = (deps.pull_client, deps.pull_apply) {
            let store: Arc<dyn InboundPulledContentStore> = Arc::new(PulledContentStore {
                cipher: Arc::clone(&deps.transfer_cipher),
                apply: pull_apply,
            });
            inbound_uc = inbound_uc.with_pull(pull_client, store);
        }
        if let Some(availability) = deps.availability {
            inbound_uc = inbound_uc.with_check_entry_availability(availability);
        }
        let inbound_uc = Arc::new(inbound_uc);

        Self {
            inbound_uc,
            dispatch: deps.dispatch,
            peer_addr_repo: deps.peer_addr_repo,
            member_repo: deps.member_repo,
            settings: deps.settings,
            presence: deps.presence,
            load_register: deps.load_register,
            reconstructor,
            local_advancer,
            send_gate,
            touch_entry: deps.touch_entry,
            host_event_emitter: deps.host_event_emitter,
            resurface_clock: deps.resurface_clock,
        }
    }

    /// Return the current converged clipboard activation, if one exists.
    pub async fn current(
        &self,
    ) -> Result<Option<ActiveClipboardState>, uc_core::ports::clipboard::ActiveClipboardRegisterError>
    {
        self.load_register.load().await
    }

    /// Announce a locally-originated activation of this device's clipboard
    /// (issue #1017 D1 call-sites 3 & 4, D2 "Mobile push → fan-out").
    ///
    /// Stamps a fresh activation `(now, this_device)` for `snapshot_hash` (held
    /// locally as `entry_id`), advances the cross-device register, then fans
    /// the converged 0xC3 state out to every send-allowed peer through the
    /// shared fan-out. The outbound gate is the full per-device send gate
    /// (`send_enabled` ∧ `send_content_types`, the latter via `categories`) —
    /// **not** `sync_on_restore`, which gates only history-restore broadcasts.
    ///
    /// Best-effort and fire-and-forget at the call site: a register storage
    /// hiccup is logged and swallowed by the advancer, and per-peer dispatch
    /// failures are isolated by the fan-out.
    pub async fn announce_local_activation(
        &self,
        snapshot_hash: String,
        entry_id: EntryId,
        categories: ClipboardContentCategorySet,
    ) {
        let state = self
            .local_advancer
            .advance_local(snapshot_hash, entry_id)
            .await;
        fan_out_active_state(
            &self.dispatch,
            &self.peer_addr_repo,
            &self.presence,
            &self.send_gate,
            &state,
            &categories,
        )
        .await;
    }

    /// Start and own every Active Clipboard background worker. The returned
    /// lifecycle is the only task-lifetime seam exposed to bootstrap.
    pub fn start_background_workers(self: &Arc<Self>) -> ActiveClipboardLifecycle {
        let (commands, command_rx) = mpsc::unbounded_channel();
        let mut workers = JoinSet::new();

        // Subscribe before the inbound loop is scheduled so its first
        // convergence event cannot be missed by the history-resurface worker.
        let resurface_rx = self.inbound_uc.subscribe_converged();
        let resurface_facade = Arc::clone(self);
        workers.spawn(async move {
            resurface_facade.run_resurface_worker(resurface_rx).await;
            "resurface"
        });

        let inbound_uc = Arc::clone(&self.inbound_uc);
        workers.spawn(async move {
            inbound_uc.run().await;
            "inbound"
        });

        let resync = self.peer_online_resync_worker();
        workers.spawn(async move {
            resync.run().await;
            "peer_online_resync"
        });

        let restore_facade = Arc::clone(self);
        let restore_worker_starter: RestoreWorkerStarter = Arc::new(move |rx| {
            let worker = restore_facade.restore_broadcast_worker(rx);
            Box::pin(async move { worker.run().await })
        });

        ActiveClipboardLifecycle::start(workers, restore_worker_starter, commands, command_rx)
    }

    fn peer_online_resync_worker(&self) -> PeerOnlineResyncWorker {
        PeerOnlineResyncWorker::new(
            Arc::clone(&self.presence),
            Arc::clone(&self.load_register),
            self.reconstructor.clone(),
            Arc::clone(&self.dispatch),
            Arc::clone(&self.member_repo),
        )
    }

    fn restore_broadcast_worker(
        &self,
        rx: UnboundedReceiver<RestoreBroadcastRequest>,
    ) -> RestoreBroadcastWorker {
        RestoreBroadcastWorker::new(
            rx,
            Arc::clone(&self.settings),
            Arc::clone(&self.dispatch),
            Arc::clone(&self.peer_addr_repo),
            Arc::clone(&self.presence),
            Arc::clone(&self.member_repo),
        )
    }

    async fn run_resurface_worker(
        self: Arc<Self>,
        mut rx: broadcast::Receiver<ActiveClipboardConvergedEvent>,
    ) {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    resurface_entry(
                        self.touch_entry.as_ref(),
                        &self.host_event_emitter,
                        self.resurface_clock.as_ref(),
                        &event.entry_id,
                    )
                    .await;
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    debug!(
                        missed = n,
                        "resurface worker lagged; some entries may not resurface immediately"
                    );
                }
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    }
}

/// Owns the Active Clipboard worker topology and its coordinated teardown.
pub struct ActiveClipboardLifecycle {
    commands: mpsc::UnboundedSender<ActiveClipboardLifecycleCommand>,
    restore_broadcast_attached: AtomicBool,
    supervisor: Option<JoinHandle<()>>,
}

impl ActiveClipboardLifecycle {
    fn start(
        workers: JoinSet<&'static str>,
        restore_worker_starter: RestoreWorkerStarter,
        commands: mpsc::UnboundedSender<ActiveClipboardLifecycleCommand>,
        command_rx: mpsc::UnboundedReceiver<ActiveClipboardLifecycleCommand>,
    ) -> Self {
        let supervisor = tokio::spawn(async move {
            ActiveClipboardWorkerSupervisor::new(restore_worker_starter, command_rx)
                .run(workers)
                .await;
        });

        Self {
            commands,
            restore_broadcast_attached: AtomicBool::new(false),
            supervisor: Some(supervisor),
        }
    }

    /// Attach the restore-broadcast source after initial assembly. Exactly one
    /// source may be attached for a lifecycle instance.
    pub fn attach_restore_broadcast(
        &self,
        rx: UnboundedReceiver<RestoreBroadcastRequest>,
    ) -> Result<(), ActiveClipboardLifecycleError> {
        if self
            .restore_broadcast_attached
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(ActiveClipboardLifecycleError::RestoreBroadcastAlreadyAttached);
        }

        if self
            .commands
            .send(ActiveClipboardLifecycleCommand::AttachRestoreBroadcast { rx })
            .is_err()
        {
            self.restore_broadcast_attached
                .store(false, Ordering::Release);
            return Err(ActiveClipboardLifecycleError::Stopped);
        }

        Ok(())
    }

    /// Stop all workers, wait for them to finish, and surface unexpected task
    /// failures through tracing before returning.
    pub async fn shutdown(mut self) {
        let (completed, response) = oneshot::channel();
        if self
            .commands
            .send(ActiveClipboardLifecycleCommand::Shutdown { completed })
            .is_ok()
        {
            let _ = response.await;
        }

        if let Some(supervisor) = self.supervisor.take() {
            if let Err(error) = supervisor.await {
                if !error.is_cancelled() {
                    warn!(error = %error, "active clipboard worker supervisor failed");
                }
            }
        }
    }
}

impl Drop for ActiveClipboardLifecycle {
    fn drop(&mut self) {
        if let Some(supervisor) = &self.supervisor {
            supervisor.abort();
        }
    }
}

/// Lifecycle command errors exposed to bootstrap.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ActiveClipboardLifecycleError {
    #[error("active clipboard background workers have stopped")]
    Stopped,
    #[error("the restore broadcast source is already attached")]
    RestoreBroadcastAlreadyAttached,
}

type RestoreWorkerFuture = Pin<Box<dyn Future<Output = ()> + Send>>;
type RestoreWorkerStarter =
    Arc<dyn Fn(UnboundedReceiver<RestoreBroadcastRequest>) -> RestoreWorkerFuture + Send + Sync>;

enum ActiveClipboardLifecycleCommand {
    AttachRestoreBroadcast {
        rx: UnboundedReceiver<RestoreBroadcastRequest>,
    },
    Shutdown {
        completed: oneshot::Sender<()>,
    },
}

struct ActiveClipboardWorkerSupervisor {
    restore_worker_starter: RestoreWorkerStarter,
    commands: mpsc::UnboundedReceiver<ActiveClipboardLifecycleCommand>,
}

impl ActiveClipboardWorkerSupervisor {
    fn new(
        restore_worker_starter: RestoreWorkerStarter,
        commands: mpsc::UnboundedReceiver<ActiveClipboardLifecycleCommand>,
    ) -> Self {
        Self {
            restore_worker_starter,
            commands,
        }
    }

    async fn run(mut self, mut workers: JoinSet<&'static str>) {
        loop {
            tokio::select! {
                command = self.commands.recv() => match command {
                    Some(ActiveClipboardLifecycleCommand::AttachRestoreBroadcast { rx }) => {
                        let worker = (self.restore_worker_starter)(rx);
                        workers.spawn(async move {
                            worker.await;
                            "restore_broadcast"
                        });
                    }
                    Some(ActiveClipboardLifecycleCommand::Shutdown { completed }) => {
                        Self::stop_workers(&mut workers).await;
                        let _ = completed.send(());
                        return;
                    }
                    None => {
                        Self::stop_workers(&mut workers).await;
                        return;
                    }
                },
                joined = workers.join_next(), if !workers.is_empty() => {
                    if let Some(joined) = joined {
                        Self::observe_worker_completion(joined);
                    }
                }
            }
        }
    }

    async fn stop_workers(workers: &mut JoinSet<&'static str>) {
        workers.abort_all();
        while let Some(joined) = workers.join_next().await {
            Self::observe_worker_completion(joined);
        }
    }

    fn observe_worker_completion(joined: Result<&'static str, tokio::task::JoinError>) {
        match joined {
            Ok(worker) => debug!(worker, "active clipboard worker stopped"),
            Err(error) if error.is_cancelled() => debug!("active clipboard worker cancelled"),
            Err(error) => warn!(error = %error, "active clipboard worker failed"),
        }
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use std::time::Duration;

    use tokio::sync::mpsc;
    use tokio::task::JoinSet;

    use super::{ActiveClipboardLifecycle, ActiveClipboardLifecycleError, RestoreWorkerStarter};

    struct StopProbe(Arc<AtomicBool>);

    impl Drop for StopProbe {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn lifecycle_starts_workers_attaches_restore_once_and_joins_on_shutdown() {
        let initial_started = Arc::new(AtomicBool::new(false));
        let initial_stopped = Arc::new(AtomicBool::new(false));
        let restore_started = Arc::new(AtomicBool::new(false));
        let restore_stopped = Arc::new(AtomicBool::new(false));
        let mut workers = JoinSet::new();
        let initial_started_for_task = Arc::clone(&initial_started);
        let initial_stopped_for_task = Arc::clone(&initial_stopped);
        workers.spawn(async move {
            initial_started_for_task.store(true, Ordering::SeqCst);
            let _probe = StopProbe(initial_stopped_for_task);
            std::future::pending::<()>().await;
            "inbound"
        });

        let restore_started_for_factory = Arc::clone(&restore_started);
        let restore_stopped_for_factory = Arc::clone(&restore_stopped);
        let restore_worker_starter: RestoreWorkerStarter = Arc::new(move |_rx| {
            restore_started_for_factory.store(true, Ordering::SeqCst);
            let stopped = Arc::clone(&restore_stopped_for_factory);
            Box::pin(async move {
                let _probe = StopProbe(stopped);
                std::future::pending::<()>().await;
            })
        });
        let (commands, command_rx) = mpsc::unbounded_channel();
        let lifecycle =
            ActiveClipboardLifecycle::start(workers, restore_worker_starter, commands, command_rx);

        wait_for(&initial_started, "lifecycle should start initial workers").await;

        let (_restore_tx, restore_rx) = mpsc::unbounded_channel();
        assert_eq!(lifecycle.attach_restore_broadcast(restore_rx), Ok(()));
        wait_for(
            &restore_started,
            "lifecycle should start a late restore worker",
        )
        .await;

        let (_duplicate_tx, duplicate_rx) = mpsc::unbounded_channel();
        assert_eq!(
            lifecycle.attach_restore_broadcast(duplicate_rx),
            Err(ActiveClipboardLifecycleError::RestoreBroadcastAlreadyAttached)
        );

        assert!(
            tokio::time::timeout(Duration::from_secs(1), lifecycle.shutdown())
                .await
                .is_ok(),
            "lifecycle shutdown should join all workers"
        );
        assert!(initial_stopped.load(Ordering::SeqCst));
        assert!(restore_stopped.load(Ordering::SeqCst));
    }

    async fn wait_for(flag: &AtomicBool, message: &str) {
        let completed = tokio::time::timeout(Duration::from_secs(1), async {
            while !flag.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await;
        assert!(completed.is_ok(), "{message}");
    }
}

#[instrument(name = "active_state.resurface", skip_all, fields(entry_id = %entry_id))]
async fn resurface_entry(
    touch: &dyn TouchClipboardEntryPort,
    bus: &SharedHostEventEmitter,
    clock: &dyn ClockPort,
    entry_id: &EntryId,
) {
    let now_ms = clock.now_ms();
    match touch.touch_entry(entry_id, now_ms).await {
        Ok(true) => {
            debug!("entry resurfaced");
            bus.emit_or_warn(HostEvent::Clipboard(ClipboardHostEvent::NewContent {
                entry_id: entry_id.as_ref().to_string(),
                attempt_id: None,
                preview: "Clipboard restored".to_string(),
                origin: ClipboardOriginKind::Remote,
            }));
        }
        Ok(false) => {
            debug!("touch_entry found no row (entry deleted?)");
        }
        Err(err) => {
            warn!(error = %err, "touch_entry failed (best-effort, ignored)");
        }
    }
}

/// Inbound store half of the pull path (issue #1017 PR8). Decrypts a pulled
/// transfer envelope and persists it through the shared inbound apply path,
/// returning the local entry id. The wrapped apply path must **not** advance
/// the active-clipboard register — the inbound convergence tail owns that.
struct PulledContentStore {
    cipher: Arc<dyn TransferCipherPort>,
    apply: Arc<dyn InboundClipboardApplyPort>,
}

#[async_trait]
impl InboundPulledContentStore for PulledContentStore {
    async fn store(
        &self,
        from_device: &DeviceId,
        snapshot_hash: &str,
        transfer_envelope: Vec<u8>,
    ) -> Result<EntryId, InboundPulledContentStoreError> {
        // Decrypt the transfer envelope into the V3 plaintext the inbound apply
        // path decodes. A locked session (between the pull and the store) or a
        // tampered envelope surfaces here.
        let plaintext = self
            .cipher
            .decrypt(&transfer_envelope)
            .await
            .map_err(|err| InboundPulledContentStoreError::Decrypt(err.to_string()))?;

        // Persist via the shared inbound apply path (decode V3 → materialize
        // blobs → capture). Reuses the same pipeline the bulk 0xC1 path uses,
        // so the pulled entry's schema matches a normal inbound entry.
        let outcome = self
            .apply
            .apply(InboundClipboardApplyInput {
                from_device: from_device.as_str().to_string(),
                snapshot_hash: snapshot_hash.to_string(),
                plaintext: plaintext.into(),
                flow_id: None,
                // Store-only path: this apply's write port is a no-op (the
                // convergence tail below owns the authoritative OS write), so
                // the intent never reaches the clipboard. `RemotePush` states
                // the truth of where the content came from.
                resurface_intent: ClipboardWriteIntent::RemotePush,
            })
            .await
            .map_err(|err| InboundPulledContentStoreError::Store(err.to_string()))?;

        match outcome {
            InboundClipboardApplyOutcome::Applied { entry_id } => Ok(EntryId::from(entry_id)),
            // A duplicate means the content landed locally between the pull and
            // the store (e.g. the bulk path raced us); the existing entry is
            // exactly what we wanted, so converge on it.
            InboundClipboardApplyOutcome::Resurfaced {
                existing_entry_id, ..
            }
            | InboundClipboardApplyOutcome::DuplicateSkipped {
                existing_entry_id, ..
            } => Ok(EntryId::from(existing_entry_id)),
            InboundClipboardApplyOutcome::DecodeFailed { reason } => {
                warn!(reason, "pulled content store: envelope decode failed");
                Err(InboundPulledContentStoreError::Store(format!(
                    "decode: {reason}"
                )))
            }
        }
    }
}
