//! Receive-side transfer workflows.
//!
//! `reconciliation` owns receive readiness, startup recovery, timeout
//! cleanup and receive-attempt reconciliation; the transfer domain is the
//! complete owner of receive settlement and terminal writes.

pub(crate) mod reconciliation;
