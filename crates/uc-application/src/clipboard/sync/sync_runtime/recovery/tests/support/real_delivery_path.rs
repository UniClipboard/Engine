//! 经真实发送用例与记录负责人的端到端补发，可注入写入失败与对端脚本。

use super::super::*;
use super::SwitchablePeerReachability;

use std::collections::VecDeque;

use crate::clipboard::sync::dispatch_entry::test_support as dispatch_test_support;
use crate::clipboard::sync::DispatchClipboardEntryUseCase;
use crate::deps::ConnectivityError;
use crate::transfer::progress::ProgressProjectionWorker;
use uc_core::ports::{
    ClipboardDispatchError, ClipboardDispatchPort, ClipboardHeader, ConnectionChannel, DispatchAck,
    DispatchReport, SyncPayload,
};

/// 写入失败的种类：`fail_next` 命中一次后存储即恢复，`fail_every` 一直失败。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in super::super) enum BrokenWrite {
    /// 发送前的 `Pending` 意图写入。
    Intent,
    /// 发送后的最终结果写入。
    Final,
}

/// 可注入写入失败的投递记录仓库；读取与其余写入照常经内存仓库。
pub(in super::super) struct FlakyDeliveries {
    pub(in super::super) inner: Deliveries,
    pub(in super::super) next_failure: Mutex<Option<BrokenWrite>>,
    pub(in super::super) persistent_failure: Mutex<Option<BrokenWrite>>,
}

impl FlakyDeliveries {
    pub(in super::super) fn fail_next(&self, write: BrokenWrite) {
        *self.next_failure.lock().unwrap() = Some(write);
    }

    pub(in super::super) fn fail_every(&self, write: BrokenWrite) {
        *self.persistent_failure.lock().unwrap() = Some(write);
    }
}

#[async_trait]
impl EntryDeliveryRepositoryPort for FlakyDeliveries {
    async fn record_attempt(
        &self,
        record: &EntryDeliveryRecord,
    ) -> Result<(), uc_core::clipboard::EntryDeliveryError> {
        let write = if record.status == EntryDeliveryStatus::Pending {
            BrokenWrite::Intent
        } else {
            BrokenWrite::Final
        };
        let failed = {
            let mut next_failure = self.next_failure.lock().unwrap();
            let once = *next_failure == Some(write);
            if once {
                *next_failure = None;
            }
            once || *self.persistent_failure.lock().unwrap() == Some(write)
        };
        if failed {
            return Err(uc_core::clipboard::EntryDeliveryError::Storage {
                source: anyhow::anyhow!("PRIVATE_STORAGE_DETAIL"),
            });
        }
        self.inner.record_attempt(record).await
    }

    async fn list_by_entry(
        &self,
        entry_id: &EntryId,
    ) -> Result<Vec<EntryDeliveryRecord>, uc_core::clipboard::EntryDeliveryError> {
        self.inner.list_by_entry(entry_id).await
    }
}

/// 对端在一次投递中的表现。
#[derive(Clone, Copy)]
pub(in super::super) enum PeerStep {
    /// 确认前连接被重置，内容未保存。
    Interrupted,
    /// 收下内容；同一快照再次到达时按接收端去重返回重复确认。
    Receive,
    /// 经过给定时长才收下并确认，用于越过前台期限、进入后台收尾。
    ReceiveAfter(Duration),
    /// 取得连接时准入被拒绝（经共享拨号结果返回）。
    Denied,
}

/// 按脚本应答的对端：只在真正收下内容时保存一份副本。
pub(in super::super) struct ScriptedPeer {
    pub(in super::super) steps: Mutex<VecDeque<PeerStep>>,
    pub(in super::super) dials: AtomicUsize,
    pub(in super::super) stored: Mutex<Vec<String>>,
}

impl ScriptedPeer {
    pub(in super::super) fn new(steps: &[PeerStep]) -> Arc<Self> {
        Arc::new(Self {
            steps: Mutex::new(steps.iter().copied().collect()),
            dials: AtomicUsize::new(0),
            stored: Mutex::new(Vec::new()),
        })
    }

    pub(in super::super) fn dials(&self) -> usize {
        self.dials.load(Ordering::SeqCst)
    }

    pub(in super::super) fn copies(&self) -> usize {
        self.stored.lock().unwrap().len()
    }

    pub(in super::super) fn receive(&self, snapshot_hash: &str) -> DispatchAck {
        let mut stored = self.stored.lock().unwrap();
        if stored.iter().any(|hash| hash == snapshot_hash) {
            DispatchAck::DuplicateIgnored
        } else {
            stored.push(snapshot_hash.to_owned());
            DispatchAck::Accepted
        }
    }
}

#[async_trait]
impl ClipboardDispatchPort for ScriptedPeer {
    async fn dispatch(
        &self,
        _target: &DeviceId,
        header: &ClipboardHeader,
        _payload: SyncPayload,
    ) -> DispatchReport {
        self.dials.fetch_add(1, Ordering::SeqCst);
        let step = self
            .steps
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected extra delivery attempt");
        let outcome = match step {
            PeerStep::Interrupted => Err(ClipboardDispatchError::Transient(Box::new(
                io::Error::from(io::ErrorKind::ConnectionReset),
            ))),
            PeerStep::Receive => Ok(self.receive(&header.snapshot_hash)),
            PeerStep::ReceiveAfter(delay) => {
                tokio::time::sleep(delay).await;
                Ok(self.receive(&header.snapshot_hash))
            }
            PeerStep::Denied => Err(ClipboardDispatchError::Io(Box::new(
                ConnectivityError::Shared(Arc::new(ConnectivityError::Denied)),
            ))),
        };
        DispatchReport {
            transport: ConnectionChannel::Direct,
            outcome,
        }
    }
}

