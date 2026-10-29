//! 脚本化补发：经真实记录负责人落库并发布结果，不经过真实发送用例。

use super::super::*;
use super::SwitchablePeerReachability;

use std::collections::VecDeque;

use crate::clipboard::sync::dispatch_entry::RecordingOwner;
use crate::facade::HostEventBus;

/// 一次补发的结果；可恢复与成功结果都经真实记录负责人落库并发布。
#[derive(Clone, Copy)]
pub(in super::super) enum Attempt {
    /// 内容拨号失败或流在确认前中断，落为 `Unreachable`。
    Interrupted,
    /// 对端确认收下。
    Accepted,
    /// 上次确认丢失但对端已保存，这次得到重复确认。
    Duplicate,
    /// 补发前存储失败，没有落下任何投递结果。
    StorageFailure,
}

pub(in super::super) struct ScriptedDelivery {
    pub(in super::super) recorder: Arc<RecordingOwner>,
    pub(in super::super) attempts: Mutex<VecDeque<Attempt>>,
    pub(in super::super) calls: AtomicUsize,
}

impl ScriptedDelivery {
    pub(in super::super) fn new(recorder: Arc<RecordingOwner>, attempts: &[Attempt]) -> Arc<Self> {
        Arc::new(Self {
            recorder,
            attempts: Mutex::new(attempts.iter().copied().collect()),
            calls: AtomicUsize::new(0),
        })
    }

    pub(in super::super) fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl RecoveryDeliveryPort for ScriptedDelivery {
    async fn deliver_existing_local_entry(
        &self,
        entry_id: EntryId,
        targets: Vec<DeviceId>,
    ) -> Result<ResendReport, ResendEntryError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let attempt = self
            .attempts
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected extra recovery attempt");
        let (status, report) = match attempt {
            Attempt::Interrupted => (
                EntryDeliveryStatus::Unreachable,
                ResendReport {
                    reconciled: Vec::new(),
                    accepted: 0,
                    duplicate: 0,
                    offline: 1,
                    errored: 0,
                    pending: 0,
                },
            ),
            Attempt::Accepted => (
                EntryDeliveryStatus::Delivered,
                ResendReport {
                    reconciled: Vec::new(),
                    accepted: 1,
                    duplicate: 0,
                    offline: 0,
                    errored: 0,
                    pending: 0,
                },
            ),
            Attempt::Duplicate => (
                EntryDeliveryStatus::Duplicate,
                ResendReport {
                    reconciled: Vec::new(),
                    accepted: 0,
                    duplicate: 1,
                    offline: 0,
                    errored: 0,
                    pending: 0,
                },
            ),
            Attempt::StorageFailure => {
                return Err(ResendEntryError::Storage(anyhow::anyhow!(
                    "PRIVATE_STORAGE_DETAIL"
                )))
            }
        };
        let records: Vec<_> = targets
            .into_iter()
            .map(|target_device_id| EntryDeliveryRecord {
                entry_id: entry_id.clone(),
                target_device_id,
                status: status.clone(),
                reason_detail: None,
                updated_at_ms: 43,
            })
            .collect();
        self.recorder.flush(&records).await;
        Ok(report)
    }
}

pub(in super::super) struct RetryHarness {
    pub(in super::super) recovery: OfflineDeliveryRecovery,
    pub(in super::super) recorder: Arc<RecordingOwner>,
    pub(in super::super) deliveries: Arc<Deliveries>,
    pub(in super::super) delivery: Arc<ScriptedDelivery>,
    pub(in super::super) reachability: Arc<SwitchablePeerReachability>,
    pub(in super::super) entry: ClipboardEntry,
    pub(in super::super) target: DeviceId,
}

impl RetryHarness {
    pub(in super::super) fn start(
        initial: Option<EntryDeliveryStatus>,
        attempts: &[Attempt],
    ) -> Self {
        let entry = entry("copied-entry", "local-event");
        let target = DeviceId::new("target");
        let deliveries = Arc::new(Deliveries {
            records: Mutex::new(HashMap::new()),
        });
        if let Some(status) = initial {
            deliveries.records.lock().unwrap().insert(
                entry.entry_id.clone(),
                vec![EntryDeliveryRecord {
                    entry_id: entry.entry_id.clone(),
                    target_device_id: target,
                    status,
                    reason_detail: None,
                    updated_at_ms: 1,
                }],
            );
        }
        let recorder = Arc::new(RecordingOwner::new(
            Arc::clone(&deliveries) as Arc<dyn EntryDeliveryRepositoryPort>,
            Arc::new(HostEventBus::new()),
        ));
        let delivery = ScriptedDelivery::new(Arc::clone(&recorder), attempts);
        let reachability = SwitchablePeerReachability::new(ReachabilityState::Online);
        let mut deps = recovery_deps(
            true,
            vec![entry.clone()],
            HashMap::from([(entry.event_id.clone(), DeviceId::new("local"))]),
            Arc::clone(&deliveries),
            Arc::new(RecordingDispatch {
                commands: Mutex::new(Vec::new()),
                result: DispatchResult::Delivered,
            }),
        );
        deps.peer_reachability = Arc::clone(&reachability) as Arc<dyn PeerReachabilityPort>;
        deps.delivery = Arc::clone(&delivery) as Arc<dyn RecoveryDeliveryPort>;
        deps.delivery_results = recorder.delivery_results();
        Self {
            recovery: OfflineDeliveryRecovery::start(deps),
            recorder,
            deliveries,
            delivery,
            reachability,
            entry,
            target,
        }
    }

    /// 模拟自动投递：经真实记录负责人写入本次结果（并在可恢复时发布通知）。
    pub(in super::super) async fn record_dispatch(&self, status: EntryDeliveryStatus) {
        self.recorder
            .flush(&[EntryDeliveryRecord {
                entry_id: self.entry.entry_id.clone(),
                target_device_id: self.target,
                status,
                reason_detail: None,
                updated_at_ms: 2,
            }])
            .await;
    }

    pub(in super::super) fn status(&self) -> Option<EntryDeliveryStatus> {
        self.deliveries
            .records
            .lock()
            .unwrap()
            .get(&self.entry.entry_id)
            .and_then(|records| records.first())
            .map(|record| record.status.clone())
    }
}

/// 让后台恢复任务处理已经到达的通知与到期计时器。
pub(in super::super) async fn settle() {
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
}
