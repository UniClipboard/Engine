//! 取消当前 Space 中尚未使用的配对邀请。
mod error;
mod use_case;

pub use error::CancelInvitationError;
pub(crate) use use_case::CancelPairingInvitationUseCase;
