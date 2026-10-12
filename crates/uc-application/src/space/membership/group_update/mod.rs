//! 组更新完整收发、应用协调与主动恢复。
mod coordinator;
mod model;
mod ports;
mod receiver;
mod recovery;
pub(crate) use model::PreparedRemoval;
#[cfg(test)]
pub(crate) use ports::NoSecurityRemoval;
pub(crate) use ports::{PrepareMembershipRemovalPort, RecoverGroupUpdatesPort};
pub(crate) use recovery::GroupUpdateRecoverySchedule;

pub(crate) use coordinator::GroupUpdateApplyCoordinator;
pub use model::{
    GroupUpdateApplyError, GroupUpdateIncoming, GroupUpdateReceiveOutcome, PreparedGroupUpdate,
    VerifiedHistoryGroupUpdate,
};
pub use ports::{
    ApplyGroupUpdatePort, GroupUpdateLocalWorkPort, GroupUpdateReceiverPort, GroupUpdateWirePort,
    GroupUpdateWork,
};
pub use receiver::GroupUpdateEndpoint;

#[cfg(test)]
mod tests;
