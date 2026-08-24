mod error;
mod model;
mod use_case;

pub use error::JoinSpaceError;
pub use model::{JoinSpaceInput, JoinSpaceResult};
pub(crate) use use_case::JoinSpaceUseCase;
