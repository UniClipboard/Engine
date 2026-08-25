mod error;
mod model;
mod port;
mod use_case;

pub use error::HandleSpaceAdmissionMessageError;
pub use model::{
    AuthenticatedSpaceAdmissionMessage, PreparedSpaceAdmissionCommit,
    PreparedSpaceAdmissionMessage, SpaceAdmissionPreparationContext,
};
pub use port::{HandleSpaceAdmissionMessagePort, PrepareSpaceAdmissionMessagePort};
pub(crate) use use_case::HandleSpaceAdmissionMessageUseCase;

#[cfg(test)]
mod tests;
