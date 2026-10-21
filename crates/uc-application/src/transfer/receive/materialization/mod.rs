//! 接收计划的唯一物化负责人；调用方只交接完整计划并接收聚合结果。

mod model;
mod ports;
mod workflow;

pub use model::{
    InboundFileSetManifest, InboundFileSetMember, MaterializeOutcome, MaterializeResult,
    MissingFileRef, ReceiveWorkPlan,
};
pub use ports::{InboundBlobFetcher, InboundBlobMaterializer};
pub(crate) use workflow::{
    compute_file_set_component, is_directory_cancel_error, verify_file_set_identity,
};
pub use workflow::{
    sweep_inbound_staging, DirectoryPublication, FileCacheBlobMaterializer, RollbackOutcome,
};
