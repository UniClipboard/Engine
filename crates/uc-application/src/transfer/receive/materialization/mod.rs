//! 接收计划的唯一物化负责人；调用方只交接完整计划并接收聚合结果。

mod cleanup;
mod model;
mod output;
mod ports;
mod reuse;
mod workflow;
pub(crate) use cleanup::OutputCleanupFailure;
pub(crate) use reuse::{validate_selection, ReceiveReuseRefusal};

pub use model::{
    InboundFileSetManifest, InboundFileSetMember, MaterializeOutcome, MaterializeResult,
    MissingFileRef, ReceiveWorkPlan,
};
pub(crate) use model::{ReceiveSelection, ReceivedItem, ReceivedOutput};
pub use ports::{InboundBlobFetcher, InboundBlobMaterializer};
pub(crate) use workflow::{
    compute_file_set_component, is_directory_cancel_error, verify_file_set_identity,
};
pub use workflow::{
    sweep_inbound_staging, DirectoryPublication, FileCacheBlobMaterializer, RollbackOutcome,
};
