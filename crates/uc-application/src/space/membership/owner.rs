//! 成员事实的唯一写入者。
//!
//! Owner 加载并校验成员记录，把输入交给 Core 成员账本推进，在一次条件提交中保存记录与成员读模型，
//! 然后发布新的只读状态并履行账本声明的保存后效果（发布设备信任变化、唤醒执行器）。其他用例、
//! 查询、准入和 Infra 都只能经 Owner 改写成员记录。

mod draft;
#[cfg(test)]
mod tests;
mod view;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use uc_core::ids::DeviceId;
use uc_core::membership::{
    HistoricalMembershipSignatureVerifier, LedgerFollowUp, LedgerTransitionError,
    LedgerTransitionErrorCategory,
};
use uc_core::ports::{ClockPort, HostEvent, HostEventEmitterPort, MembershipHostEvent};

use super::{
    CurrentSpaceMemberScope, CurrentSpaceMemberScopeError, CurrentSpaceMemberScopePort,
    MembershipLedgerError, MembershipProjectionPlan, MembershipRecordCommit,
    MembershipRecordStorePort, RetainedGroupUpdateRecipientsPort,
    WakeSpaceMembershipMaintenancePort,
};
use crate::space::lifecycle::{SpaceMembershipRebuildError, SpaceMembershipResetPort};

pub(crate) use draft::MembershipDraft;
pub(crate) use view::{pause_reason, MembershipView};

pub(crate) struct MembershipOwner {
    store: Arc<dyn MembershipRecordStorePort>,
    verifier: Arc<dyn HistoricalMembershipSignatureVerifier>,
    clock: Arc<dyn ClockPort>,
    host_events: Arc<dyn HostEventEmitterPort>,
    worker_wake: Arc<dyn WakeSpaceMembershipMaintenancePort>,
    published: Mutex<Option<Arc<MembershipView>>>,
    operation: tokio::sync::Mutex<()>,
    changes: tokio::sync::watch::Sender<()>,
}

/// 一次提交的结果：提交后（或未变化时的当前）状态与调用方闭包的输出。
pub(crate) struct MembershipCommitted<T> {
    pub(crate) view: Arc<MembershipView>,
    pub(crate) output: T,
}

impl MembershipOwner {
    pub(crate) fn new(
        store: Arc<dyn MembershipRecordStorePort>,
        verifier: Arc<dyn HistoricalMembershipSignatureVerifier>,
        clock: Arc<dyn ClockPort>,
        host_events: Arc<dyn HostEventEmitterPort>,
        worker_wake: Arc<dyn WakeSpaceMembershipMaintenancePort>,
    ) -> Self {
        Self {
            store,
            verifier,
            clock,
            host_events,
            worker_wake,
            published: Mutex::new(None),
            operation: tokio::sync::Mutex::new(()),
            changes: tokio::sync::watch::channel(()).0,
        }
    }

    pub(crate) fn verifier(&self) -> &dyn HistoricalMembershipSignatureVerifier {
        self.verifier.as_ref()
    }

    pub(crate) fn verifier_handle(&self) -> Arc<dyn HistoricalMembershipSignatureVerifier> {
        Arc::clone(&self.verifier)
    }

    pub(crate) fn now_ms(&self) -> i64 {
        self.clock.now_ms()
    }

    /// 当前已发布的成员状态。首次读取或提交失败后从持久记录重新加载并校验。
    pub(crate) async fn load(&self) -> Result<Arc<MembershipView>, MembershipLedgerError> {
        if let Some(view) = self.published()? {
            return Ok(view);
        }
        let _operation = self.operation.lock().await;
        self.load_exclusive().await
    }

    /// 在最新状态上执行 `change` 并条件提交；`change` 不产生变化时不写入。
    pub(crate) async fn commit<T>(
        &self,
        change: impl FnOnce(&mut MembershipDraft) -> Result<T, MembershipLedgerError>,
    ) -> Result<MembershipCommitted<T>, MembershipLedgerError> {
        let _operation = self.operation.lock().await;
        let current = self.load_exclusive().await?;
        let mut draft = MembershipDraft::new(Arc::clone(&current), self.clock.now_ms());
        let output = change(&mut draft)?;
        let Some(finished) = draft.finish()? else {
            return Ok(MembershipCommitted {
                view: current,
                output,
            });
        };
        let projection = finished
            .view
            .space()
            .map(|space| MembershipProjectionPlan::from_ledger(space.ledger()))
            .transpose()?;
        let committed = self
            .store
            .commit(MembershipRecordCommit {
                expected_revision: current.revision(),
                replacement: finished.view.to_record(),
                projection,
            })
            .await;
        if let Err(error) = committed {
            self.forget()?;
            return Err(error);
        }
        let view = Arc::new(finished.view);
        self.publish(Arc::clone(&view))?;
        self.changes.send_replace(());
        for follow_up in finished.follow_ups {
            match follow_up {
                LedgerFollowUp::PublishDeviceTrustChange => {
                    if self
                        .host_events
                        .emit(HostEvent::Membership(
                            MembershipHostEvent::LedgerCommitted {
                                revision: view.revision(),
                            },
                        ))
                        .is_err()
                    {
                        tracing::warn!("成员设备信任变化事件发布失败");
                    }
                }
                LedgerFollowUp::WakeWorker => self.worker_wake.wake(),
            }
        }
        Ok(MembershipCommitted { view, output })
    }

