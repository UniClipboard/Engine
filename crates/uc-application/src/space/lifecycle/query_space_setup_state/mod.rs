//! 查询当前 Space 的 Setup 产品状态。
mod error;
mod model;
mod use_case;

pub use error::QuerySetupStateError;
pub use model::{CurrentInvitation, SetupStateView};
pub(crate) use use_case::QuerySpaceSetupStateUseCase;
