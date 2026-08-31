//! Public Space facade whitelist.
//!
//! The implementation belongs to the private `space` module. External
//! consumers keep using this stable facade namespace.

pub use crate::space::{
    build_space_session_activity, CancelInvitationError, CompletePendingSpaceTransitionError,
    CurrentInvitation, InitializeSpaceError, InitializeSpaceInput, InitializeSpaceResult,
    InvitationAvailability, IssuePairingInvitationError, IssuePairingInvitationResult,
    MembershipConflictBranchView, MembershipConflictView, MembershipConflictsView,
    PairingInvitationAddressCandidate, QueryMembershipConflictsError,
    QueryPairingInvitationAddressesError, QueryPendingSpaceTransitionError, QuerySetupStateError,
    RedeemPairingInvitationError, ResetSpaceError, ResolveMembershipConflictError,
    ResolveMembershipConflictInput, ResolveMembershipConflictResult, SetupStateView,
    SpaceActivityError, SpaceAdmissionDeps, SpaceFacade, SpaceFacadeDeps, SpaceSessionActivityDeps,
    SpaceSessionActivityPort, SpaceSessionDeps, SpaceTransitionDeps, UnlockSpaceError,
    UnlockSpaceInput, UnlockSpaceResult,
};
pub use uc_observability_contract::analytics::PairingFailureReason;
