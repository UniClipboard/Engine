# Entry thumbnail implementation and cleanup

## Outcome and ownership

The Application resource module owns the complete thumbnail read. Products perform one entry-based action. The final behavior, limits, breaking changes, lock coordination, and retry rules have one source: [Entry thumbnails](../../design-docs/entry-thumbnails.md).

The user explicitly required a breaking change and product adaptation. An earlier additive implementation retained old APIs and background generation. The final revision removes them, their metadata adapters, and their URL/dimension projections. Historical schema and encrypted bytes are retained; this is not a destructive storage migration.

## Failure matrix recorded before implementation

Cover inline/blob images, historical data, absent/inaccessible derivatives, missing/deleted entries, missing preview selection, unavailable sources, concurrent reads, invalid encodings, size/dimension limits, generation failure, cancellation, locking, and restart. Preserve authoritative source bytes and opaque identities. Prefer public Engine end-to-end scenarios with reusable artifacts; add no unit tests after implementation.

Before cleanup, extend acceptance to seed inaccessible legacy metadata, prove reads ignore it, assert no new metadata is written, and check removed serialized fields. Repeat prior source, restart, concurrency, failure, and deletion cases against the final tree.

## Evidence and limits

- Initial source and Desktop pin: `357d7f64006789285fe5bfc4f025992bc9b84e5f`.
- The earlier additive candidate `27bd1f8107401bdeb49e5ab6081ca4b64904f39e` passed public Engine acceptance and required workspace/repository checks. Its artifacts are not final-cleanup evidence.
- The Desktop timeout JSON records one unavailable thumbnail after a 60-second polling window. It lacks representation state, worker errors, queue state, and spool state. Its exact cause remains unconfirmed.
- Source inspection identified a background recovery gap: original BlobReady and spool deletion preceded thumbnail generation. A later thumbnail failure was not recovered by the spool scanner. Final cleanup removes background thumbnail generation entirely.
- Security review 4423214272 was reproduced through public Engine operations: warming a blob-backed thumbnail, locking, and reading it again returned 31,516 bytes. A separate failing artifact records that baseline.
- The security fix serializes locking against reads, checks session readiness before and after the resource action, and clears the cache under exclusive lock. Public Engine acceptance passed lock/unlock, repeated locked requests, and lock races with cold/warm reads. It returns distinct retryable 1467 while locked.
- Final-tree public Engine acceptance passed both scenarios, including ignored inaccessible legacy metadata, no new derivative rows, removed list fields, Staged/Processing and source-size outcomes, source preservation, restart, deletion, lock/unlock, and locked parallel reads. Locked results contain no thumbnail bytes. Required metadata, workspace all-target compilation, formatting, Rust style, architecture, and diff checks passed. Previous artifacts remain separate.
- Deterministic cancellation and replacement injection, physical source-blob loss, and storage/authentication-failure injection are not completed acceptance items. Source inspection does not replace those scenarios.
- Product HTTP routing, GUI RSS, scrolling, object URL release, and original-image UI actions remain downstream acceptance. No Engine artifact proves GUI memory reduction.

## Review disposition

Keep documentation in English and do not add post-implementation unit tests, as explicitly required by the user. Centralize imports in existing test stubs. Keep investigation evidence here and link from the contract instead of duplicating it. Address snapshot concurrency by acquiring the resource gate before decryption and holding it through final validation; retain the existing committed snapshot port rather than adding a metadata-only port for this bounded operation.
