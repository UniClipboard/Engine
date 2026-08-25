mod error;
mod target_use_case;

pub use error::CancelSpaceJoinError;
pub(crate) use target_use_case::CancelSpaceJoinUseCase;

#[cfg(test)]
mod target_tests;
