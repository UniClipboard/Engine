//! 一个原预算内的完整快照读取；所有页通过前不返回部分投影。
mod model;
mod ports;
mod reader;
pub use model::{
    ProgressPageReply, ProgressPageRequest, ProgressSnapshot, ProgressSnapshotPage,
    ProgressSnapshotRead,
};
pub use ports::{ProgressSnapshotRoundPort, ProgressSnapshotWirePort};
pub use reader::TransferProgressSnapshotReader;
