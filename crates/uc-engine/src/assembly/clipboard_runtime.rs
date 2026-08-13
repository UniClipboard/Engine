use std::sync::Arc;

use uc_application::facade::clipboard_capture::CaptureClipboardUseCase;
use uc_application::facade::{
    ApplyInboundClipboardUseCase, FileCacheBlobMaterializer, InboundApplyCommonDeps,
    InboundCapture as ApplyInboundCapture, InboundReceiveAttemptDeps,
    InboundWrite as ApplyInboundWrite, InteractiveReceiveDeps,
};
use uc_application::facade::{
    ClipboardCaptureFacade, ClipboardInboundEvent, ClipboardInboundEventAction,
    ClipboardInboundEventPort, ClipboardInboundRuntime, ClipboardInboundRuntimeDeps,
    ClipboardLiveIndexDeps, ClipboardLiveIndexFacade, ClipboardLiveIndexPort, ClipboardLiveIndexer,
    ClipboardOutboundDeps, ClipboardOutboundFacade, ClipboardSyncRuntime, ClipboardSyncRuntimeDeps,
    InboundClipboardApplyPort,
};
use uc_infra::fs::{FsAtomicPublisher, FsHiddenPathMarker, FsInboundFileTarget};

use crate::assembly::deps::WiredDependencies;
use crate::assembly::sync_engine::SyncEngineAssembly;
use crate::engine::event_stream::EventSender;
use crate::{
    EngineEvent, InboundNoticeActionSummary, InboundNoticeEvent, InboundRepresentationSummary,
};

pub(crate) struct ClipboardRuntime {
    pub capture: Arc<ClipboardCaptureFacade>,
    pub live_index: Arc<ClipboardLiveIndexFacade>,
    pub outbound: Arc<ClipboardOutboundFacade>,
    pub sync: Arc<ClipboardSyncRuntime>,
    #[cfg(feature = "lan-compat")]
    pub apply_inbound: Arc<ApplyInboundClipboardUseCase>,
}

