//! Space-scoped application workflows.
//!
//! A member roster and every membership transition only exist inside a space.
//! The four subdomains own the complete space lifecycle:
//!
//! - `lifecycle` — create, unlock, switch, reset and the space session;
//! - `admission` — pairing invitation issuance / redemption and the
//!   admission channel used by workspace convergence (ADR-017);
//! - `roster` — member listing and per-member preferences;
//! - `convergence` — workspace convergence, membership connectivity,
//!   network recovery, legacy-member upgrade and presence reachability.
//!
//! Everything that belongs to a space stays inside this directory; callers
//! reach the space through `facade` only.

pub(crate) mod admission;
pub(crate) mod convergence;
pub(crate) mod lifecycle;
pub(crate) mod roster;
pub(crate) mod runtime;
