# Entry thumbnail reads

## Ownership and contract

The Application resource module owns the complete read. Desktop calls `ReadEntryThumbnail(HistoryEntryInput)` once and receives `ThumbnailRead` with WebP bytes. It never supplies a representation ID. The owner resolves the authoritative preview through the existing committed resource snapshot.

Use an existing stored thumbnail when available. Generate missing thumbnails from inline or blob-backed images, including old entries. Cache only successful derived bytes in a bounded, process-local cache. Do not add persistent state or change encrypted storage. Restart drops the cache; a later read regenerates it. Original content and identity remain unchanged.

A missing entry returns NotFound. A non-image, absent preview, unavailable payload, or the source byte limit returns Unavailable. Decoder rejection returns Internal. Saturated generation returns a retryable Unavailable error with a distinct code. Storage and generation failures preserve their source internally and return a sanitized Internal error. Calls execute synchronously, so no pending-job API is needed. Revalidate ownership before returning bytes after concurrent deletion or replacement. No detached Application task survives cancellation.

## Failure matrix and validation, before implementation

Cover inline images, blob images, missing derived metadata, restart with no process cache, non-image entries, missing entry, deleted entry, concurrent reads, source deletion or replacement during generation, corrupt image data, decode limits, request cancellation, storage failure, and missing source bytes. Generate a JSON evidence artifact from public Engine operations against an isolated real profile. Never use a unit test as acceptance. Keep the Desktop 60-second observation distinct from any newly reproduced failure.

## Progress

- Investigation: current HEAD equals Desktop's pin. Background generation skips inline data. Thumbnail failure follows BlobReady and spool deletion; startup recovery does not repair it.
- Implemented `ReadEntryThumbnail` using the existing committed resource snapshot, encrypted readers, the existing generator port, and the existing Moka dependency.
- Public Engine acceptance passed. It emits `result.json` and `thumbnail.webp`. Blob-backed and 2048×1536 inline sources, original-byte preservation, cold and warm concurrency, absent thumbnail metadata, restart, decoding failures, dimension limits, Lost source state, missing preview selection, and deletion races were exercised.
- Cold parallel reads produced one ready response and two retryable busy responses. One separate run observed a staged source and returned retryable 1465; the acceptance caller now retries that documented result.
- Workspace all-target compilation, metadata, formatting, Rust style, repository architecture checks, and diff checks passed.
- Cancellation permit ownership and replacement revalidation were inspected in source; deterministic cancellation, replacement, and storage-failure injection were not executed. GUI behavior, HTTP routing, and Desktop RSS remain downstream acceptance items.
- The Desktop timeout JSON was examined. It lacks worker errors and representation/queue/spool state. The precise 60-second cause remains unconfirmed. The foreground path no longer requires successful background thumbnail generation.
- Stable contract and Desktop instructions: [Entry thumbnails](../../design-docs/entry-thumbnails.md).
