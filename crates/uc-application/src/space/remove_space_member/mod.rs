mod error;
mod model;
mod ports;
mod use_case;

pub use error::RemoveSpaceMemberError;
pub use model::{MembershipCommitReceipt, RemoveSpaceMemberResult};
pub use ports::WakeSpaceMembershipMaintenancePort;
pub(crate) use use_case::RemoveSpaceMemberUseCase;

#[cfg(test)]
mod tests;
