use crate::connectivity::{ConnectivityError, ElapsedClockError};
use std::error::Error;
use std::fmt;
use uc_core::error_class::ErrorClass;
use uc_core::file_transfer::ProgressSnapshotRefusal;

#[derive(thiserror::Error)]
pub enum ProgressReceiveError {
    #[error("progress projection registration busy")]
    Busy,
    #[error("progress snapshot incomplete or inconsistent")]
    Snapshot(#[from] ProgressSnapshotRefusal),
    #[error("progress clock unavailable")]
    Clock(#[from] ElapsedClockError),
    #[error("progress connection failed")]
    Connection(#[from] ConnectivityError),
    #[error("peer does not support current progress protocol")]
    Unsupported,
    #[error("unexpected progress protocol")]
    Protocol,
    #[error("progress access denied")]
    AccessDenied(#[source] Box<dyn Error + Send + Sync>),
    #[error("progress message is malformed")]
    Malformed(#[source] Box<dyn Error + Send + Sync>),
    #[error("progress task is not authorized")]
    UnknownTask,
    #[error("progress logical plan is invalid")]
    InvalidPlan,
    #[error("progress registration or binding is stale")]
    Stale,
    #[error("progress terminal conflicts with adopted result")]
    Conflict,
    #[error("progress exchange failed")]
    Exchange(#[source] Box<dyn Error + Send + Sync>),
}
impl ErrorClass for ProgressReceiveError {
    fn class(&self) -> &'static str {
        match self {
            Self::Busy => "progress_registration_busy",
            Self::Snapshot(_) => "progress_snapshot_invalid",
            Self::Clock(source) => source.class(),
            Self::Connection(source) => source.class(),
            Self::Unsupported => "progress_protocol_unsupported",
            Self::Protocol | Self::Malformed(_) => "progress_protocol_invalid",
            Self::AccessDenied(_) => "progress_access_denied",
            Self::UnknownTask => "progress_task_not_authorized",
            Self::InvalidPlan => "progress_plan_invalid",
            Self::Stale => "progress_binding_stale",
            Self::Conflict => "progress_terminal_conflict",
            Self::Exchange(_) => "progress_exchange_failed",
        }
    }
}

impl fmt::Debug for ProgressReceiveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}
