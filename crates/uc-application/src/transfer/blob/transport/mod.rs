//! 完整 blob 交换的专属能力；连接、停止监督与调度由 transfer 负责人组合。
mod error;
mod ports;

pub use error::{BlobExchangeError, BlobExchangeFailure};
pub use ports::{BlobExchangePort, BlobReceived, BlobRetainedContent, BlobValidatedProgress};
