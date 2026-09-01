//! Application-layer clipboard restore use cases. `pub(crate)` per
//! `docs/design-docs/layers/application.md` — external callers go through
//! `ClipboardRestoreFacade`.

pub(crate) mod file_snapshot;
pub(crate) mod restore_as_plain_text;
pub(crate) mod restore_selection;
pub(crate) mod touch_entry;

pub(crate) use restore_as_plain_text::{
    NoFilePathsAvailable, PlainRestoreOutcome, RestoreClipboardEntryAsPlainTextUseCase,
};
pub(crate) use restore_selection::RestoreClipboardSelectionUseCase;
pub(crate) use touch_entry::TouchClipboardEntryUseCase;
