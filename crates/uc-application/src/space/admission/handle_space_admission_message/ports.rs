use async_trait::async_trait;
use uc_core::membership::SpaceJoinRecordId;

use super::{
    AuthenticatedSpaceAdmissionMessage, HandleSpaceAdmissionMessageError,
    LoadedMemberAdmissionActivation, MemberAdmissionCommitToken, PreparedMemberAdmissionActivation,
    PreparedSpaceAdmissionMessage, SpaceAdmissionPreparationContext,
};

use super::error::{AcceptAdmissionError, LoadMemberAdmissionError};

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ConsumedInvitation {
    digest: [u8; 32],
}

impl ConsumedInvitation {
    pub(crate) fn new(digest: [u8; 32]) -> Self {
        Self { digest }
    }

    pub(crate) fn digest(self) -> [u8; 32] {
        self.digest
    }
}

#[async_trait]
/// Validates and prepares one protocol transition without side effects.
///
/// Implementations must not persist or send the reply. The use case owns the
/// invitation check, atomic membership commit, and maintenance wake-up.
pub trait PrepareSpaceAdmissionMessagePort: Send + Sync {
    async fn prepare(
        &self,
        message: &AuthenticatedSpaceAdmissionMessage,
        context: &SpaceAdmissionPreparationContext,
    ) -> Result<PreparedSpaceAdmissionMessage, HandleSpaceAdmissionMessageError>;
}

#[async_trait]
pub trait HandleSpaceAdmissionMessagePort: Send + Sync {
    async fn handle_space_admission_message(
        &self,
        message: AuthenticatedSpaceAdmissionMessage,
    ) -> Result<Vec<u8>, HandleSpaceAdmissionMessageError>;
}

#[async_trait]
pub trait InboundAdmissionStatePort: Send + Sync {
    async fn load(
        &self,
        record_id: SpaceJoinRecordId,
    ) -> Result<LoadedMemberAdmissionActivation, LoadMemberAdmissionError>;
    async fn accept(
        &self,
        token: MemberAdmissionCommitToken,
        prepared: PreparedMemberAdmissionActivation,
        invitation: Option<ConsumedInvitation>,
    ) -> Result<(), AcceptAdmissionError>;
}
