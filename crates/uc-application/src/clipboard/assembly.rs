//! Clipboard 领域对象图装配。
//!
//! 该 owner 创建进程内唯一的内容身份协调器与写入协调器，并集中构造
//! capture、history、restore、resource 与 live-index 入口。Engine 只选择
//! 平台/存储 adapter，不再逐项拼装 Clipboard use case。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use uc_core::clipboard::ClipboardIntegrationMode;
use uc_core::file_transfer::OutboundProgressReporterPort;
use uc_core::ports::atomic_publish::AtomicPublishPort;
use uc_core::ports::blob::BlobTransferPort;
use uc_core::ports::hidden_path::MarkHiddenPort;
use uc_core::ports::inbound_file_target::ReserveInboundFileTargetPort;
use uc_core::ports::{
    ActiveClipboardDispatchPort, ActiveClipboardPullClientPort, ActiveClipboardPullServePort,
    ActiveClipboardReceiverPort, ClipboardReceiverPort, EntryDeliveryRepositoryPort,
    PeerAddressRepositoryPort, PeerReachabilityPort,
};
use uc_core::TaskRegistry;
use uc_core::TrustedPeerRepositoryPort;

use crate::clipboard::active::ClipboardSnapshotDeps;
use crate::clipboard::active::{
    build_active_clipboard_pull_serve_port, ActiveClipboardDeps, ActiveClipboardFacade,
    ActiveClipboardLifecycle, ActiveClipboardLifecycleError, ActiveClipboardPullServeFacadeDeps,
    ActiveClipboardReconcileDeps, ActiveClipboardReconcileError, ActiveClipboardReconcileFacade,
};
use crate::clipboard::capture::CaptureClipboardUseCase;
use crate::clipboard::entry_identity::EntryIdentityCoordinator;
use crate::clipboard::inbound::{
    ClipboardInboundEventPort, ClipboardInboundRuntime, ClipboardInboundRuntimeDeps,
    InboundClipboardApplyPort,
};
use crate::clipboard::outbound::{ClipboardOutboundDeps, ClipboardOutboundFacade};
use crate::clipboard::resource::ResourceFacadeDeps;
use crate::clipboard::sync::apply_inbound::{InboundBlobFetcher, InboundCapture, InboundWrite};
use crate::clipboard::sync::sync_runtime::{ClipboardSyncRuntime, ClipboardSyncRuntimeDeps};
use crate::clipboard::write::{ClipboardWriteCoordinator, RestoreBroadcastTrigger};
use crate::deps::{ApplicationDeps, CurrentSpaceMemberScopePort, IsSpaceUnlockedPort};
use crate::facade::clipboard_history::ClipboardHistoryFacadeDeps;
use crate::facade::clipboard_restore::ClipboardRestoreFacadeDeps;
use crate::facade::{
    BlobTransferFacade, ClipboardCaptureFacade, ClipboardHistoryFacade, ClipboardRestoreFacade,
    ClipboardSyncFacade, HostEventBus, ResourceFacade,
};
use crate::search::live_index::{
    ClipboardLiveIndexDeps, ClipboardLiveIndexFacade, ClipboardLiveIndexPort, ClipboardLiveIndexer,
};
use crate::transfer::file::assembly::FileTransferAssembly;
use crate::transfer::file::assembly::StoreOnlyPullIntentDeps;
use crate::transfer::file::assembly::{
    InboundMaterializerDeps, InboundReceiveIntentDeps, InteractiveReceiveIntentDeps,
};

/// Clipboard 运行所需的跨领域、被动装配输入。
#[derive(Clone)]
pub struct ClipboardAssemblyDeps {
    pub application: ApplicationDeps,
    pub file_cache_dir: PathBuf,
    pub file_transfer: Arc<FileTransferAssembly>,
    pub host_event_bus: Arc<HostEventBus>,
    pub background: Arc<dyn ClipboardBackgroundPort>,
}

