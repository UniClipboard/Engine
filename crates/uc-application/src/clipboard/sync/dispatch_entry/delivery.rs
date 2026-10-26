//! Per-peer result classification + delivery recording + the deferred
//! (post-deadline) drain continuation.
//!
//! `DeliveryRecorder` holds the two ports that persist and surface
//! delivery outcomes (`entry_delivery_repo` + the host-event bus).
//! Write-then-emit ordering in [`DeliveryRecorder::flush`] is LOAD-BEARING:
//! the host event is a payload-less "refetch ping", so the frontend's
//! follow-up read must observe the DB write — reordering or batching
//! emit-before-write surfaces a stale snapshot to the detail view.

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use tokio::sync::{broadcast, oneshot, Mutex};
use tokio::task::{JoinError, JoinSet};
use tracing::Instrument;

use uc_core::clipboard::{DeliveryFailureReason, EntryDeliveryRecord, EntryDeliveryStatus};
use uc_core::ids::{DeviceId, EntryId};
use uc_core::ports::{ClipboardDispatchError, ClockPort, DispatchAck, EntryDeliveryRepositoryPort};
use uc_observability_contract::diagnostics::{DiagnosticTaskKind, ObservationContext};
use uc_observability_contract::{
    error_source::io_error_kind, log_fields::log_id, uc_debug, uc_info, uc_warn,
};

use crate::facade::blob_transfer::SharedHostEventEmitter;
use crate::facade::host_event::{DeliveryHostEvent, HostEvent};

use super::super::work::OwnedWork;
use super::{DispatchPerTarget, PeerDispatchResult};

#[cfg(test)]
#[path = "delivery/lifecycle_tests.rs"]
mod lifecycle_tests;

/// Outcome bucket for [`classify_dispatch_result`]. `Panicked` rolls up
/// into `Errored` at the call site because `DispatchOutcome` has no
/// separate panic bucket and treating a panicked task as "errored" keeps
/// `attempted = succeeded + failed + deferred` intact for telemetry.
pub(crate) enum DispatchResultBucket {
    Accepted,
    Duplicate,
    Offline,
    Errored,
    Panicked,
}

/// Folded view of one fanned-out peer's `JoinSet` result, ready to fold
/// into `DispatchOutcome` (or the background continuation).
pub(crate) struct ProcessedDispatchResult {
    /// `None` iff the task panicked / was cancelled (no DeviceId recoverable).
    pub per_target: Option<DispatchPerTarget>,
    /// `None` iff `entry_id` was `None` OR the task panicked. Otherwise a
    /// fully populated record ready for `record_attempt`.
    pub delivery_record: Option<EntryDeliveryRecord>,
    pub bucket: DispatchResultBucket,
}

