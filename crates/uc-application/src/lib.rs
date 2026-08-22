//! Application-layer workflows for UniClipboard.
//!
//! ADR-018: external crates only import `uc_application::facade` (the
//! business entry whitelist) and `uc_application::deps` (passive assembly
//! groupings). Everything else is `pub(crate)`.

pub mod deps;
mod error;
pub mod facade;
mod profile;

pub(crate) mod clipboard;
pub(crate) mod device;
pub(crate) mod search;
pub(crate) mod settings;
pub(crate) mod space;
pub(crate) mod support;
#[cfg(test)]
pub(crate) mod test_support;
pub(crate) mod transfer;

// D16-2: deps re-exports so composition roots (uc-bootstrap, uc-tauri,
// uc-daemon) can depend on `uc_application::*` directly and the legacy
// `uc_app::*` shims can be retired.
pub use deps::{
    AppDeps, ClipboardPorts, DevicePorts, SearchPorts, SecurityPorts, StoragePorts, SystemPorts,
};
