//! Blob 传输门面对外再导出(ADR-018 阶段 5)。

pub use crate::transfer::blob::facade::{
    BatchPosition, BlobTransferDeps, BlobTransferError, BlobTransferFacade, FetchBlobCommand,
    FetchBlobResult, FetchBlobToPathCommand, FetchBlobToPathResult, FetchTransferContext,
    InboundCancelOutcome, PublishBlobCommand, PublishBlobPathCommand, PublishBlobResult,
    SharedHostEventEmitter,
};
