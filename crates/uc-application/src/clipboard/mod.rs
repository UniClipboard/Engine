//! Clipboard-scoped application workflows.
//!
//! The complete result — capture, save, sync, restore, active state and
//! queryable history — stays inside this directory:
//!
//! - `capture` — capture use case and facade internals;
//! - `write` — write coordination, active-state advancement and restore
//!   broadcast;
//! - `history` — cleanup, retention, delete, detail, resource, favorite and
//!   file reconciliation;
//! - `restore` — restore selection, plain text and file paths;
//! - `sync` — inbound materialization, codec, dispatch, resend, receive
//!   gate, active state and the outbound plan;
//! - `entry_identity.rs` / `file_set_query.rs` — shared internal owners.

pub(crate) mod active;
pub(crate) mod assembly;
pub(crate) mod capture;
pub(crate) mod entry_identity;
pub(crate) mod file_set_query;
pub(crate) mod history;
pub(crate) mod inbound;
pub(crate) mod outbound;
pub(crate) mod resource;
pub(crate) mod restore;
pub(crate) mod sync;
pub(crate) mod write;
