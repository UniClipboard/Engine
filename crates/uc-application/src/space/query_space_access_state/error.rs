use crate::space::current_space::CurrentSpaceIdentityError;

#[derive(Debug, thiserror::Error)]
pub enum QuerySpaceAccessStateError {
    #[error("failed to load current Space identity")]
    CurrentSpace(#[from] CurrentSpaceIdentityError),
}
