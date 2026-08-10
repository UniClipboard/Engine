//! Application-layer workflows for UniClipboard.

pub(crate) mod clipboard;
pub mod deps;
pub mod facade;
pub mod file_sync;
pub(crate) mod search;
#[cfg(test)]
pub(crate) mod test_support;
pub(crate) mod transfer;

// D16-2: deps re-exports so composition roots (uc-bootstrap, uc-tauri,
// uc-daemon) can depend on `uc_application::*` directly and the legacy
// `uc_app::*` shims can be retired.
pub use deps::{
    AppDeps, ClipboardPorts, DevicePorts, SearchPorts, SecurityPorts, StoragePorts, SystemPorts,
};

// Slice 2 Phase 3 · T4 — public use case consumed directly by daemon
// `InboundClipboardSyncWorker` (T8). Lives inside `clipboard::sync`
// (which is `pub(crate)`) so internals stay encapsulated; we re-export
// only the small public surface here.
pub use clipboard::sync::{
    ApplyInboundClipboardUseCase, ApplyInboundError, ApplyInboundInput, ApplyOutcome,
    FileCacheBlobMaterializer, InboundApplyCommonDeps, InboundBlobFetcher, InboundBlobMaterializer,
    InboundCapture, InboundReceiveAttemptDeps, InboundWrite, InteractiveReceiveDeps,
    StoreOnlyPullDeps,
};

// Note: V3 envelope codec helpers (decode_v3_bytes_to_snapshot,
// decode_v3_bytes_to_snapshot_and_blob_refs, V3BlobRef) used to live
// here. Per AGENTS.md §11.4.3 they now route through `facade/` —
// import them as `uc_application::facade::decode_v3_bytes_to_snapshot`
// etc. The implementations stay in `clipboard::sync` but the crate
// boundary only exposes them via the facade.
pub mod space;
pub mod trusted_peer;
/// `pub(crate)` — migration in progress (ADR-018): remaining use-case
/// directories (`mobile_sync`, `search`, `upgrade`) move into their
/// domains; nothing here is part of the stable public surface.
pub(crate) mod usecases;
