use uc_core::error_class::ErrorClass;

use crate::space::lifecycle::CurrentSpaceIdentityError;

#[derive(Debug, thiserror::Error)]
pub enum QuerySpaceAccessStateError {
    #[error("failed to load current Space identity")]
    CurrentSpace(#[from] CurrentSpaceIdentityError),
}

impl ErrorClass for QuerySpaceAccessStateError {
    fn class(&self) -> &'static str {
        match self {
            Self::CurrentSpace(_) => "current_space",
        }
    }
}
