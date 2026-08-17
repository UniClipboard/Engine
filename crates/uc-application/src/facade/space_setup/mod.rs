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

pub use commands::{
    CurrentInvitation, InitializeSpaceInput, InitializeSpaceResult, InvitationAvailability,
    IssuePairingInvitationResult, PairingInvitationAddressCandidate, RedeemPairingInvitationInput,
    RedeemPairingInvitationResult, SetupStateView, UnlockSpaceInput, UnlockSpaceResult,
};
pub use deps::{SpaceAdmissionDeps, SpaceFacadeDeps, SpaceSessionDeps, SpaceTransitionDeps};
pub use errors::{
    CancelInvitationError, FactoryResetError, InitializeSpaceError, IssuePairingInvitationError,
    QuerySetupStateError, RedeemPairingInvitationError, ResetSpaceError, TryResumeSessionError,
    UnlockSpaceError,
};
pub use facade::SpaceFacade;
pub use uc_observability_contract::analytics::PairingFailureReason;

pub(crate) const LEGACY_SPACE_ID: &str = "space";

pub(crate) fn legacy_space_id() -> uc_core::ids::SpaceId {
    uc_core::ids::SpaceId::from(LEGACY_SPACE_ID)
}

// T10:CLI `members` 入口需要 report / error 类型才能展示 probe 摘要;
// usecase 本身保持 `pub(crate)`(§11.4),此处只透出两个值对象。
pub use crate::space::convergence::connectivity::reachability::{
    EnsureReachableAllError, EnsureReachableAllReport,
};
