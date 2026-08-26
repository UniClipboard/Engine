//! Public Space facade whitelist.
//!
//! The implementation belongs to the private `space` module. External
//! consumers keep using this stable facade namespace.

pub use crate::space::{
    CancelInvitationError, CompletePendingSpaceTransitionError, CurrentInvitation,
    InitializeSpaceError, InitializeSpaceInput, InitializeSpaceResult, InvitationAvailability,
    IssuePairingInvitationError, IssuePairingInvitationResult, PairingInvitationAddressCandidate,
    QueryPairingInvitationAddressesError, QueryPendingSpaceTransitionError, QuerySetupStateError,
    RedeemPairingInvitationError, ResetSpaceError, SetupStateView, SpaceAdmissionDeps, SpaceFacade,
    SpaceFacadeDeps, SpaceSessionDeps, SpaceTransitionDeps, UnlockSpaceError, UnlockSpaceInput,
    UnlockSpaceResult,
};
pub use uc_observability_contract::analytics::PairingFailureReason;
