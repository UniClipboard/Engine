//! 接收计划的唯一物化负责人；调用方只交接完整计划并接收聚合结果。

mod cleanup;
mod directory_publication;
mod model;
mod output;
mod ports;
mod progress;
pub(crate) use progress::{FileProgressCommitError, ReceiveFailureSettlementError};
mod reuse;
mod workflow;
pub(crate) use cleanup::{remove_uncommitted_output, OutputCleanupFailure};
pub(crate) use reuse::{validate_selection, ReceiveReuseRefusal};

pub(crate) use model::{receive_member_totals, ReceiveSelection, ReceivedItem, ReceivedOutput};
pub use model::{
    InboundFileSetManifest, InboundFileSetMember, MaterializeOutcome, MaterializeResult,
    MissingFileRef, ReceiveWorkPlan,
};
pub use ports::{InboundBlobFetcher, InboundBlobMaterializer};
pub(crate) use ports::{ReceiveFileProgress, ReceiveOutputFailure};
pub(crate) use workflow::{
    compute_file_set_component, is_directory_cancel_error, verify_file_set_identity,
};
pub use workflow::{sweep_inbound_staging, FileCacheBlobMaterializer};

pub use directory_publication::DirectoryPublication;
