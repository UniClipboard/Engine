//! Application-internal holder for outstanding sponsor-side pairing
//! invitations.
//!
//! Sits outside `crate::pairing` (which is the pre-Slice-1 libp2p pairing
//! stack) so the new Slice 1 invitation flow doesn't pollute the legacy
//! namespace on the way to its eventual removal (Slice 5).
//!
//! All types here are `pub(crate)` per `uc-application/AGENTS.md` §11.4:
//! the holder is a cross-use-case flow-state component, not an external
//! boundary. External callers interact with invitations exclusively
//! through [`crate::space::SpaceFacade`].

mod cancel;
mod holder;
mod issue;
mod issue_for_address;
mod query_addresses;

mod issuer;

pub use cancel::CancelInvitationError;
pub use query_addresses::{
    PairingInvitationAddressCandidate, QueryPairingInvitationAddressesError,
};

pub(in crate::space) use cancel::CancelPairingInvitationUseCase;
pub(in crate::space) use holder::InMemoryPairingInvitationHolder;
pub(in crate::space) use issue::IssuePairingInvitationUseCase;
pub(in crate::space) use issue_for_address::IssuePairingInvitationForAddressUseCase;
pub(in crate::space) use issuer::PairingInvitationIssuer;
pub(in crate::space) use query_addresses::QueryPairingInvitationAddressesUseCase;