/// Shared per-peer result-handling — used by both the foreground fold and
/// the background continuation that drains the leftover `JoinSet` after the
/// fan-out deadline. A free function (not a method) so the detached
/// background task can call it without holding `&self`.
///
/// `now_ms` is sampled by the caller (each peer's `updated_at_ms` reflects
/// the moment that peer's result was observed, not a shared snapshot).
pub(crate) fn classify_dispatch_result(
    joined: Result<PeerDispatchResult, JoinError>,
    entry_id: Option<&EntryId>,
    now_ms: i64,
) -> ProcessedDispatchResult {
    match joined {
        Ok((device_id, Ok(DispatchAck::Accepted))) => {
            uc_debug!("dispatch accepted");
            let delivery_record = entry_id.map(|eid| EntryDeliveryRecord {
                entry_id: eid.clone(),
                target_device_id: device_id,
                status: EntryDeliveryStatus::Delivered,
                reason_detail: None,
                updated_at_ms: now_ms,
            });
            ProcessedDispatchResult {
                per_target: Some(DispatchPerTarget {
                    device_id,
                    outcome: Ok(DispatchAck::Accepted),
                }),
                delivery_record,
                bucket: DispatchResultBucket::Accepted,
            }
        }
        Ok((device_id, Ok(DispatchAck::DuplicateIgnored))) => {
            uc_debug!("dispatch duplicate ignored");
            let delivery_record = entry_id.map(|eid| EntryDeliveryRecord {
                entry_id: eid.clone(),
                target_device_id: device_id,
                status: EntryDeliveryStatus::Duplicate,
                reason_detail: None,
                updated_at_ms: now_ms,
            });
            ProcessedDispatchResult {
                per_target: Some(DispatchPerTarget {
                    device_id,
                    outcome: Ok(DispatchAck::DuplicateIgnored),
                }),
                delivery_record,
                bucket: DispatchResultBucket::Duplicate,
            }
        }
        Ok((device_id, Err(ClipboardDispatchError::Offline))) => {
            uc_debug!("dispatch deferred because peer is offline");
            let delivery_record = entry_id.map(|eid| EntryDeliveryRecord {
                entry_id: eid.clone(),
                target_device_id: device_id,
                status: EntryDeliveryStatus::Unreachable,
                reason_detail: None,
                updated_at_ms: now_ms,
            });
            ProcessedDispatchResult {
                per_target: Some(DispatchPerTarget {
                    device_id,
                    outcome: Err("offline".to_string()),
                }),
                delivery_record,
                bucket: DispatchResultBucket::Offline,
            }
        }
        Ok((device_id, Err(err @ ClipboardDispatchError::Transient(_)))) => {
            uc_debug!(
                error_kind = "dispatch_transient",
                io_error_kind = io_error_kind(&err),
                "dispatch deferred because the transport was interrupted"
            );
            // 确认前的暂时中断与不可达同属可恢复结果；对端若已保存，重试会得到重复确认。
            let delivery_record = entry_id.map(|eid| EntryDeliveryRecord {
                entry_id: eid.clone(),
                target_device_id: device_id,
                status: EntryDeliveryStatus::Unreachable,
                reason_detail: None,
                updated_at_ms: now_ms,
            });
            ProcessedDispatchResult {
                per_target: Some(DispatchPerTarget {
                    device_id,
                    outcome: Err(err.to_string()),
                }),
                delivery_record,
                bucket: DispatchResultBucket::Offline,
            }
        }
        Ok((device_id, Err(err))) => {
            uc_warn!(
                error_kind = "dispatch_failed",
                io_error_kind = io_error_kind(&err),
                "dispatch failed"
            );
            let (failure_reason, reason_detail) = match &err {
                // Offline is handled in the previous arm (Unreachable); this
                // arm only fires for the non-Offline error variants.
                ClipboardDispatchError::Offline | ClipboardDispatchError::Transient(_) => {
                    unreachable!("recoverable failures are matched in the dedicated arms above")
                }
                ClipboardDispatchError::LocalPolicyExceeded(s) => {
                    (DeliveryFailureReason::LocalPolicy, Some(s.clone()))
                }
                ClipboardDispatchError::PeerRejected(s) => {
                    (DeliveryFailureReason::PeerRejected, Some(s.clone()))
                }
                ClipboardDispatchError::PeerIncompatible => (
                    DeliveryFailureReason::PeerIncompatible,
                    Some(err.to_string()),
                ),
                ClipboardDispatchError::Io(source) => {
                    (DeliveryFailureReason::Io, Some(source.to_string()))
                }
                ClipboardDispatchError::Internal(source) => {
                    (DeliveryFailureReason::Internal, Some(source.to_string()))
                }
            };
            let delivery_record = entry_id.map(|eid| EntryDeliveryRecord {
                entry_id: eid.clone(),
                target_device_id: device_id,
                status: EntryDeliveryStatus::Failed {
                    reason: failure_reason,
                },
                reason_detail,
                updated_at_ms: now_ms,
            });
            ProcessedDispatchResult {
                per_target: Some(DispatchPerTarget {
                    device_id,
                    outcome: Err(err.to_string()),
                }),
                delivery_record,
                bucket: DispatchResultBucket::Errored,
            }
        }
        Err(err) => {
            uc_warn!(
                error_kind = "dispatch_task_join",
                io_error_kind = io_error_kind(&err),
                "dispatch task panicked or cancelled"
            );
            ProcessedDispatchResult {
                per_target: None,
                delivery_record: None,
                bucket: DispatchResultBucket::Panicked,
            }
        }
    }
}

/// 投递结果通知的缓冲上限；恢复负责人落后时按丢失处理并重新扫描在线目标。
const DELIVERY_RESULT_CAPACITY: usize = 64;

