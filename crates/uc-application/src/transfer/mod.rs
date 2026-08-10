//! Transfer-scoped application workflows.
//!
//! `blob` owns content publish/fetch (plaintext and path variants),
//! `file` owns the file-transfer session timeline and application errors,
//! and `receive` owns receive readiness, receive-attempt reconciliation,
//! startup recovery and timeout cleanup. The complete transfer result —
//! from publish, through progress and cancel, to restart recovery and
//! cleanup — stays inside this directory.

pub(crate) mod blob;
pub(crate) mod file;
pub(crate) mod receive;
