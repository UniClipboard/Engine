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

/// 恢复负责人可列举的配对设备；未知目标的地址资料与写入都不参与这些场景。
pub(in super::super) struct KnownPeers(pub(in super::super) Vec<DeviceId>);

#[async_trait]
impl PeerAddressRepositoryPort for KnownPeers {
    async fn get(
        &self,
        _device: &DeviceId,
    ) -> Result<Option<uc_core::ports::PeerAddressRecord>, uc_core::ports::PeerAddressError> {
        Ok(None)
    }

    async fn upsert(
        &self,
        _record: &uc_core::ports::PeerAddressRecord,
    ) -> Result<(), uc_core::ports::PeerAddressError> {
        Ok(())
    }

    async fn list(
        &self,
    ) -> Result<Vec<uc_core::ports::PeerAddressRecord>, uc_core::ports::PeerAddressError> {
        Ok(self
            .0
            .iter()
            .map(|device_id| uc_core::ports::PeerAddressRecord {
                device_id: *device_id,
                addr_blob: Vec::new(),
                observed_at: chrono::Utc::now(),
            })
            .collect())
    }

    async fn remove(&self, _device: &DeviceId) -> Result<(), uc_core::ports::PeerAddressError> {
        Ok(())
    }
}

/// 场景可选项。
#[derive(Default)]
pub(in super::super) struct RetryScenario {
    /// 按新到旧排在当前条目之后的一条较旧本机条目，及其对目标的初始投递状态。
    pub(in super::super) older_entry: Option<EntryDeliveryStatus>,
    /// 恢复负责人可以列举到目标（启动扫描与通知积压时据此查找在线目标）。
    pub(in super::super) known_target: bool,
    /// 其余同样在线、在成员范围内且已知的目标；每个目标各有一条本机条目，排在当前条目之后。
    pub(in super::super) other_targets: usize,
}

pub(in super::super) struct RetryHarness {
    pub(in super::super) recovery: OfflineDeliveryRecovery,
    pub(in super::super) recorder: Arc<RecordingOwner>,
    pub(in super::super) deliveries: Arc<Deliveries>,
    pub(in super::super) delivery: Arc<ScriptedDelivery>,
    pub(in super::super) reachability: Arc<SwitchablePeerReachability>,
    pub(in super::super) entry: ClipboardEntry,
    pub(in super::super) older: ClipboardEntry,
    pub(in super::super) target: DeviceId,
}

impl RetryHarness {
    pub(in super::super) fn start(
        initial: Option<EntryDeliveryStatus>,
        attempts: &[Attempt],
    ) -> Self {
        Self::start_with(initial, attempts, RetryScenario::default())
    }

    pub(in super::super) fn start_with(
        initial: Option<EntryDeliveryStatus>,
        attempts: &[Attempt],
        scenario: RetryScenario,
    ) -> Self {
        let older = entry("older-entry", "older-event");
        let others: Vec<_> = (0..scenario.other_targets).map(other_target).collect();
        let entry = entry("copied-entry", "local-event");
        let target = DeviceId::new("target");
        let deliveries = Arc::new(Deliveries {
            records: Mutex::new(HashMap::new()),
        });
        let mut seeded = Vec::new();
        if let Some(status) = initial {
            seeded.push((entry.entry_id.clone(), status));
        }
        if let Some(status) = scenario.older_entry.clone() {
            seeded.push((older.entry_id.clone(), status));
        }
        for (entry_id, status) in seeded {
            deliveries.records.lock().unwrap().insert(
                entry_id.clone(),
                vec![EntryDeliveryRecord {
                    entry_id,
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
        // 列表按新到旧：当前条目最新，较旧条目与其余目标的条目排在其后。
        let mut entries = vec![entry.clone()];
        let mut sources = HashMap::from([(entry.event_id.clone(), DeviceId::new("local"))]);
        if scenario.older_entry.is_some() {
            entries.push(older.clone());
            sources.insert(older.event_id.clone(), DeviceId::new("local"));
        }
        for (_, other) in &others {
            entries.push(other.clone());
            sources.insert(other.event_id.clone(), DeviceId::new("local"));
        }
        let mut deps = recovery_deps(
            true,
            entries,
            sources,
            Arc::clone(&deliveries),
            Arc::new(RecordingDispatch {
                commands: Mutex::new(Vec::new()),
                result: DispatchResult::Delivered,
            }),
        );
        let mut usable = vec![target];
        usable.extend(others.iter().map(|(device, _)| *device));
        if scenario.known_target || !others.is_empty() {
            deps.known_peers = Arc::new(KnownPeers(usable.clone()));
        }
        deps.member_scope = Arc::new(FixedMemberScope {
            usable,
            paused: Vec::new(),
        });
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
            older,
            target,
        }
    }

    /// 经真实记录负责人写入一条结果（并按结果发布通知）。一次写入多条时中间不让出执行权，
    /// 用于在恢复负责人处理之前让结果通知超出缓冲上限。
    pub(in super::super) async fn record_results(
        &self,
        results: impl IntoIterator<Item = (EntryId, DeviceId, EntryDeliveryStatus)>,
    ) {
        let records: Vec<_> = results
            .into_iter()
            .map(|(entry_id, target_device_id, status)| EntryDeliveryRecord {
                entry_id,
                target_device_id,
                status,
                reason_detail: None,
                updated_at_ms: 2,
            })
            .collect();
        self.recorder.flush(&records).await;
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

/// [`RetryScenario::other_targets`] 中第 `index` 个目标及其条目。
pub(in super::super) fn other_target(index: usize) -> (DeviceId, ClipboardEntry) {
    (
        DeviceId::new(&format!("other-target-{index}")),
        entry(
            &format!("other-entry-{index}"),
            &format!("other-event-{index}"),
        ),
    )
}

/// 让后台恢复任务处理已经到达的通知与到期计时器。
pub(in super::super) async fn settle() {
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
}
