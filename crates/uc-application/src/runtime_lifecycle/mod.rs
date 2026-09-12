//! 运行生命周期的完整负责人；参与者合同、转换执行和失败报告分别维护。

mod coordinator;
mod error;
mod model;
mod ports;

pub use coordinator::RuntimeLifecycleCoordinator;
pub use error::LifecycleError;
pub use model::{LifecycleTarget, TransitionContext};
pub use ports::{RuntimeLifecycleParticipants, RuntimeLifecyclePort};

#[cfg(test)]
mod tests;
