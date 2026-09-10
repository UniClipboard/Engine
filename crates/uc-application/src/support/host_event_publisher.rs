use std::collections::HashSet;
use std::sync::{Arc, Mutex, PoisonError};

use anyhow::Result;
use async_trait::async_trait;
use tracing::{debug, warn};

/// 显式恢复 poisoned mutex 守卫,并 log 警告。
///
/// `unwrap_or_else(|p| p.into_inner())` 直接吞 poison 会让 invariant 违
/// 反静默,排障时完全找不到信号。这里集中加 warn 让"前一次 panic 留下
/// 了不一致状态"在日志里有据可查。`context` 标明发生位置(锁名),便于
/// grep。
#[inline]
fn recover_poisoned<T>(poisoned: PoisonError<T>, context: &'static str) -> T {
    warn!(
        context,
        "host event publisher: lock poisoned, recovering inner state (a prior panic likely left invariants broken)"
    );
    poisoned.into_inner()
}
use uc_core::file_transfer::{
    FileTransferEvent, FileTransferEventPublisherPort, FileTransferFailureReason,
};
use uc_core::ports::{FindAttemptIdForTransferPort, FindEntryIdForTransferPort};

use crate::facade::host_event::HostEventBus;
use crate::support::outbound_entry_cache::OutboundEntryIdCache;
use uc_core::ports::host_event::{HostEvent, TransferHostEvent};

pub struct FileTransferHostEventPublisher {
    bus: Arc<HostEventBus>,
    file_transfer_repo: Arc<dyn FindEntryIdForTransferPort>,
    attempt_repo: Arc<dyn FindAttemptIdForTransferPort>,
    outbound_entry_cache: Arc<OutboundEntryIdCache>,
    /// 已经为占位 transfer 发过 Progress(entry_id=None)的 transfer_id 集合,
    /// 仅用于诊断日志去重(每条 transfer 只 warn 一次"buffered 阶段 progress
    /// 没有 entry 链接")。
    progress_no_entry_warned: Arc<Mutex<HashSet<String>>>,
}

