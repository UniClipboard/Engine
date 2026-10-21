//! 完整 blob 交换的专属能力；连接、停止监督与调度由 transfer 负责人组合。
mod error;
mod ports;
mod runtime;

pub use error::{BlobExchangeError, BlobExchangeFailure};
pub use ports::{
    BlobExchangePort, BlobReadStorePort, BlobReceived, BlobRetainedContent, BlobSource,
    BlobSourcePort, BlobStoreLifecyclePort, BlobValidatedProgress,
};

pub use runtime::{BlobDownloadResources, BlobShutdownWait, BlobTransport};

pub(in super::super) use runtime::PreparedDownload;

pub(in super::super) use runtime::{DownloadCompletion, DownloadTask};
