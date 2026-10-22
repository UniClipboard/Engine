//! 一次条目取消的完整负责人；五秒只限制调用等待，旧尝试的实际收尾继续执行。
mod error;
mod model;
mod owner;
mod ports;
mod workflow;
pub use error::CancelEntryReceiveError;
pub use model::CancelEntryReceiveOutcome;
pub(crate) use owner::{CancelEntryReceiveUseCase, ReceiveCancellationJobs};
pub(crate) use ports::AttemptTransferCancellation;
#[cfg(test)]
mod tests;