    /// 测试直接改写持久记录后丢弃已发布状态，使下一次读取重新加载。
    #[cfg(test)]
    pub(crate) fn reload_for_test(&self) {
        *self.published.lock().unwrap() = None;
    }

    pub(crate) fn schedule_worker_at(&self, due_at_ms: i64) {
        self.worker_wake.schedule_at(due_at_ms, self.clock.now_ms());
    }

    async fn load_exclusive(&self) -> Result<Arc<MembershipView>, MembershipLedgerError> {
        if let Some(view) = self.published()? {
            return Ok(view);
        }
        let record = self.store.load().await?;
        let view = Arc::new(MembershipView::from_record(record)?);
        self.publish(Arc::clone(&view))?;
        Ok(view)
    }

    fn published(&self) -> Result<Option<Arc<MembershipView>>, MembershipLedgerError> {
        Ok(self
            .published
            .lock()
            .map_err(|_| MembershipLedgerError::Unavailable)?
            .clone())
    }

    fn publish(&self, view: Arc<MembershipView>) -> Result<(), MembershipLedgerError> {
        *self
            .published
            .lock()
            .map_err(|_| MembershipLedgerError::Unavailable)? = Some(view);
        Ok(())
    }

    fn forget(&self) -> Result<(), MembershipLedgerError> {
        *self
            .published
            .lock()
            .map_err(|_| MembershipLedgerError::Unavailable)? = None;
        Ok(())
    }
}

/// 账本拒绝输入时，状态已被其他事实推进，调用方按冲突重新判定；无法推进时进入恢复。
pub(crate) fn ledger_error(error: LedgerTransitionError) -> MembershipLedgerError {
    match error.category() {
        LedgerTransitionErrorCategory::Rejected => MembershipLedgerError::Conflict,
        LedgerTransitionErrorCategory::RecoveryRequired => MembershipLedgerError::Corrupt,
    }
}

#[async_trait]
impl CurrentSpaceMemberScopePort for MembershipOwner {
    fn subscribe_changes(&self) -> tokio::sync::watch::Receiver<()> {
        self.changes.subscribe()
    }

    async fn snapshot(&self) -> Result<CurrentSpaceMemberScope, CurrentSpaceMemberScopeError> {
        let view = self.load().await?;
        view.current_scope()
    }
}

#[async_trait]
impl RetainedGroupUpdateRecipientsPort for MembershipOwner {
    /// 仍应接收设备组更新的收件人：当前生效成员中除本机以外的设备。成员状态不可读时返回 `None`，
    /// 调用方必须保持队列原样，不得按空名单清空。
    async fn retained_group_update_recipients(&self) -> Option<Vec<DeviceId>> {
        let view = self.load().await.ok()?;
        let space = view.space()?;
        let history = space.history();
        let mut recipients = Vec::new();
        for member in history.effective_members() {
            let facts = history.admission_facts_for(member)?;
            if &facts.device_id != space.local_device_id() {
                recipients.push(facts.device_id);
            }
        }
        Some(recipients)
    }
}

#[async_trait]
impl SpaceMembershipResetPort for MembershipOwner {
    async fn reset(&self) -> Result<(), SpaceMembershipRebuildError> {
        match self.commit(|draft| draft.clear_space()).await {
            Ok(_) => Ok(()),
            Err(MembershipLedgerError::Corrupt | MembershipLedgerError::RecoveryRequired) => {
                Err(SpaceMembershipRebuildError::Inconsistent)
            }
            Err(
                MembershipLedgerError::Locked
                | MembershipLedgerError::Conflict
                | MembershipLedgerError::Unavailable,
            ) => Err(SpaceMembershipRebuildError::Unavailable),
        }
    }
}
