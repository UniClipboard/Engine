mod error;
mod model;
mod ports;
mod use_case;

pub use error::{AcceptAdmissionError, HandleSpaceAdmissionMessageError, LoadMemberAdmissionError};
pub use model::{
    AuthenticatedSpaceAdmissionMessage, LoadedMemberAdmissionActivation,
    MemberAdmissionCommitToken, PreparedMemberAdmissionActivation, PreparedSpaceAdmissionCommit,
    PreparedSpaceAdmissionMessage, SpaceAdmissionPreparationContext,
};
pub use ports::{
    ConsumedInvitation, HandleSpaceAdmissionMessagePort, InboundAdmissionStatePort,
    PrepareSpaceAdmissionMessagePort,
};
pub(crate) use use_case::HandleSpaceAdmissionMessageUseCase;

#[cfg(test)]
mod tests;
