//! 文件传输领域对象图装配。
//!
//! Engine 只选择具体 adapter；readiness、receive attempt、materializer、
//! cancellation 与 session lifecycle 的组合顺序由本模块唯一持有。

use std::path::PathBuf;
use std::sync::Arc;

use uc_core::file_transfer::{
    FileTransferEventPublisherPort, FileTransferEventStorePort, OutboundProgressReporterPort,
};
use uc_core::ports::atomic_publish::AtomicPublishPort;
use uc_core::ports::clipboard::{
    AdvanceActiveClipboardPort, CheckEntryAvailabilityPort, FindEntryIdBySnapshotHashPort,
    TouchClipboardEntryPort,
};
use uc_core::ports::hidden_path::MarkHiddenPort;
use uc_core::ports::inbound_file_target::{
    ReserveInboundFileTargetPort, ResolveInboundSaveDirPort,
};
use uc_core::ports::{
    CleanupDirectoryStagingPort, CleanupReceiveArtifactsPort, ClockPort,
    FinalizeProvisionalReceivePort,
};

use crate::clipboard::active::ClipboardSnapshotDeps;
use crate::clipboard::entry_identity::EntryIdentityCoordinator;
use crate::clipboard::inbound::InboundClipboardApplyPort;
use crate::clipboard::sync::apply_inbound::{
    ApplyInboundClipboardUseCase, FileCacheBlobMaterializer, InboundApplyCommonDeps,
    InboundBlobFetcher, InboundCapture, InboundReceiveAttemptDeps, InboundWrite,
    InteractiveReceiveDeps, StoreOnlyPullDeps,
};
use crate::clipboard::write::MobileConsumabilityProbe;
use crate::deps::{DirectoryReceivePorts, FileTransferPorts};
use crate::facade::blob_transfer::{BlobTransferFacade, SharedHostEventEmitter};
use crate::facade::clipboard::ClipboardSyncFacade;
use crate::facade::host_event::{
    FileTransferHostEventPublisher, HostEventBus, OutboundEntryIdCache,
};
use crate::search::live_index::ClipboardLiveIndexPort;
use crate::transfer::file::facade::{FileTransferFacade, FileTransferFacadeDeps};
use crate::transfer::file::lifecycle::FileTransferLifecycleDeps;
use crate::transfer::receive::reconciliation::ReceiveReadinessCoordinator;

/// Engine 选择的文件系统与持久化 adapter。
pub struct FileTransferAssemblyDeps {
    pub event_store: Arc<dyn FileTransferEventStorePort>,
    pub host_event_bus: Arc<HostEventBus>,
    pub file_transfer: FileTransferPorts,
    pub directory_receive: DirectoryReceivePorts,
    pub clock: Arc<dyn ClockPort>,
    pub artifact_cleanup: Arc<dyn CleanupReceiveArtifactsPort>,
    pub save_dir_resolver: Arc<dyn ResolveInboundSaveDirPort>,
    pub file_cache_dir: PathBuf,
}

/// 每个 inbound 模式选择的实际传输与文件系统 adapter。
pub struct InboundMaterializerDeps {
    pub fetcher: Arc<dyn InboundBlobFetcher>,
    pub publisher: Arc<dyn AtomicPublishPort>,
    pub target_reserver: Arc<dyn ReserveInboundFileTargetPort>,
    pub hidden_marker: Arc<dyn MarkHiddenPort>,
}

/// Interactive 与 store-only 共同的领域输入；不含步骤级 receive port。
pub struct InboundReceiveIntentDeps {
    pub entry_repo: Arc<dyn FindEntryIdBySnapshotHashPort>,
    pub capture: Arc<dyn InboundCapture>,
    pub materializer: InboundMaterializerDeps,
    pub host_event_emitter: SharedHostEventEmitter,
    pub search_live_index: Arc<dyn ClipboardLiveIndexPort>,
    pub availability: Arc<dyn CheckEntryAvailabilityPort>,
    pub entry_identity_coordinator: Arc<EntryIdentityCoordinator>,
}

