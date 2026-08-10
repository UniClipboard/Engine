//! File-transfer lifecycle wiring.
//!
//! Wires the durable event store + host-event publisher + receiver-side
//! projection plumbing into the `FileTransferFacade`; the facade owns the
//! receiver lifecycle (readiness, startup recovery, timeout sweep) and
//! exposes lifecycle actions only (ADR-018 stage 2). This module only
//! assembles dependencies; it no longer constructs the lifecycle itself.

use std::path::PathBuf;
use std::sync::Arc;

use uc_application::deps::{DirectoryReceivePorts, FileTransferPorts};
use uc_application::facade::{
    BlobTransferFacade, FileTransferFacade, FileTransferFacadeDeps, FileTransferHostEventPublisher,
    FileTransferLifecycleDeps, HostEventBus, OutboundEntryIdCache,
};
use uc_core::file_transfer::{FileTransferEventPublisherPort, FileTransferEventStorePort};
use uc_core::ports::inbound_file_target::ResolveInboundSaveDirPort;
use uc_core::ports::EnsureFileTransferPrivacyMaintenancePort;
use uc_core::ports::{ClockPort, FailInflightTransfersPort, ListExpiredInflightTransfersPort};
use uc_infra::db::executor::DieselSqliteExecutor;
use uc_infra::file_transfer::SqliteReceiverFileTransferStore;

pub type FileTransferEventStore = SqliteReceiverFileTransferStore<Arc<DieselSqliteExecutor>>;

/// Assembled file-transfer plumbing returned by
/// [`build_file_transfer_assembly`].
///
/// Hands the composition root the session owner; receive readiness, startup
/// recovery and the timeout sweep stay behind facade lifecycle actions.
pub struct FileTransferAssembly {
    pub facade: Arc<FileTransferFacade>,
}

pub fn build_file_transfer_assembly(
    store: Arc<FileTransferEventStore>,
    host_event_bus: Arc<HostEventBus>,
    file_transfer: FileTransferPorts,
    directory_receive: DirectoryReceivePorts,
    clock: Arc<dyn ClockPort>,
    receive_readiness: Arc<uc_application::facade::ReceiveReadinessCoordinator>,
    save_dir_resolver: Arc<dyn ResolveInboundSaveDirPort>,
    file_cache_dir: PathBuf,
) -> FileTransferAssembly {
    let outbound_entry_cache = Arc::new(OutboundEntryIdCache::new());

    let publisher = Arc::new(FileTransferHostEventPublisher::new(
        Arc::clone(&host_event_bus),
        file_transfer.find_entry_id,
        file_transfer.find_attempt_id,
        Arc::clone(&outbound_entry_cache),
    ));

    let store_port: Arc<dyn FileTransferEventStorePort> = store as _;
    let publisher_port: Arc<dyn FileTransferEventPublisherPort> = Arc::clone(&publisher) as _;

    let facade = Arc::new(FileTransferFacade::new(FileTransferFacadeDeps {
        store: store_port,
        publisher: publisher_port,
        repo: file_transfer.record,
        provisional_seed: file_transfer.seed_provisional,
        provisional_path: file_transfer.update_provisional_path,
        provisional_finalize: file_transfer.finalize_provisional.clone(),
        clock: Arc::clone(&clock),
        lifecycle: FileTransferLifecycleDeps {
            list_expired: file_transfer.list_expired,
            fail_inflight: file_transfer.fail_inflight,
            get_receive_attempt: directory_receive.get_attempt,
            list_receive_attempts: directory_receive.list_attempts,
            list_unsettled_artifacts: directory_receive.list_unsettled_artifacts,
            get_directory_publish: directory_receive.get_publish,
            begin_receive_failure: directory_receive.begin_failure,
            cleanup_artifacts: Arc::new(uc_infra::fs::FsReceiveArtifactCleaner),
            commit_inbound: directory_receive.commit_inbound,
            list_provisional: file_transfer.list_provisional,
            finalize_provisional: file_transfer.finalize_provisional.clone(),
            privacy_maintenance: file_transfer.privacy_maintenance,
            save_dir_resolver,
            file_cache_dir,
            clock,
            host_event_bus,
            receive_readiness,
        },
    }));

    FileTransferAssembly { facade }
}
