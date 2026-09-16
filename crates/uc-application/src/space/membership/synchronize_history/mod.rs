mod error;
mod model;
mod target_use_case;

pub(crate) use error::SynchronizeMembershipHistoryError;
pub use model::{MembershipSyncReport, MembershipSyncTarget};
pub(crate) use target_use_case::SynchronizeMembershipHistoryUseCase;

#[cfg(test)]
mod tests;