pub struct InteractiveReceiveIntentDeps {
    pub common: InboundReceiveIntentDeps,
    pub write: Arc<dyn InboundWrite>,
    pub provisional_receive: Arc<dyn FinalizeProvisionalReceivePort>,
    pub outbound_progress_reporter: Arc<dyn OutboundProgressReporterPort>,
    pub active_register: Arc<dyn AdvanceActiveClipboardPort>,
    pub mobile_consumability: MobileConsumabilityProbe,
    pub snapshot_deps: ClipboardSnapshotDeps,
    pub touch_entry: Arc<dyn TouchClipboardEntryPort>,
}

pub struct StoreOnlyPullIntentDeps {
    pub common: InboundReceiveIntentDeps,
}

/// Cancel intent 仍由 Engine 选择网络与文件系统 adapter，但步骤顺序不外泄。
pub struct ReceiveCancellationDeps {
    pub staging_cleanup: Arc<dyn CleanupDirectoryStagingPort>,
    pub blob_transfer: Arc<BlobTransferFacade>,
}

/// 文件传输领域唯一对象图 owner。
#[derive(Clone)]
pub struct FileTransferAssembly {
    facade: Arc<FileTransferFacade>,
    directory_receive: DirectoryReceivePorts,
    file_transfer: FileTransferPorts,
    clock: Arc<dyn ClockPort>,
    artifact_cleanup: Arc<dyn CleanupReceiveArtifactsPort>,
    save_dir_resolver: Arc<dyn ResolveInboundSaveDirPort>,
    file_cache_dir: PathBuf,
    receive_readiness: Arc<ReceiveReadinessCoordinator>,
}

impl FileTransferAssembly {
    pub fn build(deps: FileTransferAssemblyDeps) -> Self {
        let receive_readiness = Arc::new(ReceiveReadinessCoordinator::new());
        let publisher = Arc::new(FileTransferHostEventPublisher::new(
            Arc::clone(&deps.host_event_bus),
            Arc::clone(&deps.file_transfer.find_entry_id),
            Arc::clone(&deps.file_transfer.find_attempt_id),
            Arc::new(OutboundEntryIdCache::new()),
        ));
        let facade = Arc::new(FileTransferFacade::new(FileTransferFacadeDeps {
            store: deps.event_store,
            publisher: publisher as Arc<dyn FileTransferEventPublisherPort>,
            repo: Arc::clone(&deps.file_transfer.record),
            provisional_seed: Arc::clone(&deps.file_transfer.seed_provisional),
            provisional_path: Arc::clone(&deps.file_transfer.update_provisional_path),
            provisional_finalize: Arc::clone(&deps.file_transfer.finalize_provisional),
            clock: Arc::clone(&deps.clock),
            lifecycle: FileTransferLifecycleDeps {
                list_expired: Arc::clone(&deps.file_transfer.list_expired),
                fail_inflight: Arc::clone(&deps.file_transfer.fail_inflight),
                get_receive_attempt: Arc::clone(&deps.directory_receive.get_attempt),
                list_receive_attempts: Arc::clone(&deps.directory_receive.list_attempts),
                list_unsettled_artifacts: Arc::clone(
                    &deps.directory_receive.list_unsettled_artifacts,
                ),
                get_directory_publish: Arc::clone(&deps.directory_receive.get_publish),
                begin_receive_failure: Arc::clone(&deps.directory_receive.begin_failure),
                cleanup_artifacts: Arc::clone(&deps.artifact_cleanup),
                commit_inbound: Arc::clone(&deps.directory_receive.commit_inbound),
                list_provisional: Arc::clone(&deps.file_transfer.list_provisional),
                finalize_provisional: Arc::clone(&deps.file_transfer.finalize_provisional),
                privacy_maintenance: Arc::clone(&deps.file_transfer.privacy_maintenance),
                save_dir_resolver: Arc::clone(&deps.save_dir_resolver),
                file_cache_dir: deps.file_cache_dir.clone(),
                clock: Arc::clone(&deps.clock),
                host_event_bus: deps.host_event_bus,
                receive_readiness: Arc::clone(&receive_readiness),
            },
        }));

        Self {
            facade,
            directory_receive: deps.directory_receive,
            file_transfer: deps.file_transfer,
            clock: deps.clock,
            artifact_cleanup: deps.artifact_cleanup,
            save_dir_resolver: deps.save_dir_resolver,
            file_cache_dir: deps.file_cache_dir,
            receive_readiness,
        }
    }