/// 一条投递结果对自动恢复的含义。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeliveryResultKind {
    /// 结果已落库为 `Unreachable`：目标需要再试。
    Recoverable,
    /// 意图或最终结果未能落库：持久状态停留在此前的 `Pending`/`Unreachable`，
    /// 恢复负责人必须继续承担这次投递，而不是等待一个不会到来的可恢复结果。
    NotPersisted,
    /// 结果已落库为送达、重复确认或终态失败：这条内容对该目标不再需要重试。
    Settled,
}

/// 一条投递结果通知。只说明哪个目标的哪条内容得到了何种结果，不携带内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeliveryResult {
    pub entry_id: EntryId,
    pub target_device_id: DeviceId,
    pub kind: DeliveryResultKind,
}

/// 投递结果的订阅入口。只由投递记录负责人在每次写入结束后发布（成功落库或写入失败），
/// 自动投递、后台收尾、恢复补发与手动重发共用同一来源。
#[derive(Clone)]
pub(crate) struct DeliveryResultFeed(broadcast::Sender<DeliveryResult>);

impl DeliveryResultFeed {
    pub(crate) fn new() -> Self {
        Self(broadcast::channel(DELIVERY_RESULT_CAPACITY).0)
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<DeliveryResult> {
        self.0.subscribe()
    }

    fn publish(&self, entry_id: &EntryId, target_device_id: DeviceId, kind: DeliveryResultKind) {
        // 没有订阅者只说明恢复负责人尚未启动或已停止；启动扫描会按持久状态再处理。
        let _ = self.0.send(DeliveryResult {
            entry_id: entry_id.clone(),
            target_device_id,
            kind,
        });
    }
}

/// 已落库状态对自动恢复的含义；`Pending` 只由意图写入产生，按未结束处理。
fn persisted_result_kind(status: &EntryDeliveryStatus) -> DeliveryResultKind {
    match status {
        EntryDeliveryStatus::Pending | EntryDeliveryStatus::Unreachable => {
            DeliveryResultKind::Recoverable
        }
        EntryDeliveryStatus::Delivered
        | EntryDeliveryStatus::Duplicate
        | EntryDeliveryStatus::Superseded
        | EntryDeliveryStatus::Failed { .. } => DeliveryResultKind::Settled,
    }
}

/// Persists per-peer delivery outcomes and pings the frontend to refetch.
pub(crate) struct DeliveryRecorder {
    entry_delivery_repo: Arc<dyn EntryDeliveryRepositoryPort>,
    host_event_bus: SharedHostEventEmitter,
    current: Arc<Mutex<HashMap<(EntryId, DeviceId), Weak<()>>>>,
    attempt: Option<Arc<()>>,
    results: DeliveryResultFeed,
}

impl DeliveryRecorder {
    /// 每个完整发送持有自己的尝试；同目标新尝试取代旧结果写入权，不取消远端提交。
    pub(super) async fn begin_attempt(self: &Arc<Self>) -> Arc<Self> {
        let attempt = Arc::new(());
        let mut current = self.current.lock().await;
        current.retain(|_, token| token.strong_count() > 0);
        Arc::new(Self {
            entry_delivery_repo: self.entry_delivery_repo.clone(),
            host_event_bus: self.host_event_bus.clone(),
            current: self.current.clone(),
            attempt: Some(attempt),
            results: self.results.clone(),
        })
    }

    pub(crate) fn delivery_results(&self) -> DeliveryResultFeed {
        self.results.clone()
    }
    fn is_current(
        &self,
        current: &HashMap<(EntryId, DeviceId), Weak<()>>,
        entry: &EntryId,
        target: DeviceId,
    ) -> bool {
        self.attempt.as_ref().is_none_or(|attempt| {
            current
                .get(&(entry.clone(), target))
                .and_then(Weak::upgrade)
                .is_some_and(|active| Arc::ptr_eq(attempt, &active))
        })
    }
    pub(super) async fn record_pending(
        &self,
        entry_id: &EntryId,
        target_device_id: DeviceId,
        now_ms: i64,
    ) -> Result<(), uc_core::clipboard::EntryDeliveryError> {
        let mut current = self.current.lock().await;
        if let Err(error) = self
            .entry_delivery_repo
            .record_attempt(&EntryDeliveryRecord {
                entry_id: entry_id.clone(),
                target_device_id,
                status: EntryDeliveryStatus::Pending,
                reason_detail: None,
                updated_at_ms: now_ms,
            })
            .await
        {
            // 本次不会发送；此前已持久的待恢复状态仍需恢复负责人按退避继续承担。
            self.results
                .publish(entry_id, target_device_id, DeliveryResultKind::NotPersisted);
            return Err(error);
        }
        // 意图提交成功后才转交结果写入权；失败时原在途尝试仍可结算。
        if let Some(attempt) = &self.attempt {
            current.insert(
                (entry_id.clone(), target_device_id),
                Arc::downgrade(attempt),
            );
        }
        Ok(())
    }

