//! `ActiveClipboardFacade` — application entry point for the cross-device
//! active-clipboard register convergence (issue #1017).
//!
//! Owns the inbound state use case, the background worker topology that drives
//! convergence, and the mobile-push activation announce
//! ([`ActiveClipboardFacade::announce_local_activation`]). Bootstrap crosses a
//! single lifecycle seam for worker startup, late restore-source attachment,
//! and coordinated shutdown.

mod lifecycle;
mod reconcile;

pub use lifecycle::{ActiveClipboardLifecycle, ActiveClipboardLifecycleError};

pub use reconcile::{
    ActiveClipboardReconcileDeps, ActiveClipboardReconcileError, ActiveClipboardReconcileFacade,
    ActiveClipboardReconcileOutcome,
};

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::broadcast;
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
use uc_core::ports::{
    ClockPort, DeviceIdentityPort, PeerAddressRepositoryPort, PeerReachabilityPort, SettingsPort,
};
use uc_core::{blob::ports::BlobReaderPort, MemberRepositoryPort};

use crate::deps::CurrentSpaceMemberScopePort;

use crate::space::IsSpaceUnlockedPort;

use crate::clipboard::inbound::{
    InboundClipboardApplyInput, InboundClipboardApplyOutcome, InboundClipboardApplyPort,
};
use crate::clipboard::outbound::OutboundBlobPublishGateway;
use crate::clipboard::sync::active_state::apply_inbound::{
    ActiveClipboardConvergedEvent, ApplyInboundActiveClipboardStateUseCase,
    InboundPulledContentStore, InboundPulledContentStoreError, InboundPulledContentStoreOutcome,
};
use crate::clipboard::sync::active_state::fanout::fan_out_active_state;
use crate::clipboard::sync::active_state::serve_pull::{
    ActiveClipboardPullServeDeps, ActiveClipboardPullServeUseCase,
};
use crate::clipboard::sync::payload_codec::decode_v3_bytes_to_snapshot;
use crate::clipboard::sync::receive_gate::MemberReceiveGate;
use crate::clipboard::sync::send_gate::MemberSendGate;
use crate::clipboard::sync::snapshot_from_entry::SnapshotReconstructor;
use crate::clipboard::write::{
    ClipboardWriteCoordinator, ClipboardWriteIntent, LocalActiveRegisterAdvancer,
    MobileConsumabilityProbe,
};
use crate::facade::blob_transfer::{BlobTransferFacade, SharedHostEventEmitter};
use crate::facade::host_event::{ClipboardHostEvent, ClipboardOriginKind, HostEvent};

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
    pub peer_scope: Arc<dyn CurrentSpaceMemberScopePort>,
    /// Presence stream for the peer-online resync worker: an "online"
    /// transition triggers a resend of the current register to that peer.
    pub peer_reachability: Arc<dyn PeerReachabilityPort>,
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
    peer_scope: Arc<dyn CurrentSpaceMemberScopePort>,
    member_repo: Arc<dyn MemberRepositoryPort>,
    settings: Arc<dyn SettingsPort>,
    peer_reachability: Arc<dyn PeerReachabilityPort>,
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
            Arc::clone(&deps.peer_scope),
            Arc::clone(&deps.peer_reachability),
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
                receive_gate: MemberReceiveGate::new(
                    Arc::clone(&deps.member_repo),
                    Arc::clone(&deps.peer_scope),
                ),
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
            peer_scope: deps.peer_scope,
            member_repo: deps.member_repo,
            settings: deps.settings,
            peer_reachability: deps.peer_reachability,
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
            &self.peer_scope,
            &self.peer_reachability,
            &self.send_gate,
            &state,
            &categories,
        )
        .await;
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
    receive_gate: MemberReceiveGate,
    apply: Arc<dyn InboundClipboardApplyPort>,
}

#[async_trait]
impl InboundPulledContentStore for PulledContentStore {
    async fn store(
        &self,
        from_device: &DeviceId,
        snapshot_hash: &str,
        transfer_envelope: Vec<u8>,
        receive_permit: &crate::clipboard::sync::receive_gate::MemberReceivePermit,
    ) -> Result<InboundPulledContentStoreOutcome, InboundPulledContentStoreError> {
        // Decrypt first because categories are inside the encrypted V3
        // envelope. Decode and enforce the receive allowlist before invoking
        // the shared inbound apply path, which can persist entries and blobs.
        let plaintext = self
            .cipher
            .decrypt(&transfer_envelope)
            .await
            .map_err(|err| InboundPulledContentStoreError::Decrypt(err.to_string()))?;
        let snapshot = decode_v3_bytes_to_snapshot(&plaintext)
            .map_err(|err| InboundPulledContentStoreError::Store(format!("decode: {err}")))?;
        let categories = ClipboardContentCategorySet::from_snapshot(&snapshot);
        if !self
            .receive_gate
            .is_receive_category_allowed(receive_permit, &categories)
        {
            return Ok(InboundPulledContentStoreOutcome::RejectedByReceivePolicy);
        }

        // Persist via the shared inbound apply path (decode V3 → materialize
        // blobs → capture). Reuses the same pipeline the bulk 0xC1 path uses,
        // so the pulled entry's schema matches a normal inbound entry.
        let outcome = self
            .apply
            .apply(InboundClipboardApplyInput {
                from_device: from_device.as_str().to_string(),
                snapshot_hash: snapshot_hash.to_string(),
                plaintext: plaintext.into(),
                provisional: None,
                // Store-only path: this apply's write port is a no-op (the
                // convergence tail below owns the authoritative OS write), so
                // the intent never reaches the clipboard. `RemotePush` states
                // the truth of where the content came from.
                resurface_intent: ClipboardWriteIntent::RemotePush,
            })
            .await
            .map_err(|err| InboundPulledContentStoreError::Store(err.to_string()))?;

        match outcome {
            InboundClipboardApplyOutcome::Applied { entry_id } => Ok(
                InboundPulledContentStoreOutcome::Stored(EntryId::from(entry_id)),
            ),
            // A duplicate means the content landed locally between the pull and
            // the store (e.g. the bulk path raced us); the existing entry is
            // exactly what we wanted, so converge on it.
            InboundClipboardApplyOutcome::Resurfaced {
                existing_entry_id, ..
            }
            | InboundClipboardApplyOutcome::DuplicateSkipped {
                existing_entry_id, ..
            } => Ok(InboundPulledContentStoreOutcome::Stored(EntryId::from(
                existing_entry_id,
            ))),
            InboundClipboardApplyOutcome::DecodeFailed { reason } => {
                warn!(reason, "pulled content store: envelope decode failed");
                Err(InboundPulledContentStoreError::Store(format!(
                    "decode: {reason}"
                )))
            }
        }
    }
}

