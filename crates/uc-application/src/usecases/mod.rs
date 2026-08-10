//! Remaining use-case directories during the ADR-018 migration. Each
//! directory moves into its domain in the same round that the domain is
//! consolidated; nothing here is part of the stable public surface.

#[cfg(feature = "lan-compat")]
pub(crate) mod mobile_sync;
pub(crate) mod upgrade;
