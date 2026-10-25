//! 反向进度的完整业务负责人；原始发送意图授权，登记、活动、联系及终态采用共享同一投影。
use crate::connectivity::{ElapsedClockError, ElapsedClockPort, ElapsedClockRefusal};
mod clock;
mod delivery;
pub use delivery::{ProgressExchangeService, ProgressExchangeWorker, ProgressQueryResult};
mod deadline;
mod error;
pub use deadline::ProgressExchangeDeadline;
mod model;
mod ports;
mod producer;
mod receiver;
pub use producer::TransferProgressProducer;
mod source;
pub use error::ProgressReceiveError;
pub use model::{
    ProgressBound, ProgressClaimRefusal, ProgressProjectionSnapshot, ProgressSourceTask,
    ProgressSubmission, ProgressTaskDescriptor, ProgressTerminalNotice,
};
pub use ports::{
    CurrentProgressTaskPort, ProgressReceiverPort, ProgressTaskAuthorityPort,
    ProgressTerminalRoundPort, ProgressWirePort,
};
pub use receiver::TransferProgressReceiver;
pub(crate) use source::ProgressSourceCatalog;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod source_tests;

#[cfg(test)]
mod deadline_tests;

mod snapshot;
pub use snapshot::{
    ProgressPageReply, ProgressPageRequest, ProgressSnapshot, ProgressSnapshotPage,
    ProgressSnapshotProviderPort, ProgressSnapshotQuery, ProgressSnapshotRead,
    ProgressSnapshotRoundPort, ProgressSnapshotTarget, ProgressSnapshotWirePort,
    TransferProgressSnapshotReader,
};

pub use model::ProgressUpdateNotice;
pub use ports::ProgressLiveRoundPort;

pub use model::ProgressContactNotice;

mod projection;
pub use projection::{
    ProgressProjectionAdmission, ProgressProjectionEvent, ProgressProjectionKey,
    ProgressProjectionRegistry, ProgressProjectionWorker, TransferProgressReconciler,
};

mod connectivity;
pub use connectivity::transfer_progress_connectivity_policy;