#[cfg(test)]
mod pull_store_tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use chrono::Utc;
    use uc_core::clipboard::{MimeType, ObservedClipboardRepresentation, SystemClipboardSnapshot};
    use uc_core::ids::{FormatId, RepresentationId};
    use uc_core::ports::security::{TransferCipherError, TransferCipherPort};
    use uc_core::{MemberRepositoryPort, MemberSyncPreferences, MembershipError, SpaceMember};

    use super::*;
    use crate::clipboard::inbound::InboundClipboardApplyError;
    use crate::clipboard::sync::payload_codec::encode_snapshot_to_v3_bytes;

    struct PlaintextCipher(Vec<u8>);

    #[async_trait]
    impl TransferCipherPort for PlaintextCipher {
        async fn encrypt(&self, _plaintext: &[u8]) -> Result<Vec<u8>, TransferCipherError> {
            unreachable!("pull store only decrypts")
        }

        async fn decrypt(&self, _encrypted: &[u8]) -> Result<Vec<u8>, TransferCipherError> {
            Ok(self.0.clone())
        }
    }

    struct TextDeniedMemberRepo;

    #[async_trait]
    impl MemberRepositoryPort for TextDeniedMemberRepo {
        async fn get(&self, device_id: &DeviceId) -> Result<Option<SpaceMember>, MembershipError> {
            let mut preferences = MemberSyncPreferences::default();
            preferences.receive_content_types.text = false;
            Ok(Some(SpaceMember {
                device_id: device_id.clone(),
                device_name: "peer".to_owned(),
                identity_fingerprint: uc_core::security::IdentityFingerprint::from_raw_string(
                    "0123456789abcdef",
                )
                .expect("valid test fingerprint"),
                joined_at: Utc::now(),
                sync_preferences: preferences,
            }))
        }

        async fn list(&self) -> Result<Vec<SpaceMember>, MembershipError> {
            Ok(Vec::new())
        }

        async fn save(&self, _member: &SpaceMember) -> Result<(), MembershipError> {
            Ok(())
        }

        async fn remove(&self, _device_id: &DeviceId) -> Result<bool, MembershipError> {
            Ok(false)
        }
    }

    struct AllowAllContent;

    #[async_trait]
    impl crate::deps::CurrentSpaceMemberScopePort for AllowAllContent {
        async fn snapshot(
            &self,
        ) -> Result<crate::deps::CurrentSpaceMemberScope, crate::deps::CurrentSpaceMemberScopeError>
        {
            Ok(crate::deps::CurrentSpaceMemberScope {
                revision: 1,
                local_member_active: true,
                usable_peer_device_ids: vec![DeviceId::new("peer-text-disabled")],
                paused_peer_devices: Vec::new(),
            })
        }
    }

    struct ApplyNeverCalled;

    #[async_trait]
    impl InboundClipboardApplyPort for ApplyNeverCalled {
        async fn apply(
            &self,
            _input: InboundClipboardApplyInput,
        ) -> Result<InboundClipboardApplyOutcome, InboundClipboardApplyError> {
            panic!("receive policy must reject before inbound apply")
        }
    }

    #[tokio::test]
    async fn text_rejection_happens_before_pulled_content_is_persisted() {
        let snapshot = SystemClipboardSnapshot {
            ts_ms: 0,
            representations: vec![ObservedClipboardRepresentation::new(
                RepresentationId::new(),
                FormatId::from("text"),
                Some(MimeType("text/plain".to_owned())),
                b"private text".to_vec(),
            )],
            file_content_digests: Vec::new(),
            file_set_v1_component: None,
        };
        let (plaintext, snapshot_hash) =
            encode_snapshot_to_v3_bytes(&snapshot).expect("encode text envelope");
        let receive_gate =
            MemberReceiveGate::new(Arc::new(TextDeniedMemberRepo), Arc::new(AllowAllContent));
        let peer = DeviceId::new("peer-text-disabled");
        let receive_permit = receive_gate
            .authorize(&peer)
            .await
            .expect("device-level receive is allowed");
        let store = PulledContentStore {
            cipher: Arc::new(PlaintextCipher(plaintext.to_vec())),
            receive_gate,
            apply: Arc::new(ApplyNeverCalled),
        };

        let outcome = store
            .store(
                &peer,
                &snapshot_hash,
                b"encrypted-envelope".to_vec(),
                &receive_permit,
            )
            .await
            .expect("policy rejection is not a storage error");

        assert_eq!(
            outcome,
            InboundPulledContentStoreOutcome::RejectedByReceivePolicy
        );
    }
}
