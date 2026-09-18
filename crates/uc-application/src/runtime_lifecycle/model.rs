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
}

impl TransitionContext {
    pub(super) fn new(generation: u64, deadline: Option<Instant>) -> Self {
        Self {
            generation,
            deadline,
            cancellation: CancellationToken::new(),
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    pub(super) fn with_cancellation(mut self, cancellation: CancellationToken) -> Self {
        self.cancellation = cancellation;
        self
    }

    pub(super) fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }
}
