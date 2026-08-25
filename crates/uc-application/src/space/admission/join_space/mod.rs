mod error;
mod model;
mod target_use_case;

pub use error::JoinSpaceError;
pub use model::{JoinSpaceInput, JoinSpaceResult};
pub(crate) use target_use_case::JoinSpaceUseCase;
pub use target_use_case::{PrepareJoinSpacePort, PreparedJoinSpace};

#[cfg(test)]
mod target_tests;
