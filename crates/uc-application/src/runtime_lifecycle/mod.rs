//! 运行生命周期的完整负责人；参与者合同、转换执行和失败报告分别维护。

mod coordinator;
mod error;
mod invocation;
mod model;
mod ports;

pub use coordinator::RuntimeLifecycle;
pub use error::LifecycleError;
pub(crate) use model::LifecycleTarget;
pub use model::TransitionContext;
pub(crate) use ports::RuntimeLifecycleParticipants;
pub use ports::RuntimeLifecyclePort;

#[cfg(test)]
mod tests;