pub(crate) fn build_clipboard_runtime(
    wired: &WiredDependencies,
    sync_engine: &SyncEngineAssembly,
    events: EventSender,
) -> ClipboardRuntime {
    let deps = &wired.deps;
    let capture = Arc::new(
        CaptureClipboardUseCase::new(
            deps.clipboard.entry_ports.save.clone(),
            deps.clipboard.entry_ports.touch.clone(),
            deps.clipboard.entry_ports.find_by_snapshot_hash.clone(),
            deps.clipboard.clipboard_event_repo.clone(),
            deps.clipboard.representation_policy.clone(),
            deps.clipboard.representation_normalizer.clone(),
            deps.device.device_identity.clone(),
            deps.clipboard.representation_cache.clone(),
            deps.clipboard.spool_queue.clone(),
            deps.storage.blob_content_ingest.clone(),
            deps.storage.entry_file_set_repo.clone(),
            deps.settings.clone(),
            deps.clipboard.entry_ports.replace_content.clone(),
            deps.analytics.clone(),
        )
        .with_inbound_receive_commit(deps.storage.directory_receive.commit_inbound.clone())
        .with_entry_identity_coordinator(deps.clipboard.entry_identity_coordinator.clone()),
    );
    let search_live_indexer: Arc<dyn ClipboardLiveIndexPort> =
        Arc::new(ClipboardLiveIndexer::new(ClipboardLiveIndexDeps {
            clipboard_entry_repo: deps.clipboard.entry_ports.get.clone(),
            representation_policy: deps.clipboard.representation_policy.clone(),
            search_key_derivation: deps.search.search_key_derivation.clone(),
            search_pipeline: deps.search.search_pipeline.clone(),
            search_index: deps.search.search_index.clone(),
            event_repo: wired.shared.clipboard_event_reader_repo.clone(),
            entry_file_set_repo: deps.storage.entry_file_set_repo.clone(),
        }));
    let outbound = Arc::new(ClipboardOutboundFacade::new(ClipboardOutboundDeps {
        settings: deps.settings.clone(),
        clipboard_sync: sync_engine.clipboard_sync.clone(),
        blob_transfer: sync_engine.blob.clone(),
        entry_repo: deps.clipboard.entry_ports.get.clone(),
        event_repo: wired.shared.clipboard_event_reader_repo.clone(),
        selection_repo: deps.clipboard.selection_repo.clone(),
        representation_repo: deps.clipboard.representation_ports.get.clone(),
        rep_processing_repo: deps
            .clipboard
            .representation_ports
            .update_processing_result
            .clone(),
        payload_resolver: deps.clipboard.payload_resolver.clone(),
        blob_store: deps.storage.blob_store.clone(),
        entry_delivery_repo: wired.shared.entry_delivery_repo.clone(),
        trusted_peer_repo: wired.shared.trusted_peer_repo.clone(),
        peer_scope: sync_engine.current_peer_scope(),
        device_identity: deps.device.device_identity.clone(),
        entry_file_set_repo: deps.storage.entry_file_set_repo.clone(),
    }));
    let blob_materializer = Arc::new(
        FileCacheBlobMaterializer::new(
            sync_engine.blob.clone(),
            wired.shared.file_cache_dir.clone(),
            FsAtomicPublisher::new(),
        )
        .with_directory_receive_attempt_ports(
            deps.storage.directory_receive.get_attempt.clone(),
            deps.storage.directory_receive.claim_commit.clone(),
            deps.storage.directory_receive.record_publish.clone(),
            deps.system.clock.clone(),
        )
        .with_receive_artifact_log(deps.storage.directory_receive.record_artifacts.clone())
        .with_target_reserver(FsInboundFileTarget::new(deps.settings.clone()))
        .with_save_dir_resolver(FsInboundFileTarget::new(deps.settings.clone()))
        .with_hidden_marker(FsHiddenPathMarker::new()),
    );
    let apply_inbound = Arc::new(ApplyInboundClipboardUseCase::interactive_receive(
        InteractiveReceiveDeps {
            common: InboundApplyCommonDeps {
                entry_repo: deps.clipboard.entry_ports.find_by_snapshot_hash.clone(),
                capture: Arc::clone(&capture) as Arc<dyn ApplyInboundCapture>,
                blob_materializer,
                receive_attempts: InboundReceiveAttemptDeps {
                    get: deps.storage.directory_receive.get_attempt.clone(),
                    begin: deps.storage.directory_receive.begin_receive.clone(),
                    claim_commit: deps.storage.directory_receive.claim_commit.clone(),
                    request_cancel: deps.storage.directory_receive.request_cancel.clone(),
                    begin_failure: deps.storage.directory_receive.begin_failure.clone(),
                    commit: deps.storage.directory_receive.commit_inbound.clone(),
                    clock: deps.system.clock.clone(),
                },
                receive_artifact_cleanup: Arc::new(uc_infra::fs::FsReceiveArtifactCleaner),
                receive_readiness: wired.shared.receive_readiness.clone(),
                host_event_emitter: wired.shared.host_event_bus.clone(),
                search_live_index: Arc::clone(&search_live_indexer),
                availability: deps.clipboard.entry_ports.availability.clone(),
                entry_identity_coordinator: deps.clipboard.entry_identity_coordinator.clone(),
            },
            write: Arc::clone(&wired.shared.clipboard_write_coordinator)
                as Arc<dyn ApplyInboundWrite>,
            provisional_receive: deps.storage.file_transfer.finalize_provisional.clone(),
            outbound_progress_reporter: Arc::clone(&sync_engine.outbound_progress_reporter),
            active_register: deps.clipboard.active_register.clone(),
            mobile_consumability: deps.clipboard.mobile_consumability.clone(),
            snapshot_deps: uc_application::facade::ClipboardSnapshotDeps {
                entry_repo: deps.clipboard.entry_ports.get.clone(),
                selection_repo: deps.clipboard.selection_repo.clone(),
                representation_repo: deps.clipboard.representation_ports.get.clone(),
                rep_processing_repo: deps
                    .clipboard
                    .representation_ports
                    .update_processing_result
                    .clone(),
                payload_resolver: deps.clipboard.payload_resolver.clone(),
                blob_store: deps.storage.blob_store.clone(),
            },
            touch_entry: deps.clipboard.entry_ports.touch.clone(),
        },
    ));
    let inbound_runtime = ClipboardInboundRuntime::start(ClipboardInboundRuntimeDeps {
        receiver: sync_engine.clipboard_receiver(),
        member_repo: deps.device.member_repo.clone(),
        content_gate: sync_engine.convergence_content_gate(),
        transfer_cipher: deps.security.transfer_cipher.clone(),
        settings: deps.settings.clone(),
        clock: deps.system.clock.clone(),
        apply: apply_inbound.clone() as Arc<dyn InboundClipboardApplyPort>,
        events: Arc::new(EngineClipboardInboundEvents { events }),
    });
    let sync = Arc::new(ClipboardSyncRuntime::start(ClipboardSyncRuntimeDeps {
        outbound: Arc::clone(&outbound),
        settings: deps.settings.clone(),
        inbound: inbound_runtime,
        presence: Arc::clone(&sync_engine.presence),
        known_peers: wired.sync_engine.peer_addr_repo.clone(),
        entries: deps.clipboard.entry_ports.list.clone(),
        events: wired.shared.clipboard_event_reader_repo.clone(),
        deliveries: wired.shared.entry_delivery_repo.clone(),
        device_identity: deps.device.device_identity.clone(),
        clock: deps.system.clock.clone(),
    }));

    ClipboardRuntime {
        capture: Arc::new(ClipboardCaptureFacade::new(
            capture,
            deps.clipboard.clipboard.clone(),
        )),
        live_index: Arc::new(ClipboardLiveIndexFacade::new(search_live_indexer)),
        outbound,
        sync,
        #[cfg(feature = "lan-compat")]
        apply_inbound,
    }
}

impl ClipboardRuntime {
    pub(crate) async fn shutdown(self) {
        self.sync.shutdown().await;
    }
}

struct EngineClipboardInboundEvents {
    events: EventSender,
}

impl ClipboardInboundEventPort for EngineClipboardInboundEvents {
    fn emit(&self, event: ClipboardInboundEvent) {
        self.events
            .send(EngineEvent::InboundNotice(InboundNoticeEvent {
                from_device: event.from_device.as_str().to_owned(),
                snapshot_hash: event.snapshot_hash,
                text_preview: event.text_preview,
                representations: event
                    .representations
                    .into_iter()
                    .map(|representation| InboundRepresentationSummary {
                        mime_type: representation.mime_type,
                        size_bytes: representation.size_bytes,
                    })
                    .collect(),
                action: match event.action {
                    ClipboardInboundEventAction::NewEntry => InboundNoticeActionSummary::NewEntry,
                    ClipboardInboundEventAction::DuplicateIgnored => {
                        InboundNoticeActionSummary::DuplicateIgnored
                    }
                },
                at_ms: event.at_ms,
            }));
    }
}
