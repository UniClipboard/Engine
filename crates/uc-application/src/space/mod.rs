//! Space-scoped application workflows.
//!
//! A member roster and every membership transition only exist inside a space.
//! The four subdomains own the complete space lifecycle:
//!
//! - `lifecycle` — create, unlock, switch, reset and the space session;
//! - `admission` — pairing invitation issuance / redemption and the
//!   admission channel used by workspace convergence (ADR-017);
//! - `roster` — member listing and per-member preferences;
//! - `workspace_membership` — history, removal, decisions and current scope;
//! - `convergence` — temporary home of connectivity code during migration.
//!
//! Everything that belongs to a space stays inside this directory; callers
//! reach the space through `facade` only.

pub(crate) mod admission;
pub(crate) mod assembly;
pub(crate) mod connectivity;
pub(crate) mod current_space;
pub(crate) mod initialize_space;
pub(crate) mod lock_space_session;
pub(crate) mod query_space_access_state;
pub(crate) mod re_pairing;
pub(crate) mod rebuild_space;
pub(crate) mod recover_space_session;
pub(crate) mod reset_space;
pub(crate) mod runtime;
pub(crate) mod session;
pub(crate) mod unlock_space;
pub(crate) mod upgrade_space;
pub(crate) mod workspace_membership;
