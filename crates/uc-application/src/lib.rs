//! Application-layer workflows for UniClipboard.
//!
//! ADR-018: external crates only import `uc_application::facade` (the
//! business entry whitelist) and `uc_application::deps` (passive assembly
//! groupings). Everything else is `pub(crate)`.

pub(crate) mod application;
pub mod deps;
pub(crate) mod error;
pub mod facade;
pub(crate) mod profile;

pub(crate) mod clipboard;
pub(crate) mod device;
pub(crate) mod search;
pub(crate) mod settings;
pub(crate) mod space;
pub(crate) mod support;
#[cfg(test)]
pub(crate) mod test_support;
pub(crate) mod transfer;
