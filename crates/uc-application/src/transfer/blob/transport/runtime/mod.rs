//! 完整 blob 连接与交换监督；运行实现、提供流程和关闭分别组织。
mod owner;
mod provide;
mod shutdown;

pub use owner::{BlobShutdownWait, BlobTransport};

#[cfg(test)]
mod tests;

mod download;
pub use download::BlobDownloadResources;
pub(in super::super::super) use download::PreparedDownload;

mod supervision;

pub(in super::super::super) use supervision::{DownloadCompletion, DownloadTask};