    pub fn facade(&self) -> Arc<FileTransferFacade> {
        Arc::clone(&self.facade)
    }

    pub fn interactive_receive(
        &self,
        deps: InteractiveReceiveIntentDeps,
    ) -> Arc<dyn InboundClipboardApplyPort> {
        Arc::new(ApplyInboundClipboardUseCase::interactive_receive(
            InteractiveReceiveDeps {
                common: self.common_receive_deps(deps.common),
                write: deps.write,
                provisional_receive: deps.provisional_receive,
                outbound_progress_reporter: deps.outbound_progress_reporter,
                active_register: deps.active_register,
                mobile_consumability: deps.mobile_consumability,
                snapshot_deps: deps.snapshot_deps,
                touch_entry: deps.touch_entry,
            },
        ))
    }

    pub fn store_only_pull(
        &self,
        deps: StoreOnlyPullIntentDeps,
    ) -> Arc<dyn InboundClipboardApplyPort> {
        Arc::new(ApplyInboundClipboardUseCase::store_only_pull(
            StoreOnlyPullDeps {
                common: self.common_receive_deps(deps.common),
            },
        ))
    }

    pub fn with_receive_cancellation(
        &self,
        facade: ClipboardSyncFacade,
        deps: ReceiveCancellationDeps,
    ) -> ClipboardSyncFacade {
        facade.with_entry_receive_cancellation(
            Arc::clone(&self.directory_receive.get_attempt),
            Arc::clone(&self.directory_receive.request_cancel),
            Arc::clone(&self.directory_receive.entry_progress),
            Arc::clone(&self.directory_receive.list_attempts),
            Arc::clone(&self.directory_receive.commit_inbound),
            Arc::clone(&self.directory_receive.get_publish),
            deps.staging_cleanup,
            Arc::clone(&self.file_transfer.cancel_attempt),
            deps.blob_transfer,
            Arc::clone(&self.clock),
        )
    }

    fn common_receive_deps(&self, deps: InboundReceiveIntentDeps) -> InboundApplyCommonDeps {
        let materializer = Arc::new(
            FileCacheBlobMaterializer::new(
                deps.materializer.fetcher,
                self.file_cache_dir.clone(),
                deps.materializer.publisher,
            )
            .with_directory_receive_attempt_ports(
                Arc::clone(&self.directory_receive.get_attempt),
                Arc::clone(&self.directory_receive.claim_commit),
                Arc::clone(&self.directory_receive.record_publish),
                Arc::clone(&self.clock),
            )
            .with_receive_artifact_log(Arc::clone(&self.directory_receive.record_artifacts))
            .with_target_reserver(deps.materializer.target_reserver)
            .with_save_dir_resolver(Arc::clone(&self.save_dir_resolver))
            .with_hidden_marker(deps.materializer.hidden_marker),
        );

        InboundApplyCommonDeps {
            entry_repo: deps.entry_repo,
            capture: deps.capture,
            blob_materializer: materializer,
            receive_attempts: InboundReceiveAttemptDeps {
                get: Arc::clone(&self.directory_receive.get_attempt),
                begin: Arc::clone(&self.directory_receive.begin_receive),
                claim_commit: Arc::clone(&self.directory_receive.claim_commit),
                request_cancel: Arc::clone(&self.directory_receive.request_cancel),
                begin_failure: Arc::clone(&self.directory_receive.begin_failure),
                commit: Arc::clone(&self.directory_receive.commit_inbound),
                clock: Arc::clone(&self.clock),
            },
            receive_artifact_cleanup: Arc::clone(&self.artifact_cleanup),
            receive_readiness: Arc::clone(&self.receive_readiness),
            host_event_emitter: deps.host_event_emitter,
            search_live_index: deps.search_live_index,
            availability: deps.availability,
            entry_identity_coordinator: deps.entry_identity_coordinator,
        }
    }
}
