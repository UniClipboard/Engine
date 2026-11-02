//! 原接收负责人交接已接受的源身份与原始位置；不包含provider或文件路径。
use super::{
    ReceiveAction, ReceiveRegistrationError, ReceiveSource, ReceiveWorkRegistry, RegistryState,
};
use crate::clipboard::sync::payload_codec::V3BlobRef;
use crate::transfer::progress::{
    ProgressClaimRefusal, ProgressOriginalPlan, ProgressSourceTask, ProgressTaskAuthorityPort,
    ProgressTaskDescriptor,
};
use crate::transfer::receive::materialization::{receive_member_totals, InboundFileSetManifest};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, PoisonError, Weak,
};
use uc_core::file_transfer::{
    ProgressIdentity, ProgressItem, ProgressItemKind, ProgressItemState, ProgressTerminal,
    ProgressTerminalOutcome,
};
use uc_core::ids::{DeviceId, EntryId};
use uuid::Uuid;

impl ReceiveAction {
    /// 入站负责人已核对完整可用性；只登记本次来源的成果，不创建持久下载尝试。
    pub(crate) fn accept_fully_held(
        &self,
        entry: &EntryId,
        peer: DeviceId,
        source_hint: Option<&EntryId>,
        blobs: &[V3BlobRef],
        manifest: Option<&InboundFileSetManifest>,
    ) -> Result<(), ReceiveRegistrationError> {
        let Some(plan) = ProgressOriginalPlan::from_refs(source_hint, blobs, manifest.is_some())
        else {
            return Ok(());
        };
        let mut state = self
            .registry
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if !state.accepting
            || state.clearing
            || state.clear_failed
            || self.generation.is_cancelled()
        {
            return Err(ReceiveRegistrationError::GateClosed);
        }
        if state.retired.get(entry).is_some_and(|births| {
            births
                .iter()
                .filter_map(Weak::upgrade)
                .any(|birth| Arc::ptr_eq(&birth, &self.birth))
        }) {
            return Err(ReceiveRegistrationError::EntryDeleted);
        }
        if state.cleaning.contains(entry.as_ref()) {
            return Err(ReceiveRegistrationError::CleanupInProgress);
        }
        // 每次实际投递建立新范围；发送端可能已经重启，不能复用已被旧进程确认的通知。
        let items = plan
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let total = match item.kind {
                    ProgressItemKind::File => Some(blobs[index].size_bytes),
                    ProgressItemKind::Representation => None,
                    ProgressItemKind::DirectoryUnit => {
                        Some(receive_member_totals(blobs, manifest).1)
                    }
                };
                ProgressItemState::Terminal(ProgressTerminal {
                    item: *item,
                    round: 0,
                    sequence: 0,
                    result_revision: 1,
                    bytes: total.unwrap_or(0),
                    total,
                    outcome: ProgressTerminalOutcome::Completed,
                })
            })
            .collect();
        let attempt = ProgressIdentity::new(*Uuid::new_v4().as_bytes());
        state
            .progress_catalog
            .accept(entry, peer, plan.source, attempt, plan.items);
        if let Some(current) = state.progress_catalog.current(&peer, plan.source) {
            state
                .progress_catalog
                .retain_states(&peer, current.descriptor, items);
        }
        drop(state);
        self.registry.changed.notify_waiters();
        Ok(())
    }
}

impl RegistryState {
    pub(super) fn accept_progress(
        &mut self,
        entry: &EntryId,
        attempt: &str,
        source: &ReceiveSource,
    ) -> Option<Arc<AtomicBool>> {
        let Some((source_id, attempt, items)) = source.progress_identity(attempt) else {
            // 新attempt无法可靠关联时退休旧映射；下载及本地结算继续由原负责人执行。
            self.progress_catalog.forget(entry);
            return None;
        };
        self.progress_catalog
            .accept(entry, source.from_device, source_id, attempt, items);
        self.progress_catalog
            .observe_changes(source.from_device, source_id, attempt)
    }
}
impl ReceiveSource {
    fn progress_identity(
        &self,
        attempt: &str,
    ) -> Option<(ProgressIdentity, ProgressIdentity, Vec<ProgressItem>)> {
        let plan = ProgressOriginalPlan::from_refs(
            self.source_entry.as_ref(),
            &self.blob_refs,
            self.manifest.is_some(),
        )?;
        // 无法表达的旧标识是关联拒绝，不是底层存储／网络错误。
        let attempt = Uuid::parse_str(attempt).ok()?;
        Some((
            plan.source,
            ProgressIdentity::new(*attempt.as_bytes()),
            plan.items,
        ))
    }
}
impl ProgressTaskAuthorityPort for ReceiveWorkRegistry {
    fn current_task(
        &self,
        peer: &DeviceId,
        source: ProgressIdentity,
    ) -> Option<ProgressSourceTask> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if !state.accepting || state.clearing || state.clear_failed {
            return None;
        }
        state.progress_catalog.current(peer, source)
    }

    fn claim_terminal(
        &self,
        peer: &DeviceId,
        expected: ProgressTaskDescriptor,
        item: ProgressItem,
    ) -> Result<ProgressSourceTask, ProgressClaimRefusal> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if !state.accepting || state.clearing || state.clear_failed {
            return Err(ProgressClaimRefusal::NotRetained);
        }
        state.progress_catalog.claim_terminal(peer, expected, item)
    }
}

/// 只发送变化机会，不携带网络或旧结果；退休catalog的旧信号不能令新任务变脏。
#[derive(Clone)]
pub(crate) struct ReceiveProgressSignal {
    changed: Arc<AtomicBool>,
    registry: Weak<ReceiveWorkRegistry>,
}
impl ReceiveProgressSignal {
    pub(super) fn new(registry: &Arc<ReceiveWorkRegistry>, changed: Arc<AtomicBool>) -> Self {
        Self {
            changed,
            registry: Arc::downgrade(registry),
        }
    }
    pub(crate) fn changed(&self) {
        self.changed.store(true, Ordering::Release);
        if let Some(registry) = self.registry.upgrade() {
            registry.changed.notify_waiters();
        }
    }
}
