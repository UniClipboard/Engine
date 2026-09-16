mod error;
mod model;
mod revocation;
mod use_case;

pub use error::RemoveSpaceMemberError;
pub use model::{
    AdmissionAbandonmentRevocationTarget, AdmissionRevocationResult, AdmissionRevocationTarget,
    MembershipCommitReceipt, RemoveSpaceMemberResult,
};
pub use revocation::AdmissionRevocationPort;
pub(crate) use use_case::RemoveSpaceMemberUseCase;

#[cfg(test)]
mod tests;
