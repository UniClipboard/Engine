use crate::space::lifecycle::CurrentSpaceIdentityError;

#[derive(Debug, thiserror::Error)]
pub enum QuerySpaceAccessStateError {
    #[error("failed to load current Space identity")]
    CurrentSpace(#[from] CurrentSpaceIdentityError),
}
