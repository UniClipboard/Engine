//! 原接收负责人交接已接受的源身份与原始位置；不包含provider或文件路径。
use super::{ReceiveSource, ReceiveWorkRegistry, RegistryState};
use crate::transfer::progress::{
    ProgressClaimRefusal, ProgressOriginalPlan, ProgressSourceTask, ProgressTaskAuthorityPort,
    ProgressTaskDescriptor,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, PoisonError, Weak,
};
use uc_core::file_transfer::{ProgressIdentity, ProgressItem};
use uc_core::ids::{DeviceId, EntryId};
use uuid::Uuid;

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
