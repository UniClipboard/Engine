# ADR-004: Restore Single Representation for Clipboard Restore

Status: Historical. The current restore path reconstructs a snapshot with multiple available representations and passes it to the host write boundary. See [`restore_selection.rs`](../../../crates/uc-application/src/clipboard/restore/restore_selection.rs) and [`snapshot_from_entry.rs`](../../../crates/uc-application/src/clipboard/sync/snapshot_from_entry.rs). This does not establish multi-format acceptance on every host.

## Context

Clipboard entries can contain multiple representations (e.g. text + html). The
restore use case currently rebuilds a `SystemClipboardSnapshot` that includes the
primary representation plus secondary representations selected by policy.

The platform clipboard adapter (`uc-platform`) currently writes through
`clipboard-rs` high-level APIs, which overwrite the clipboard on each call. It
requires exactly one representation and returns an error when the snapshot
contains more than one.

This causes restore to fail for entries that include secondary representations.

## Decision

Limit restore to the primary `paste_rep_id` in
`RestoreClipboardSelectionUseCase::build_snapshot`, producing a snapshot with a
single representation.

Secondary representations remain stored in the database and continue to be used
for previews and resource resolution, but are not restored to the system
clipboard until platform-specific multi-format atomic writes are implemented.

## Consequences

- Restore succeeds for entries with multiple representations by writing only the
  primary representation.
- Clipboard fidelity on restore is reduced (secondary formats are dropped).
- Multi-representation restore remains a platform-level TODO (macOS
  `NSPasteboardItem`, Windows multi-format clipboard, Linux target lists).