impl FileTransferHostEventPublisher {
    pub fn new(
        bus: Arc<HostEventBus>,
        file_transfer_repo: Arc<dyn FindEntryIdForTransferPort>,
        attempt_repo: Arc<dyn FindAttemptIdForTransferPort>,
        outbound_entry_cache: Arc<OutboundEntryIdCache>,
    ) -> Self {
        Self {
            bus,
            file_transfer_repo,
            attempt_repo,
            outbound_entry_cache,
            progress_no_entry_warned: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Resolve the entry that owns the transfer, including outbound cache fallback.
    async fn resolve_entry_id(&self, transfer_id: &str) -> Option<String> {
        let resolved = match self
            .file_transfer_repo
            .get_entry_id_for_transfer(transfer_id)
            .await
        {
            Ok(Some(entry_id)) => Some(entry_id),
            Ok(None) => None,
            Err(err) => {
                warn!(error = %err, transfer_id, "failed to resolve entry_id from projection");
                None
            }
        };

        resolved.or_else(|| self.outbound_entry_cache.get(transfer_id))
    }

    async fn resolve_attempt_id(&self, transfer_id: &str) -> Option<String> {
        match self
            .attempt_repo
            .get_attempt_id_for_transfer(transfer_id)
            .await
        {
            Ok(attempt_id) => attempt_id,
            Err(error) => {
                warn!(error = %error, transfer_id, "failed to resolve attempt_id from projection");
                None
            }
        }
    }

    fn emit(&self, event: HostEvent) {
        self.bus.emit_or_warn(event);
    }
}

#[async_trait]
impl FileTransferEventPublisherPort for FileTransferHostEventPublisher {
    async fn publish(&self, event: FileTransferEvent) -> Result<()> {
        match event {
            FileTransferEvent::Started { transfer_id, .. } => {
                self.publish_status_change(&transfer_id, "transferring", None, "Started")
                    .await;
            }
            FileTransferEvent::Progress {
                transfer_id,
                peer_id,
                progress,
            } => {
                let entry_id = self.resolve_entry_id(&transfer_id).await;
                if entry_id.is_none() {
                    let mut warned = self
                        .progress_no_entry_warned
                        .lock()
                        .unwrap_or_else(|p| recover_poisoned(p, "progress_no_entry_warned"));
                    if warned.insert(transfer_id.clone()) {
                        debug!(
                            transfer_id = %transfer_id,
                            "buffered-phase progress: no real entry_id; front-end indexes via transferId only"
                        );
                    }
                }
                self.emit(HostEvent::Transfer(TransferHostEvent::Progress {
                    attempt_id: self.resolve_attempt_id(&transfer_id).await,
                    transfer_id,
                    entry_id,
                    peer_id,
                    direction: progress.direction,
                    bytes_transferred: progress.bytes_transferred,
                    total_bytes: progress.total_bytes,
                }));
            }
            FileTransferEvent::Completed { transfer_id, .. } => {
                self.publish_status_change(&transfer_id, "completed", None, "Completed")
                    .await;
            }
            FileTransferEvent::Failed {
                transfer_id,
                reason,
                detail,
                ..
            } => {
                let reason_label = Some(format_failure_reason(reason, detail.as_deref()));
                self.publish_status_change(&transfer_id, "failed", reason_label, "Failed")
                    .await;
            }
            FileTransferEvent::Cancelled {
                transfer_id,
                reason,
                ..
            } => {
                let reason_label = Some(reason.as_str().to_string());
                self.publish_status_change(&transfer_id, "cancelled", reason_label, "Cancelled")
                    .await;
            }
        }
        Ok(())
    }
}

impl FileTransferHostEventPublisher {
    async fn publish_status_change(
        &self,
        transfer_id: &str,
        status: &str,
        reason: Option<String>,
        event_kind: &'static str,
    ) {
        let entry_id = self.resolve_entry_id(transfer_id).await;
        if entry_id.is_none() {
            debug!(
                event_kind,
                "publishing provisional transfer status without entry ownership"
            );
        }
        self.emit(HostEvent::Transfer(TransferHostEvent::StatusChanged {
            transfer_id: transfer_id.to_string(),
            entry_id,
            attempt_id: self.resolve_attempt_id(transfer_id).await,
            status: status.to_string(),
            reason,
        }));
    }
}

fn failure_reason_label(reason: FileTransferFailureReason) -> &'static str {
    match reason {
        FileTransferFailureReason::NetworkUnavailable => "network_unavailable",
        FileTransferFailureReason::TimedOut => "timed_out",
        FileTransferFailureReason::AccessDenied => "access_denied",
        FileTransferFailureReason::StorageUnavailable => "storage_unavailable",
        FileTransferFailureReason::IntegrityCheckFailed => "integrity_check_failed",
        FileTransferFailureReason::Unknown => "unknown",
    }
}

fn format_failure_reason(reason: FileTransferFailureReason, detail: Option<&str>) -> String {
    let label = failure_reason_label(reason);
    match detail.map(str::trim).filter(|s| !s.is_empty()) {
        Some(detail) => format!("{label}: {detail}"),
        None => label.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uc_core::ports::host_event::{EmitError, HostEventEmitterPort};
    use uc_core::ports::FileTransferProjectionError;

    struct UnownedTransfer;
    #[async_trait]
    impl FindEntryIdForTransferPort for UnownedTransfer {
        async fn get_entry_id_for_transfer(
            &self,
            _: &str,
        ) -> std::result::Result<Option<String>, FileTransferProjectionError> {
            Ok(None)
        }
    }
    #[async_trait]
    impl FindAttemptIdForTransferPort for UnownedTransfer {
        async fn get_attempt_id_for_transfer(
            &self,
            _: &str,
        ) -> std::result::Result<Option<String>, FileTransferProjectionError> {
            Ok(None)
        }
    }
    #[derive(Default)]
    struct Recorder(Mutex<Vec<HostEvent>>);
    impl HostEventEmitterPort for Recorder {
        fn emit(&self, event: HostEvent) -> std::result::Result<(), EmitError> {
            self.0.lock().unwrap().push(event);
            Ok(())
        }
    }

    #[tokio::test]
    async fn completed_provisional_transfer_reaches_host_without_entry() {
        let bus = Arc::new(HostEventBus::new());
        let recorder = Arc::new(Recorder::default());
        bus.register("test", recorder.clone());
        let publisher = FileTransferHostEventPublisher::new(
            bus,
            Arc::new(UnownedTransfer),
            Arc::new(UnownedTransfer),
            Arc::new(OutboundEntryIdCache::new()),
        );
        publisher
            .publish(FileTransferEvent::completed("mobile-lan:duplicate", "peer"))
            .await
            .unwrap();
        let events = recorder.0.lock().unwrap();
        assert!(
            matches!(events.as_slice(), [HostEvent::Transfer(TransferHostEvent::StatusChanged { transfer_id, entry_id: None, attempt_id: None, status, .. })] if transfer_id == "mobile-lan:duplicate" && status == "completed"),
            "the HUD must receive completion after duplicate provisional data is discarded"
        );
    }
    #[tokio::test]
    async fn provisional_failures_and_cancellations_reach_host() {
        use uc_core::file_transfer::FileTransferCancellationReason;
        for (event, expected) in [
            (
                FileTransferEvent::failed(
                    "temporary",
                    "peer",
                    FileTransferFailureReason::TimedOut,
                    None,
                ),
                "failed",
            ),
            (
                FileTransferEvent::cancelled(
                    "temporary",
                    "peer",
                    FileTransferCancellationReason::LocalUser,
                ),
                "cancelled",
            ),
        ] {
            let bus = Arc::new(HostEventBus::new());
            let recorder = Arc::new(Recorder::default());
            bus.register("test", recorder.clone());
            let publisher = FileTransferHostEventPublisher::new(
                bus,
                Arc::new(UnownedTransfer),
                Arc::new(UnownedTransfer),
                Arc::new(OutboundEntryIdCache::new()),
            );
            publisher.publish(event).await.unwrap();
            let events = recorder.0.lock().unwrap();
            assert!(
                matches!(events.as_slice(), [HostEvent::Transfer(TransferHostEvent::StatusChanged { transfer_id, entry_id: None, status, .. })] if transfer_id == "temporary" && status == expected)
            );
        }
    }
}
