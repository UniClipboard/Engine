//! 通知只保存每任务最新值；网络阻塞不进入获取和取消的等待链。
use super::transport::{BlobTransport, DownloadTask};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use uc_core::file_transfer::{OutboundProgressReporterPort, OutboundProgressStatus};
use uc_core::ids::DeviceId;
use uc_observability_contract::uc_warn;

const MAX_PENDING_NOTIFICATIONS: usize = 64;
struct Report {
    target: DeviceId,
    transfer: String,
    bytes: u64,
    total: Option<u64>,
    status: OutboundProgressStatus,
}
#[derive(Default)]
struct Pending {
    latest: HashMap<(DeviceId, String), Report>,
    order: VecDeque<(DeviceId, String)>,
}
#[derive(Default)]
struct Mailbox {
    pending: Mutex<Pending>,
    changed: Notify,
}
pub(super) struct OutboundNotifications {
    mailbox: Arc<Mailbox>,
    _task: DownloadTask<()>,
}
impl OutboundNotifications {
    pub fn new(
        reporter: Arc<dyn OutboundProgressReporterPort>,
        transport: &Arc<BlobTransport>,
    ) -> Arc<Self> {
        let mailbox = Arc::new(Mailbox::default());
        let owned = mailbox.clone();
        let cancel = CancellationToken::new();
        let stopping = cancel.clone();
        let task = transport.supervise(cancel, async move {
            loop {
                let changed = owned.changed.notified();
                let report = {
                    let mut pending = owned.pending.lock().unwrap_or_else(PoisonError::into_inner);
                    pending.order.pop_front().and_then(|key| pending.latest.remove(&key))
                };
                match report {
                    Some(report) => {
                        tokio::select! {
                            biased;
                            _ = stopping.cancelled() => break,
                            _ = reporter.report(&report.target, &report.transfer, report.bytes, report.total, report.status) => {},
                        }
                    }
                    None => tokio::select! { biased; _ = stopping.cancelled() => break, _ = changed => {} },
                }
            }
            Ok(())
        });
        Arc::new(Self {
            mailbox,
            _task: task,
        })
    }
    pub fn report(
        &self,
        target: &DeviceId,
        transfer: &str,
        bytes: u64,
        total: Option<u64>,
        status: OutboundProgressStatus,
    ) {
        let key = (*target, transfer.to_owned());
        let mut pending = self
            .mailbox
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(previous) = pending.latest.get(&key) {
            if !matches!(previous.status, OutboundProgressStatus::InProgress)
                && matches!(status, OutboundProgressStatus::InProgress)
            {
                return;
            }
        } else {
            if pending.latest.len() >= MAX_PENDING_NOTIFICATIONS {
                uc_warn!(
                    error_kind = "blob_notification_busy",
                    "blob notification handoff full"
                );
                return;
            }
            pending.order.push_back(key.clone());
        }
        pending.latest.insert(
            key,
            Report {
                target: *target,
                transfer: transfer.to_owned(),
                bytes,
                total,
                status,
            },
        );
        self.mailbox.changed.notify_one();
    }
}
