mod error;
mod model;
mod use_case;

pub use error::RemoveSpaceMemberError;
pub use model::{MembershipCommitReceipt, RemoveSpaceMemberResult};
pub(crate) use use_case::RemoveSpaceMemberUseCase;

#[cfg(test)]
mod tests;
