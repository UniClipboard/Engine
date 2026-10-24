use uc_core::ids::EntryId;

/// 已有取消协调者仍在真实收尾时，重试不能创建新尝试。
pub(crate) trait ReceiveCancellationBarrier: Send + Sync {
    fn is_settling(&self, entry: &EntryId, attempt: &str) -> bool;
}
