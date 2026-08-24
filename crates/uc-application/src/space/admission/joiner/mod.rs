//! Joiner-side pairing internals.
//!
//! Symmetric to [`crate::pairing_inbound`] on the sponsor side: wire and
//! crypto work is owned by a coordinator, persistence / setup-status /
//! composition lives in the joiner-side redemption flow and the outer
//! [`crate::space::admission::join_space::JoinSpaceUseCase`].
//!
//! Per `uc-application/AGENTS.md` §11.4 everything here is `pub(crate)`;
//! external callers reach joiner pairing exclusively through
//! [`SpaceFacade::redeem_pairing_invitation`].
//!
//! [`SpaceFacade::redeem_pairing_invitation`]:
//!     crate::facade::space_setup::SpaceFacade::redeem_pairing_invitation

mod durable_flow;
pub(crate) mod joiner_handshake;
mod owner;
mod ports;
mod redeem_invitation;

pub(in crate::space) use durable_flow::{
    record_invitation_consume_result, InvitationConsumeResultV1,
};
pub(crate) use owner::JoinerAdmissionOwnerPort;
pub use ports::GroupAdmissionPort;
pub(crate) use redeem_invitation::{
    RedeemPairingInvitationOutcome, RedeemPairingInvitationUseCase,
};
