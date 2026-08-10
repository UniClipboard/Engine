//! `lifecycle` domain — first-run space creation, post-setup unlock,
//! switch-space migration, space session, setup status, encryption and
//! local device identity.
//!
//! All flows are intentionally kept as cross-port orchestration inside
//! single use cases (AGENTS.md §8.1): the passphrase-mismatch check,
//! ordered port calls and "don't mark complete if an earlier step failed"
//! invariant all belong to one atomic application action.

pub(crate) mod device;
pub(crate) mod encryption;
pub(crate) mod initialize_space;
pub(crate) mod session;
pub(crate) mod setup_status;
pub(crate) mod switch_space;
pub(crate) mod unlock_space;
