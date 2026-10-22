//! Blob 传输门面对外再导出(ADR-018 阶段 5)。

pub use crate::transfer::blob::facade::{
    BatchPosition, BlobOutputTarget, BlobTransferError, BlobTransferFacade, FetchBlobCommand,
    FetchBlobResult, FetchBlobToPathCommand, FetchBlobToPathResult, FetchTransferContext,
    InboundCancelOutcome, PublishBlobCommand, PublishBlobPathCommand, PublishBlobResult,
    SharedHostEventEmitter,
};

// 测试支持只公开完整能力的被动装配参数；不提供内部步骤、状态或 SDK 访问。
#[cfg(feature = "test-support")]
pub use crate::transfer::blob::facade::BlobTransferDeps;