    pub(super) async fn flush_owned(
        self: &Arc<Self>,
        work: OwnedWork,
        records: Vec<EntryDeliveryRecord>,
    ) {
        if records.is_empty() {
            return;
        }
        let recorder = Arc::clone(self);
        let (finished, completion) = oneshot::channel();
        let observation = ObservationContext::capture();
        work.spawn(
            DiagnosticTaskKind::ClipboardDeliveryRecord,
            observation.scope(
                async move {
                    recorder.flush(&records).await;
                    let _ = finished.send(());
                }
                .in_current_span(),
            ),
        );
        // 异常由工作负责人保存；已收到的对端回执不因记录失败改写。
        let _ = completion.await;
    }

    pub(crate) fn new(
        entry_delivery_repo: Arc<dyn EntryDeliveryRepositoryPort>,
        host_event_bus: SharedHostEventEmitter,
    ) -> Self {
        Self {
            entry_delivery_repo,
            host_event_bus,
            current: Arc::new(Mutex::new(HashMap::new())),
            attempt: None,
            results: DeliveryResultFeed::new(),
        }
    }

    /// Sequentially `record_attempt` each record then `emit_or_warn` the
    /// matching refetch ping. Write-then-emit order is load-bearing — the
    /// host event carries no payload, so the frontend's follow-up read must
    /// observe the write.
    ///
    /// 写入失败不改写发送方已得到的对端结果，但必须交给恢复负责人：持久状态仍是此前的
    /// `Pending`/`Unreachable`，只有通知才能让它按退避继续承担，而不是永远停在未结算状态。
    pub(crate) async fn flush(&self, records: &[EntryDeliveryRecord]) {
        for record in records {
            // 与新尝试登记共用写锁，检查及落库不可被另一尝试穿插。
            let current = self.current.lock().await;
            if !self.is_current(&current, &record.entry_id, record.target_device_id) {
                continue;
            }
            if let Err(err) = self.entry_delivery_repo.record_attempt(record).await {
                uc_warn!(
                    error_kind = "delivery_record",
                    io_error_kind = io_error_kind(&err),
                    entry_id = log_id(&record.entry_id),
                    "failed to record entry delivery"
                );
                self.results.publish(
                    &record.entry_id,
                    record.target_device_id,
                    DeliveryResultKind::NotPersisted,
                );
                continue;
            }
            self.host_event_bus.emit_or_warn(HostEvent::Delivery(
                DeliveryHostEvent::StatusChanged {
                    entry_id: record.entry_id.to_string(),
                    target_device_id: record.target_device_id.as_str().to_string(),
                },
            ));
            self.results.publish(
                &record.entry_id,
                record.target_device_id,
                persisted_result_kind(&record.status),
            );
        }
    }
}

/// 前台期限后继续等待已发出的对端结果，每条完成即记录并通知，不重试发送。
/// 工作许可归同一发送负责人；暂停必须等待这里的记录与异常处理结束。
pub(super) fn spawn_deferred_drain(
    work: OwnedWork,
    mut set: JoinSet<PeerDispatchResult>,
    entry_id: Option<EntryId>,
    clock: Arc<dyn ClockPort>,
    recorder: Arc<DeliveryRecorder>,
) {
    let deferred_count = set.len();
    let observation = ObservationContext::capture();
    work.spawn(
        DiagnosticTaskKind::ClipboardDeferredDrain,
        observation.scope(async move {
            let mut accepted = 0usize;
            let mut duplicate = 0usize;
            let mut offline = 0usize;
            let mut errored = 0usize;
            while let Some(joined) = set.join_next().await {
                let processed = classify_dispatch_result(joined, entry_id.as_ref(), clock.now_ms());
                match processed.bucket {
                    DispatchResultBucket::Accepted => accepted += 1,
                    DispatchResultBucket::Duplicate => duplicate += 1,
                    DispatchResultBucket::Offline => offline += 1,
                    DispatchResultBucket::Errored | DispatchResultBucket::Panicked => errored += 1,
                }
                if let Some(rec) = processed.delivery_record {
                    recorder.flush(std::slice::from_ref(&rec)).await;
                }
            }
            uc_info!(
                deferred_count = deferred_count,
                accepted = accepted,
                duplicate = duplicate,
                offline = offline,
                errored = errored,
                "dispatch: deferred fan-out completed"
            );
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::super::super::work::WorkOwner;
    use super::*;

    #[tokio::test]
    async fn deferred_delivery_does_not_keep_the_returned_operation_open() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use tracing_subscriber::{layer::SubscriberExt, registry::LookupSpan, Layer};
        struct Closed(Arc<AtomicBool>);
        impl<S: tracing::Subscriber + for<'a> LookupSpan<'a>> Layer<S> for Closed {
            fn on_close(
                &self,
                id: tracing::Id,
                context: tracing_subscriber::layer::Context<'_, S>,
            ) {
                if context
                    .span(&id)
                    .is_some_and(|span| span.name() == "foreground")
                {
                    self.0.store(true, Ordering::SeqCst);
                }
            }
        }
        struct Repo(Arc<tokio::sync::Notify>);
        #[async_trait::async_trait]
        impl EntryDeliveryRepositoryPort for Repo {
            async fn record_attempt(
                &self,
                _: &EntryDeliveryRecord,
            ) -> Result<(), uc_core::clipboard::EntryDeliveryError> {
                self.0.notify_one();
                Ok(())
            }
            async fn list_by_entry(
                &self,
                _: &EntryId,
            ) -> Result<Vec<EntryDeliveryRecord>, uc_core::clipboard::EntryDeliveryError>
            {
                Ok(vec![])
            }
        }
        let closed = Arc::new(AtomicBool::new(false));
        let _subscriber = tracing::subscriber::set_default(
            tracing_subscriber::registry().with(Closed(closed.clone())),
        );
        let settled = Arc::new(tokio::sync::Notify::new());
        let recorder = Arc::new(DeliveryRecorder::new(
            Arc::new(Repo(settled.clone())),
            Arc::new(crate::facade::HostEventBus::new()),
        ));
        let (release, wait) = tokio::sync::oneshot::channel();
        let mut tasks = JoinSet::new();
        tasks.spawn(async move {
            let _ = wait.await;
            (dev("peer"), Ok(DispatchAck::Accepted))
        });
        let root = tracing::info_span!("foreground");
        let owner = WorkOwner::default();
        root.in_scope(|| {
            spawn_deferred_drain(
                owner.begin().unwrap(),
                tasks,
                Some(eid()),
                Arc::new(super::super::test_support::FixedClock(0)),
                recorder,
            )
        });
        drop(root);
        let closed_before_settlement = closed.load(Ordering::SeqCst);
        release.send(()).expect("release delivery");
        tokio::time::timeout(std::time::Duration::from_secs(1), settled.notified())
            .await
            .expect("delivery is still recorded");
        assert!(
            closed_before_settlement,
            "background settlement must not extend the returned operation"
        );
    }
    /// 旧在途结果不能覆盖后来恢复成功，但未被新尝试覆盖的目标仍要结算。
    #[tokio::test]
    async fn older_deferred_attempt_cannot_overwrite_newer_delivery_for_same_target() {
        #[derive(Default)]
        struct Repo(tokio::sync::Mutex<Vec<EntryDeliveryRecord>>);
        #[async_trait::async_trait]
        impl EntryDeliveryRepositoryPort for Repo {
            async fn record_attempt(
                &self,
                record: &EntryDeliveryRecord,
            ) -> Result<(), uc_core::clipboard::EntryDeliveryError> {
                if record.updated_at_ms == 99 {
                    return Err(uc_core::clipboard::EntryDeliveryError::InvalidStatus);
                }
                let mut rows = self.0.lock().await;
                rows.retain(|row| {
                    row.entry_id != record.entry_id
                        || row.target_device_id != record.target_device_id
                });
                rows.push(record.clone());
                Ok(())
            }
            async fn list_by_entry(
                &self,
                _: &EntryId,
            ) -> Result<Vec<EntryDeliveryRecord>, uc_core::clipboard::EntryDeliveryError>
            {
                Ok(self.0.lock().await.clone())
            }
        }
        let repo = Arc::new(Repo::default());
        let owner = Arc::new(DeliveryRecorder::new(
            repo.clone(),
            Arc::new(crate::facade::HostEventBus::new()),
        ));
        let entry = eid();
        let first = dev("peer-one");
        let other = dev("peer-two");
        let old = owner.begin_attempt().await;
        old.record_pending(&entry, first, 0).await.unwrap();
        old.record_pending(&entry, other, 0).await.unwrap();
        let new = owner.begin_attempt().await;
        new.record_pending(&entry, first, 1).await.unwrap();
        let record = |target, status| EntryDeliveryRecord {
            entry_id: entry.clone(),
            target_device_id: target,
            status,
            reason_detail: None,
            updated_at_ms: 0,
        };
        new.flush(&[record(first, EntryDeliveryStatus::Delivered)])
            .await;
        old.flush(&[
            record(first, EntryDeliveryStatus::Unreachable),
            record(other, EntryDeliveryStatus::Delivered),
        ])
        .await;
        let before_failed_intent = repo.list_by_entry(&entry).await.unwrap();
        assert_eq!(
            before_failed_intent
                .iter()
                .find(|row| row.target_device_id == first)
                .unwrap()
                .status,
            EntryDeliveryStatus::Delivered
        );
        let failed = owner.begin_attempt().await;
        assert!(failed.record_pending(&entry, first, 99).await.is_err());
        // 新意图写入失败时，现有尝试仍有权保存自己的最后回执。
        new.flush(&[record(first, EntryDeliveryStatus::Duplicate)])
            .await;
        let rows = repo.list_by_entry(&entry).await.unwrap();
        assert_eq!(
            rows.iter()
                .find(|row| row.target_device_id == first)
                .unwrap()
                .status,
            EntryDeliveryStatus::Duplicate
        );
        assert_eq!(
            rows.iter()
                .find(|row| row.target_device_id == other)
                .unwrap()
                .status,
            EntryDeliveryStatus::Delivered
        );
    }

    use uc_core::ids::DeviceId;

    fn dev(id: &str) -> DeviceId {
        DeviceId::new(id)
    }

    fn eid() -> EntryId {
        EntryId::from("entry-1".to_string())
    }

    #[test]
    fn accepted_maps_to_delivered_record_and_accepted_bucket() {
        let joined = Ok((dev("peer-a"), Ok(DispatchAck::Accepted)));
        let p = classify_dispatch_result(joined, Some(&eid()), 42);
        assert!(matches!(p.bucket, DispatchResultBucket::Accepted));
        let pt = p.per_target.expect("per_target present");
        assert_eq!(pt.device_id, dev("peer-a"));
        assert!(matches!(pt.outcome, Ok(DispatchAck::Accepted)));
        let rec = p.delivery_record.expect("delivery record present");
        assert!(matches!(rec.status, EntryDeliveryStatus::Delivered));
        assert_eq!(rec.updated_at_ms, 42);
        assert_eq!(rec.reason_detail, None);
    }

    #[test]
    fn duplicate_maps_to_duplicate_record_and_bucket() {
        let joined = Ok((dev("peer-a"), Ok(DispatchAck::DuplicateIgnored)));
        let p = classify_dispatch_result(joined, Some(&eid()), 1);
        assert!(matches!(p.bucket, DispatchResultBucket::Duplicate));
        assert!(matches!(
            p.per_target.unwrap().outcome,
            Ok(DispatchAck::DuplicateIgnored)
        ));
        assert!(matches!(
            p.delivery_record.unwrap().status,
            EntryDeliveryStatus::Duplicate
        ));
    }

    #[test]
    fn offline_maps_to_unreachable_record_and_offline_bucket() {
        let joined = Ok((dev("peer-a"), Err(ClipboardDispatchError::Offline)));
        let p = classify_dispatch_result(joined, Some(&eid()), 1);
        assert!(matches!(p.bucket, DispatchResultBucket::Offline));
        assert!(matches!(p.per_target.unwrap().outcome, Err(ref s) if s == "offline"));
        assert!(matches!(
            p.delivery_record.unwrap().status,
            EntryDeliveryStatus::Unreachable,
        ));
    }

    #[test]
    fn transient_failure_maps_to_recoverable_unreachable_record() {
        let joined = Ok((
            dev("peer-a"),
            Err(ClipboardDispatchError::Transient(Box::new(
                std::io::Error::from(std::io::ErrorKind::ConnectionReset),
            ))),
        ));
        let p = classify_dispatch_result(joined, Some(&eid()), 1);
        assert!(matches!(p.bucket, DispatchResultBucket::Offline));
        assert!(p.per_target.unwrap().outcome.is_err());
        let rec = p.delivery_record.unwrap();
        assert_eq!(rec.status, EntryDeliveryStatus::Unreachable);
        assert_eq!(rec.reason_detail, None);
    }

    /// 策略拒绝、协议不兼容、不可重试的连接失败与本机内部错误都保持终态。
    #[test]
    fn non_retryable_failures_stay_terminal() {
        for (error, reason) in [
            (
                ClipboardDispatchError::LocalPolicyExceeded("too big".into()),
                DeliveryFailureReason::LocalPolicy,
            ),
            (
                ClipboardDispatchError::PeerRejected("bad header".into()),
                DeliveryFailureReason::PeerRejected,
            ),
            (
                ClipboardDispatchError::PeerIncompatible,
                DeliveryFailureReason::PeerIncompatible,
            ),
            (
                ClipboardDispatchError::Io("owner stopped".into()),
                DeliveryFailureReason::Io,
            ),
            (
                ClipboardDispatchError::Internal("corrupt".into()),
                DeliveryFailureReason::Internal,
            ),
        ] {
            let p = classify_dispatch_result(Ok((dev("peer-a"), Err(error))), Some(&eid()), 1);
            assert!(matches!(p.bucket, DispatchResultBucket::Errored));
            assert_eq!(
                p.delivery_record.unwrap().status,
                EntryDeliveryStatus::Failed { reason }
            );
        }
    }

    /// 每条结果写入结束都通知恢复负责人，并区分可恢复、已结算与未能落库三类；
    /// 写入失败的最终结果（无论原状态）按未落库通知，恢复负责人据此继续承担。
    #[tokio::test]
    async fn every_result_write_is_announced_with_its_recovery_meaning() {
        struct Repo;
        #[async_trait::async_trait]
        impl EntryDeliveryRepositoryPort for Repo {
            async fn record_attempt(
                &self,
                record: &EntryDeliveryRecord,
            ) -> Result<(), uc_core::clipboard::EntryDeliveryError> {
                if record.updated_at_ms == 99 {
                    return Err(uc_core::clipboard::EntryDeliveryError::InvalidStatus);
                }
                Ok(())
            }
            async fn list_by_entry(
                &self,
                _: &EntryId,
            ) -> Result<Vec<EntryDeliveryRecord>, uc_core::clipboard::EntryDeliveryError>
            {
                Ok(vec![])
            }
        }
        let recorder =
            DeliveryRecorder::new(Arc::new(Repo), Arc::new(crate::facade::HostEventBus::new()));
        let mut results = recorder.delivery_results().subscribe();
        let record = |target: &str, status, updated_at_ms| EntryDeliveryRecord {
            entry_id: eid(),
            target_device_id: dev(target),
            status,
            reason_detail: None,
            updated_at_ms,
        };
        recorder
            .flush(&[
                record("delivered", EntryDeliveryStatus::Delivered, 1),
                record(
                    "failed",
                    EntryDeliveryStatus::Failed {
                        reason: DeliveryFailureReason::PeerRejected,
                    },
                    1,
                ),
                record("not-persisted", EntryDeliveryStatus::Unreachable, 99),
                record(
                    "delivered-not-persisted",
                    EntryDeliveryStatus::Delivered,
                    99,
                ),
                record("unreachable", EntryDeliveryStatus::Unreachable, 1),
            ])
            .await;

        let announced: Vec<_> = std::iter::from_fn(|| results.try_recv().ok())
            .map(|result| (result.target_device_id, result.kind))
            .collect();
        assert_eq!(
            announced,
            vec![
                (dev("delivered"), DeliveryResultKind::Settled),
                (dev("failed"), DeliveryResultKind::Settled),
                (dev("not-persisted"), DeliveryResultKind::NotPersisted),
                (
                    dev("delivered-not-persisted"),
                    DeliveryResultKind::NotPersisted
                ),
                (dev("unreachable"), DeliveryResultKind::Recoverable),
            ]
        );
    }

    /// 意图写入失败时本次不发送，但要通知恢复负责人：此前的持久待恢复状态仍需继续承担。
    #[tokio::test]
    async fn a_failed_intent_write_is_announced_as_not_persisted() {
        struct Repo;
        #[async_trait::async_trait]
        impl EntryDeliveryRepositoryPort for Repo {
            async fn record_attempt(
                &self,
                _: &EntryDeliveryRecord,
            ) -> Result<(), uc_core::clipboard::EntryDeliveryError> {
                Err(uc_core::clipboard::EntryDeliveryError::InvalidStatus)
            }
            async fn list_by_entry(
                &self,
                _: &EntryId,
            ) -> Result<Vec<EntryDeliveryRecord>, uc_core::clipboard::EntryDeliveryError>
            {
                Ok(vec![])
            }
        }
        let recorder = Arc::new(DeliveryRecorder::new(
            Arc::new(Repo),
            Arc::new(crate::facade::HostEventBus::new()),
        ));
        let mut results = recorder.delivery_results().subscribe();
        let attempt = recorder.begin_attempt().await;

        assert!(attempt
            .record_pending(&eid(), dev("peer"), 1)
            .await
            .is_err());

        assert_eq!(
            results.try_recv().unwrap(),
            DeliveryResult {
                entry_id: eid(),
                target_device_id: dev("peer"),
                kind: DeliveryResultKind::NotPersisted,
            }
        );
        assert!(results.try_recv().is_err());
    }

    #[test]
    fn non_offline_error_maps_to_errored_with_reason_detail() {
        let joined = Ok((
            dev("peer-a"),
            Err(ClipboardDispatchError::PeerRejected("nope".to_string())),
        ));
        let p = classify_dispatch_result(joined, Some(&eid()), 1);
        assert!(matches!(p.bucket, DispatchResultBucket::Errored));
        let rec = p.delivery_record.unwrap();
        assert!(matches!(
            rec.status,
            EntryDeliveryStatus::Failed {
                reason: DeliveryFailureReason::PeerRejected
            }
        ));
        assert_eq!(rec.reason_detail, Some("nope".to_string()));
    }

    #[test]
    fn incompatible_peer_maps_to_a_distinct_delivery_failure() {
        let joined = Ok((
            dev("peer-old"),
            Err(ClipboardDispatchError::PeerIncompatible),
        ));

        let processed = classify_dispatch_result(joined, Some(&eid()), 7);
        let record = processed.delivery_record.expect("delivery record");

        assert!(matches!(
            record.status,
            EntryDeliveryStatus::Failed {
                reason: DeliveryFailureReason::PeerIncompatible
            }
        ));
    }

    #[test]
    fn no_entry_id_yields_per_target_but_no_delivery_record() {
        let joined = Ok((dev("peer-a"), Ok(DispatchAck::Accepted)));
        let p = classify_dispatch_result(joined, None, 1);
        assert!(p.per_target.is_some());
        assert!(p.delivery_record.is_none());
    }

    #[tokio::test]
    async fn panicked_task_maps_to_panicked_bucket_with_no_target_or_record() {
        // A real JoinError is only obtainable from a JoinSet, so drive a
        // panicking task through to settle, then classify its Err(JoinError).
        let mut set: JoinSet<PeerDispatchResult> = JoinSet::new();
        set.spawn(async { panic!("boom") });
        let joined = set.join_next().await.expect("one task settled");
        assert!(joined.is_err(), "panicked task must surface as JoinError");
        let p = classify_dispatch_result(joined, Some(&eid()), 1);
        assert!(matches!(p.bucket, DispatchResultBucket::Panicked));
        assert!(p.per_target.is_none());
        assert!(p.delivery_record.is_none());
    }
}