/// 恢复补发经真实发送用例；只省略从仓库重建快照这一步，快照哈希保持不变以便接收端去重。
pub(in super::super) struct RealDelivery(Arc<DispatchClipboardEntryUseCase>);

impl RealDelivery {
    pub(in super::super) async fn send(
        &self,
        entry_id: EntryId,
        targets: Option<Vec<DeviceId>>,
    ) -> Result<ResendReport, ResendEntryError> {
        let mut input = dispatch_test_support::dispatch_input();
        input.entry_id = Some(entry_id);
        input.target_filter = targets;
        let outcome = self
            .0
            .execute(input)
            .await
            .map_err(|error| ResendEntryError::Dispatch(anyhow::Error::new(error)))?;
        Ok(ResendReport {
            reconciled: Vec::new(),
            accepted: outcome.total_accepted,
            duplicate: outcome.total_duplicate,
            offline: outcome.total_offline,
            errored: outcome.total_errored,
            pending: outcome.total_pending,
        })
    }
}

#[async_trait]
impl RecoveryDeliveryPort for RealDelivery {
    async fn deliver_existing_local_entry(
        &self,
        entry_id: EntryId,
        targets: Vec<DeviceId>,
    ) -> Result<ResendReport, ResendEntryError> {
        self.send(entry_id, Some(targets)).await
    }
}

pub(in super::super) struct RealPathHarness {
    pub(in super::super) recovery: OfflineDeliveryRecovery,
    pub(in super::super) _progress_worker: ProgressProjectionWorker,
    pub(in super::super) delivery: Arc<RealDelivery>,
    pub(in super::super) deliveries: Arc<FlakyDeliveries>,
    pub(in super::super) peer: Arc<ScriptedPeer>,
    pub(in super::super) entry: ClipboardEntry,
    pub(in super::super) target: DeviceId,
}

impl RealPathHarness {
    pub(in super::super) fn start(steps: &[PeerStep]) -> Self {
        let entry = entry("copied-entry", "local-event");
        let target = DeviceId::new("target");
        let deliveries = Arc::new(FlakyDeliveries {
            inner: Deliveries {
                records: Mutex::new(HashMap::new()),
            },
            next_failure: Mutex::new(None),
            persistent_failure: Mutex::new(None),
        });
        let peer = ScriptedPeer::new(steps);
        // presence 始终在线，整个测试不产生任何上线事件。
        let reachability = SwitchablePeerReachability::new(ReachabilityState::Online);
        let scope = Arc::new(FixedMemberScope {
            usable: vec![target],
            paused: Vec::new(),
        });
        let (use_case, progress_worker) = dispatch_test_support::dispatch_use_case(
            &[target],
            Arc::clone(&peer) as Arc<dyn ClipboardDispatchPort>,
            Arc::clone(&deliveries) as Arc<dyn EntryDeliveryRepositoryPort>,
            Arc::clone(&reachability) as Arc<dyn PeerReachabilityPort>,
            Arc::clone(&scope) as Arc<dyn CurrentSpaceMemberScopePort>,
        );
        let use_case = Arc::new(use_case);
        let delivery = Arc::new(RealDelivery(Arc::clone(&use_case)));
        let mut deps = recovery_deps(
            true,
            vec![entry.clone()],
            HashMap::from([(entry.event_id.clone(), DeviceId::new("local"))]),
            Arc::new(Deliveries {
                records: Mutex::new(HashMap::new()),
            }),
            Arc::new(RecordingDispatch {
                commands: Mutex::new(Vec::new()),
                result: DispatchResult::Delivered,
            }),
        );
        deps.peer_reachability = reachability;
        deps.member_scope = scope;
        deps.deliveries = Arc::clone(&deliveries) as Arc<dyn EntryDeliveryRepositoryPort>;
        deps.delivery = Arc::clone(&delivery) as Arc<dyn RecoveryDeliveryPort>;
        deps.delivery_results = use_case.delivery_results();
        Self {
            recovery: OfflineDeliveryRecovery::start(deps),
            _progress_worker: progress_worker,
            delivery,
            deliveries,
            peer,
            entry,
            target,
        }
    }

    /// 本机复制后的自动发送。
    pub(in super::super) async fn copy(&self) {
        self.delivery
            .send(self.entry.entry_id.clone(), None)
            .await
            .unwrap();
    }

    /// 用户对同一条内容的手动重发。
    pub(in super::super) async fn resend(&self) {
        self.delivery
            .send(self.entry.entry_id.clone(), Some(vec![self.target]))
            .await
            .unwrap();
    }

    pub(in super::super) fn status(&self) -> Option<EntryDeliveryStatus> {
        self.deliveries
            .inner
            .records
            .lock()
            .unwrap()
            .get(&self.entry.entry_id)
            .and_then(|records| records.first())
            .map(|record| record.status.clone())
    }

    pub(in super::super) async fn shutdown(self) {
        self.recovery.shutdown().await.unwrap();
        self.delivery.0.shutdown().await.unwrap();
    }
}

/// 让发送、记录、后台收尾与恢复任务处理完已经就绪的工作。
pub(in super::super) async fn drain() {
    for _ in 0..256 {
        tokio::task::yield_now().await;
    }
}
