use async_trait::async_trait;

use super::{
    AuthenticatedSpaceAdmissionMessage, HandleSpaceAdmissionMessageError,
    PreparedSpaceAdmissionMessage, SpaceAdmissionPreparationContext,
};

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