#[derive(Debug, thiserror::Error)]
pub enum ClipboardBackgroundError {
    #[error("clipboard background runtime has already started")]
    AlreadyStarted,
    #[error("clipboard spool recovery failed")]
    SpoolRecovery {
        #[source]
        source: anyhow::Error,
    },
}

#[async_trait::async_trait]
pub trait ClipboardBackgroundPort: Send + Sync {
    async fn start(&self, task_registry: Arc<TaskRegistry>)
        -> Result<(), ClipboardBackgroundError>;
}

#[derive(Debug, thiserror::Error)]
pub enum ClipboardBackgroundStartError {
    #[error("active clipboard startup reconciliation failed")]
    Reconcile {
        #[source]
        source: ActiveClipboardReconcileError,
    },
    #[error(transparent)]
    Background(#[from] ClipboardBackgroundError),
}

/// Engine 选择的入站文件系统 adapter；Clipboard 决定它们参与哪种模式。
#[derive(Clone)]
pub(crate) struct ClipboardInboundAdapters {
    pub fetcher: Arc<dyn InboundBlobFetcher>,
    pub publisher: Arc<dyn AtomicPublishPort>,
    pub target_reserver: Arc<dyn ReserveInboundFileTargetPort>,
    pub hidden_marker: Arc<dyn MarkHiddenPort>,
}

/// 单个网络 session 注入 Clipboard 的被动能力。
pub(crate) struct ClipboardSessionDeps {
    pub clipboard_sync: Arc<ClipboardSyncFacade>,
    pub blob_transfer: Arc<BlobTransferFacade>,
    pub receiver: Arc<dyn ClipboardReceiverPort>,
    pub member_scope: Arc<dyn CurrentSpaceMemberScopePort>,
    pub presence: Arc<dyn PeerReachabilityPort>,
    pub known_peers: Arc<dyn PeerAddressRepositoryPort>,
    pub deliveries: Arc<dyn EntryDeliveryRepositoryPort>,
    pub trusted_peers: Arc<dyn TrustedPeerRepositoryPort>,
    pub outbound_progress_reporter: Arc<dyn OutboundProgressReporterPort>,
    pub inbound_adapters: ClipboardInboundAdapters,
    pub inbound_events: Arc<dyn ClipboardInboundEventPort>,
}

/// Active Clipboard session 的网络能力；Application 保持 worker 对象图私有。
pub(crate) struct ActiveClipboardSessionDeps {
    pub receiver: Arc<dyn ActiveClipboardReceiverPort>,
    pub dispatch: Arc<dyn ActiveClipboardDispatchPort>,
    pub is_unlocked: Arc<dyn IsSpaceUnlockedPort>,
    pub peer_addresses: Arc<dyn PeerAddressRepositoryPort>,
    pub member_scope: Arc<dyn CurrentSpaceMemberScopePort>,
    pub peer_reachability: Arc<dyn PeerReachabilityPort>,
    pub pull_client: Arc<dyn ActiveClipboardPullClientPort>,
    pub pull_adapters: ClipboardInboundAdapters,
}

#[derive(Debug, thiserror::Error)]
pub enum ActiveClipboardStartError {
    #[error("clipboard background gate has not completed")]
    BackgroundNotReady,
}

/// Active Clipboard 唯一 session owner；worker lifecycle 不暴露给 Engine。
pub struct ActiveClipboardSession {
    facade: Arc<ActiveClipboardFacade>,
    lifecycle: ActiveClipboardLifecycle,
}

/// Clipboard session 对外稳定入口与唯一关闭句柄。
pub struct ClipboardSession {
    capture: Arc<ClipboardCaptureFacade>,
    live_index: Arc<ClipboardLiveIndexFacade>,
    outbound: Arc<ClipboardOutboundFacade>,
    sync: Arc<ClipboardSyncRuntime>,
    apply_inbound: Arc<dyn InboundClipboardApplyPort>,
}

/// Clipboard 领域唯一对象图 owner。
#[derive(Clone)]
pub struct ClipboardAssembly {
    deps: ApplicationDeps,
    file_cache_dir: PathBuf,
    entry_identity: Arc<EntryIdentityCoordinator>,
    write_coordinator: Arc<ClipboardWriteCoordinator>,
    capture_use_case: Arc<CaptureClipboardUseCase>,
    capture: Arc<ClipboardCaptureFacade>,
    live_index: Arc<dyn ClipboardLiveIndexPort>,
    file_transfer: Arc<FileTransferAssembly>,
    host_event_bus: Arc<HostEventBus>,
    background: Arc<dyn ClipboardBackgroundPort>,
    background_ready: Arc<AtomicBool>,
}

impl ClipboardAssembly {
    pub fn build(deps: ClipboardAssemblyDeps) -> Self {
        let application = deps.application;
        let entry_identity = Arc::new(EntryIdentityCoordinator::new());
        let write_coordinator = Arc::new(ClipboardWriteCoordinator::new(
            Arc::clone(&application.clipboard.system_clipboard),
            Arc::clone(&application.clipboard.clipboard_change_origin),
        ));
        let capture_use_case = Arc::new(
            CaptureClipboardUseCase::new(
                Arc::clone(&application.clipboard.entry_ports.save),
                Arc::clone(&application.clipboard.entry_ports.touch),
                Arc::clone(&application.clipboard.entry_ports.find_by_snapshot_hash),
                Arc::clone(&application.clipboard.clipboard_event_repo),
                Arc::clone(&application.clipboard.representation_policy),
                Arc::clone(&application.clipboard.representation_normalizer),
                Arc::clone(&application.device.device_identity),
                Arc::clone(&application.clipboard.representation_cache),
                Arc::clone(&application.clipboard.spool_queue),
                Arc::clone(&application.storage.blob_content_ingest),
                Arc::clone(&application.storage.entry_file_set_repo),
                Arc::clone(&application.settings),
                Arc::clone(&application.clipboard.entry_ports.replace_content),
                Arc::clone(&application.analytics),
            )
            .with_inbound_receive_commit(Arc::clone(
                &application.storage.directory_receive.commit_inbound,
            ))
            .with_entry_identity_coordinator(Arc::clone(&entry_identity)),
        );
        let capture = Arc::new(
            ClipboardCaptureFacade::new(
                Arc::clone(&capture_use_case) as Arc<_>,
                Arc::clone(&application.clipboard.clipboard),
            )
            .with_entry_file_set_repository(Arc::clone(&application.storage.entry_file_set_repo)),
        );
        let live_index: Arc<dyn ClipboardLiveIndexPort> =
            Arc::new(ClipboardLiveIndexer::new(ClipboardLiveIndexDeps {
                clipboard_entry_repo: Arc::clone(&application.clipboard.entry_ports.get),
                representation_policy: Arc::clone(&application.clipboard.representation_policy),
                search_key_derivation: Arc::clone(&application.search.search_key_derivation),
                search_pipeline: Arc::clone(&application.search.search_pipeline),
                search_index: Arc::clone(&application.search.search_index),
                event_repo: Arc::clone(&application.clipboard.clipboard_event_reader_repo),
                entry_file_set_repo: Arc::clone(&application.storage.entry_file_set_repo),
            }));

        Self {
            deps: application,
            file_cache_dir: deps.file_cache_dir,
            entry_identity,
            write_coordinator,
            capture_use_case,
            capture,
            live_index,
            file_transfer: deps.file_transfer,
            host_event_bus: deps.host_event_bus,
            background: deps.background,
            background_ready: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Clipboard 进程级启动门禁：先验证/修复 active register，再恢复 spool
    /// 并启动 blob worker。失败时网络 session 尚未创建，因而不会有读取或
    /// 广播 register 的 worker。
    pub async fn start_background(
        &self,
        task_registry: Arc<TaskRegistry>,
    ) -> Result<(), ClipboardBackgroundStartError> {
        let reconcile = self.active_reconcile();
        start_background_after_reconcile(
            reconcile.reconcile().await,
            self.background.as_ref(),
            task_registry,
        )
        .await?;
        self.background_ready.store(true, Ordering::Release);
        Ok(())
    }

    pub fn start_session(&self, session: ClipboardSessionDeps) -> ClipboardSession {
        let outbound = Arc::new(ClipboardOutboundFacade::new(ClipboardOutboundDeps {
            settings: Arc::clone(&self.deps.settings),
            clipboard_sync: Arc::clone(&session.clipboard_sync),
            blob_transfer: Arc::clone(&session.blob_transfer),
            entry_repo: Arc::clone(&self.deps.clipboard.entry_ports.get),
            event_repo: Arc::clone(&self.deps.clipboard.clipboard_event_reader_repo),
            selection_repo: Arc::clone(&self.deps.clipboard.selection_repo),
            representation_repo: Arc::clone(&self.deps.clipboard.representation_ports.get),
            rep_processing_repo: Arc::clone(
                &self
                    .deps
                    .clipboard
                    .representation_ports
                    .update_processing_result,
            ),
            payload_resolver: Arc::clone(&self.deps.clipboard.payload_resolver),
            blob_store: Arc::clone(&self.deps.storage.blob_store),
            entry_delivery_repo: Arc::clone(&session.deliveries),
            trusted_peer_repo: session.trusted_peers,
            peer_scope: Arc::clone(&session.member_scope),
            device_identity: Arc::clone(&self.deps.device.device_identity),
            entry_file_set_repo: Arc::clone(&self.deps.storage.entry_file_set_repo),
        }));
        let apply_inbound = self
            .file_transfer
            .interactive_receive(InteractiveReceiveIntentDeps {
                common: InboundReceiveIntentDeps {
                    entry_repo: Arc::clone(&self.deps.clipboard.entry_ports.find_by_snapshot_hash),
                    capture: Arc::clone(&self.capture_use_case) as Arc<dyn InboundCapture>,
                    materializer: InboundMaterializerDeps {
                        fetcher: session.inbound_adapters.fetcher,
                        publisher: session.inbound_adapters.publisher,
                        target_reserver: session.inbound_adapters.target_reserver,
                        hidden_marker: session.inbound_adapters.hidden_marker,
                    },
                    host_event_emitter: Arc::clone(&self.host_event_bus),
                    search_live_index: Arc::clone(&self.live_index),
                    availability: Arc::clone(&self.deps.clipboard.entry_ports.availability),
                    entry_identity_coordinator: Arc::clone(&self.entry_identity),
                },
                write: Arc::clone(&self.write_coordinator) as Arc<dyn InboundWrite>,
                provisional_receive: Arc::clone(
                    &self.deps.storage.file_transfer.finalize_provisional,
                ),
                outbound_progress_reporter: session.outbound_progress_reporter,
                active_register: Arc::clone(&self.deps.clipboard.active_register),
                mobile_consumability: self.deps.clipboard.mobile_consumability.clone(),
                snapshot_deps: self.snapshot_deps(),
                touch_entry: Arc::clone(&self.deps.clipboard.entry_ports.touch),
            });
        let inbound = ClipboardInboundRuntime::start(ClipboardInboundRuntimeDeps {
            receiver: session.receiver,
            member_repo: Arc::clone(&self.deps.device.member_repo),
            member_scope: Arc::clone(&session.member_scope),
            transfer_cipher: Arc::clone(&self.deps.security.transfer_cipher),
            settings: Arc::clone(&self.deps.settings),
            clock: Arc::clone(&self.deps.system.clock),
            apply: Arc::clone(&apply_inbound),
            events: session.inbound_events,
        });
        let sync = Arc::new(ClipboardSyncRuntime::start(ClipboardSyncRuntimeDeps {
            outbound: Arc::clone(&outbound),
            settings: Arc::clone(&self.deps.settings),
            inbound,
            presence: session.presence,
            known_peers: session.known_peers,
            entries: Arc::clone(&self.deps.clipboard.entry_ports.list),
            events: Arc::clone(&self.deps.clipboard.clipboard_event_reader_repo),
            deliveries: session.deliveries,
            device_identity: Arc::clone(&self.deps.device.device_identity),
            clock: Arc::clone(&self.deps.system.clock),
        }));

        ClipboardSession {
            capture: self.capture(),
            live_index: self.live_index(),
            outbound,
            sync,
            apply_inbound,
        }
    }

    pub fn active_pull_serve(
        &self,
        blob_publisher: Arc<BlobTransferFacade>,
    ) -> Arc<dyn ActiveClipboardPullServePort> {
        build_active_clipboard_pull_serve_port(ActiveClipboardPullServeFacadeDeps {
            entry_lookup: Arc::clone(&self.deps.clipboard.entry_ports.find_by_snapshot_hash),
            settings: Arc::clone(&self.deps.settings),
            transfer_cipher: Arc::clone(&self.deps.security.transfer_cipher),
            blob_publisher,
            entry_file_set_repo: Arc::clone(&self.deps.storage.entry_file_set_repo),
            snapshot: self.snapshot_deps(),
        })
    }

    pub async fn start_active(
        &self,
        session: ActiveClipboardSessionDeps,
    ) -> Result<ActiveClipboardSession, ActiveClipboardStartError> {
        if !self.background_ready.load(Ordering::Acquire) {
            return Err(ActiveClipboardStartError::BackgroundNotReady);
        }

        let pull_apply = self.store_only_pull(session.pull_adapters);
        let facade = Arc::new(ActiveClipboardFacade::new(ActiveClipboardDeps {
            receiver: session.receiver,
            dispatch: session.dispatch,
            is_unlocked: session.is_unlocked,
            load_register: Arc::clone(&self.deps.clipboard.active_register_load),
            advance_register: Arc::clone(&self.deps.clipboard.active_register),
            mobile_consumability: self.deps.clipboard.mobile_consumability.clone(),
            member_repo: Arc::clone(&self.deps.device.member_repo),
            peer_addr_repo: session.peer_addresses,
            peer_scope: session.member_scope,
            presence: session.peer_reachability,
            entry_lookup: Arc::clone(&self.deps.clipboard.entry_ports.find_by_snapshot_hash),
            availability: Some(Arc::clone(&self.deps.clipboard.entry_ports.availability)),
            coordinator: Arc::clone(&self.write_coordinator),
            clock: Arc::clone(&self.deps.system.clock),
            device_identity: Arc::clone(&self.deps.device.device_identity),
            settings: Arc::clone(&self.deps.settings),
            snapshot: self.snapshot_deps(),
            transfer_cipher: Arc::clone(&self.deps.security.transfer_cipher),
            pull_client: Some(session.pull_client),
            pull_apply: Some(pull_apply),
            touch_entry: Arc::clone(&self.deps.clipboard.entry_ports.touch),
            host_event_emitter: Arc::clone(&self.host_event_bus),
            resurface_clock: Arc::clone(&self.deps.system.clock),
        }));
        let lifecycle = facade.start_background_workers();
        Ok(ActiveClipboardSession { facade, lifecycle })
    }

    fn store_only_pull(
        &self,
        adapters: ClipboardInboundAdapters,
    ) -> Arc<dyn InboundClipboardApplyPort> {
        self.file_transfer.store_only_pull(StoreOnlyPullIntentDeps {
            common: InboundReceiveIntentDeps {
                entry_repo: Arc::clone(&self.deps.clipboard.entry_ports.find_by_snapshot_hash),
                capture: Arc::clone(&self.capture_use_case) as Arc<dyn InboundCapture>,
                materializer: InboundMaterializerDeps {
                    fetcher: adapters.fetcher,
                    publisher: adapters.publisher,
                    target_reserver: adapters.target_reserver,
                    hidden_marker: adapters.hidden_marker,
                },
                host_event_emitter: Arc::clone(&self.host_event_bus),
                search_live_index: Arc::clone(&self.live_index),
                availability: Arc::clone(&self.deps.clipboard.entry_ports.availability),
                entry_identity_coordinator: Arc::clone(&self.entry_identity),
            },
        })
    }

    pub fn capture(&self) -> Arc<ClipboardCaptureFacade> {
        Arc::clone(&self.capture)
    }

    pub fn live_index(&self) -> Arc<ClipboardLiveIndexFacade> {
        Arc::new(ClipboardLiveIndexFacade::new(Arc::clone(&self.live_index)))
    }

    pub fn resource(&self) -> Arc<ResourceFacade> {
        Arc::new(ResourceFacade::new(ResourceFacadeDeps {
            representation_by_blob_id: Arc::clone(
                &self.deps.clipboard.representation_ports.get_by_blob_id,
            ),
            representations_for_event: Arc::clone(
                &self.deps.clipboard.representation_ports.list_for_event,
            ),
            thumbnail_repo: Arc::clone(&self.deps.storage.thumbnail_repo),
            blob_store: Arc::clone(&self.deps.storage.blob_store),
            entry_repo: Arc::clone(&self.deps.clipboard.entry_ports.get),
        }))
    }

    pub fn history(
        &self,
        blob_transfer: Option<Arc<dyn BlobTransferPort>>,
    ) -> Arc<ClipboardHistoryFacade> {
        Arc::new(ClipboardHistoryFacade::new(ClipboardHistoryFacadeDeps {
            entry_ports: self.deps.clipboard.entry_ports.clone(),
            selection_repo: Arc::clone(&self.deps.clipboard.selection_repo),
            representation_ports: self.deps.clipboard.representation_ports.clone(),
            event_writer: Arc::clone(&self.deps.clipboard.clipboard_event_repo),
            payload_resolver: Arc::clone(&self.deps.clipboard.payload_resolver),
            blob_store: Arc::clone(&self.deps.storage.blob_store),
            thumbnail_repo: Arc::clone(&self.deps.storage.thumbnail_repo),
            file_transfer_repo: Arc::clone(&self.deps.storage.file_transfer.entry_summary),
            entry_file_set_repo: Arc::clone(&self.deps.storage.entry_file_set_repo),
            search_index: Some(Arc::clone(&self.deps.search.search_index)),
            file_cache_dir: Some(self.file_cache_dir.clone()),
            blob_transfer,
            settings: Arc::clone(&self.deps.settings),
            device_identity: Arc::clone(&self.deps.device.device_identity),
            clock: Arc::clone(&self.deps.system.clock),
            cache_fs: Arc::clone(&self.deps.system.cache_fs),
        }))
    }

    pub fn restore(
        &self,
        integration_mode: ClipboardIntegrationMode,
        restore_broadcast: Option<RestoreBroadcastTrigger>,
    ) -> Arc<ClipboardRestoreFacade> {
        Arc::new(ClipboardRestoreFacade::new(ClipboardRestoreFacadeDeps {
            selection_repo: Arc::clone(&self.deps.clipboard.selection_repo),
            entry_ports: self.deps.clipboard.entry_ports.clone(),
            representation_ports: self.deps.clipboard.representation_ports.clone(),
            payload_resolver: Arc::clone(&self.deps.clipboard.payload_resolver),
            blob_store: Arc::clone(&self.deps.storage.blob_store),
            clock: Arc::clone(&self.deps.system.clock),
            device_identity: Arc::clone(&self.deps.device.device_identity),
            active_register: Arc::clone(&self.deps.clipboard.active_register),
            mobile_consumability: self.deps.clipboard.mobile_consumability.clone(),
            restore_broadcast,
            write_coordinator: Arc::clone(&self.write_coordinator),
            integration_mode,
        }))
    }

    fn snapshot_deps(&self) -> ClipboardSnapshotDeps {
        ClipboardSnapshotDeps {
            entry_repo: Arc::clone(&self.deps.clipboard.entry_ports.get),
            selection_repo: Arc::clone(&self.deps.clipboard.selection_repo),
            representation_repo: Arc::clone(&self.deps.clipboard.representation_ports.get),
            rep_processing_repo: Arc::clone(
                &self
                    .deps
                    .clipboard
                    .representation_ports
                    .update_processing_result,
            ),
            payload_resolver: Arc::clone(&self.deps.clipboard.payload_resolver),
            blob_store: Arc::clone(&self.deps.storage.blob_store),
        }
    }

    fn active_reconcile(&self) -> ActiveClipboardReconcileFacade {
        ActiveClipboardReconcileFacade::new(ActiveClipboardReconcileDeps {
            system_clipboard: Arc::clone(&self.deps.clipboard.system_clipboard),
            load_register: Arc::clone(&self.deps.clipboard.active_register_load),
            reset_register: Arc::clone(&self.deps.clipboard.active_register_reset),
            snapshot: self.snapshot_deps(),
        })
    }
}

async fn start_background_after_reconcile(
    reconcile: Result<
        crate::clipboard::active::ActiveClipboardReconcileOutcome,
        ActiveClipboardReconcileError,
    >,
    background: &dyn ClipboardBackgroundPort,
    task_registry: Arc<TaskRegistry>,
) -> Result<(), ClipboardBackgroundStartError> {
    reconcile.map_err(|source| ClipboardBackgroundStartError::Reconcile { source })?;
    background.start(task_registry).await?;
    Ok(())
}

impl ActiveClipboardSession {
    pub fn facade(&self) -> Arc<ActiveClipboardFacade> {
        Arc::clone(&self.facade)
    }

    pub fn attach_restore_broadcast(
        &self,
        rx: tokio::sync::mpsc::UnboundedReceiver<crate::clipboard::write::RestoreBroadcastRequest>,
    ) -> Result<(), ActiveClipboardLifecycleError> {
        self.lifecycle.attach_restore_broadcast(rx)
    }

    pub async fn shutdown(self) {
        self.lifecycle.shutdown().await;
    }
}

impl ClipboardSession {
    pub fn capture(&self) -> Arc<ClipboardCaptureFacade> {
        Arc::clone(&self.capture)
    }

    pub fn live_index(&self) -> Arc<ClipboardLiveIndexFacade> {
        Arc::clone(&self.live_index)
    }

    pub fn outbound(&self) -> Arc<ClipboardOutboundFacade> {
        Arc::clone(&self.outbound)
    }

    pub fn sync(&self) -> Arc<ClipboardSyncRuntime> {
        Arc::clone(&self.sync)
    }

    pub fn apply_inbound(&self) -> Arc<dyn InboundClipboardApplyPort> {
        Arc::clone(&self.apply_inbound)
    }

    pub async fn shutdown(self) {
        self.sync.shutdown().await;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use async_trait::async_trait;
    use uc_core::ports::clipboard::ActiveClipboardRegisterError;

    use super::*;

    struct BackgroundProbe(AtomicBool);

    #[async_trait]
    impl ClipboardBackgroundPort for BackgroundProbe {
        async fn start(
            &self,
            _task_registry: Arc<TaskRegistry>,
        ) -> Result<(), ClipboardBackgroundError> {
            self.0.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn reconcile_failure_prevents_background_workers_from_starting() {
        let background = BackgroundProbe(AtomicBool::new(false));
        let result = start_background_after_reconcile(
            Err(ActiveClipboardReconcileError::LoadRegister {
                source: ActiveClipboardRegisterError::Storage("unavailable".to_owned()),
            }),
            &background,
            Arc::new(TaskRegistry::new()),
        )
        .await;

        assert!(matches!(
            result,
            Err(ClipboardBackgroundStartError::Reconcile { .. })
        ));
        assert!(!background.0.load(Ordering::SeqCst));
    }
}
