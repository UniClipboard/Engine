use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifecycleTarget {
    Active,
    Suspended,
}

/// 同一次转换及其回收始终使用相同上下文；无预算入口使用 None。
#[derive(Clone)]
pub struct TransitionContext {
    generation: u64,
    deadline: Option<Instant>,
    cancellation: CancellationToken,
    final_stop: bool,
}

impl TransitionContext {
    pub(super) fn new(generation: u64, deadline: Option<Instant>) -> Self {
        Self {
            generation,
            deadline,
            cancellation: CancellationToken::new(),
            final_stop: false,
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    /// 本次暂停是不可恢复的最终停止：随后由重置或关闭负责人继续使用并最终释放本地存储，
    /// 参与者不应为将来的恢复而关闭这些存储。
    pub fn is_final_stop(&self) -> bool {
        self.final_stop
    }

    pub(super) fn with_final_stop(mut self, final_stop: bool) -> Self {
        self.final_stop = final_stop;
        self
    }

    pub(super) fn with_cancellation(mut self, cancellation: CancellationToken) -> Self {
        self.cancellation = cancellation;
        self
    }

    pub(super) fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }
}
