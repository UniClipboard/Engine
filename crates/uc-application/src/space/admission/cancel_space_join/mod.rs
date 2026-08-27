mod error;
mod use_case;

pub use error::CancelSpaceJoinError;
pub(crate) use use_case::CancelSpaceJoinUseCase;

#[cfg(test)]
mod tests;
