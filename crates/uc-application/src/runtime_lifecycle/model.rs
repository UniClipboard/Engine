use tokio::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifecycleTarget {
    Active,
    Suspended,
}

/// 同一次转换及其回收始终使用相同上下文；无预算入口使用 None。
pub struct TransitionContext {
    generation: u64,
    deadline: Option<Instant>,
}

impl TransitionContext {
    pub(super) fn new(generation: u64, deadline: Option<Instant>) -> Self {
        Self {
            generation,
            deadline,
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }
}
