//! `SpaceFacade` — lifecycle of the local encrypted space.
//!
//! Covers first-run initialization (A1 `InitializeSpaceUseCase`) and
//! post-setup unlock (A2 `UnlockSpaceUseCase`). Constructed from
//! [`SpaceFacadeDeps`] so external callers (bootstrap) bundle ports into
//! one struct instead of passing a dozen positional arguments.
//!
//! Distinct from the older `crate::setup::SetupFacade`, which orchestrates
//! the device-onboarding (pairing / join) flow that predates Slice 1. The
//! two facades will co-exist until later slices consolidate them.

pub(crate) mod commands;
mod deps;
mod errors;
mod facade;

pub use crate::space::admission::complete_pending_space_transition::CompletePendingSpaceTransitionError;
pub use crate::space::admission::invitation::cancel::CancelInvitationError;
pub use crate::space::admission::invitation::query_addresses::{
    PairingInvitationAddressCandidate, QueryPairingInvitationAddressesError,
};
pub use crate::space::admission::query_pending_space_transition::QueryPendingSpaceTransitionError;
pub use crate::space::initialize_space::{InitializeSpaceError, InitializeSpaceResult};
pub use crate::space::query_space_setup_state::{
    CurrentInvitation, QuerySetupStateError, SetupStateView,
};
pub use crate::space::reset_space::ResetSpaceError;
pub use crate::space::unlock_space::UnlockSpaceError;
pub use commands::{
    InitializeSpaceInput, InvitationAvailability, IssuePairingInvitationResult,
    RedeemPairingInvitationInput, RedeemPairingInvitationResult, UnlockSpaceInput,
    UnlockSpaceResult,
};
pub use deps::{SpaceAdmissionDeps, SpaceFacadeDeps, SpaceSessionDeps, SpaceTransitionDeps};
pub use errors::{IssuePairingInvitationError, RedeemPairingInvitationError};
pub use facade::SpaceFacade;
pub use uc_observability_contract::analytics::PairingFailureReason;

pub(crate) const LEGACY_SPACE_ID: &str = "space";

pub(crate) fn legacy_space_id() -> uc_core::ids::SpaceId {
    uc_core::ids::SpaceId::from(LEGACY_SPACE_ID)
}

// T10:CLI `members` 入口需要 report / error 类型才能展示 probe 摘要;
// usecase 本身保持 `pub(crate)`(§11.4),此处只透出两个值对象。
pub use crate::space::connectivity::reachability::{
    EnsureReachableAllError, EnsureReachableAllReport,
};
