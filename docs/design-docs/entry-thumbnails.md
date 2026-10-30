# Entry thumbnails

The only thumbnail operation is `Operation::ReadEntryThumbnail(HistoryEntryInput { entry_id })`. Success is `OperationResult::ThumbnailRead(BinaryResourceSummary { bytes, media_type })`. The media type is `image/webp`. The generator preserves aspect ratio and limits the longest edge to 128 pixels. The operation completes synchronously.

The Application resource module owns preview resolution, generation, caching, and ownership validation. It resolves the current preview from the existing committed entry resource snapshot. It reads authenticated inline bytes or blob bytes only for that preview. Search consumers need only the entry ID.

Generated thumbnails use a 4 MiB weighted process cache. They are not persisted. Eviction or restart causes generation on a later read. Original content and representation identities remain unchanged. Existing encryption adapters protect persistent source bytes.

## Breaking change

`ReadThumbnail`, `ThumbnailResourceInput`, the representation-addressed resource resolver, background thumbnail generation, and the dedicated thumbnail metadata repository, mapper, domain model, and wiring are removed. The RGBA-only generator method is also removed. History summaries no longer contain `thumbnail_url`, `image_width`, or `image_height`. Image preview text uses a byte-size placeholder; it does not decode inline image bytes as text.

Historical thumbnail schema, migrations, and encrypted data remain intact. The operation does not read historical thumbnail metadata or blobs. An unreadable legacy derivative cannot block generation from an available original. Existing entry deletion still removes associated historical rows. Code cleanup does not introduce a destructive data migration.

Products must replace the old route and generated SDK references. Do not retain a compatibility route that depends on the removed operation. Original-resource operations remain necessary for explicit original-image actions.

## Session and concurrency

Application serializes thumbnail reads against encryption locking with a fair read/write guard. Each read checks session readiness before accessing the resource and again before returning its result. Locking acquires the write guard, waits for active readers, clears the thumbnail cache, and then locks the space session. A queued reader cannot pass the writer and reuse pre-lock cached bytes.

A read already authorized before the lock may complete. Locking does not retract bytes already returned to a host or received over HTTP. The host must revoke its own object URLs, clear its caches, and discard stale responses when content authorization changes.

One resource request at a time may acquire the source/ownership snapshot, use the cache, generate, and revalidate ownership. Capacity is checked before any snapshot inline decryption. Concurrent cache hits may therefore return Busy. Cache hits still read the authenticated snapshot to validate current ownership. This bounded approach reuses the existing snapshot port and avoids a separate metadata projection port.

Before returning, the owner validates the current event and preview again. Deletion or replacement cannot use an old cache entry to bypass ownership. A read whose final validation precedes a deletion may succeed; a later read must fail. Deleted representations' cached entries remain bounded and inaccessible until eviction or locking.

The Infra generator permits one blocking decode/resize task at a time. Its permit stays with the blocking task until it ends, including after caller cancellation. No persistent generation job exists. Cancelling a caller cannot stop an already running decoder; its limits still apply.

| Result | Code | Category | Retryable | Host behavior |
| --- | --- | --- | --- | --- |
| Entry absent or deleted | 1462 | NotFound | No | Remove or hide the image. |
| Resource slot occupied, or source still staged/processing | 1465 | Unavailable | Yes | Keep a placeholder; use bounded retry while visible. |
| Non-image, missing preview, lost/failed source, or source byte limit | 1466 | Unavailable | No | Keep a placeholder. |
| Encryption session locked | 1467 | Unavailable | Yes | Wait for confirmed unlock before retrying. |
| Source storage, authentication, decoder, or task failure | 1464 | Internal | No | Keep a placeholder; use normal diagnostics. |

Decoder dimension rejection and physical missing-source-blob errors return 1464. Errors contain no source bytes, filenames, paths, or internal error messages. Session locking is distinct from an unavailable preview.

Source input is limited to 32 MiB. The decoder limits each dimension to 8192 pixels and sets its allocation limit to 64 MiB. These are not total process RSS limits. Codec working memory, conversions, source/output buffers, allocator retention, and the existing background original-image conversion have additional costs. Removing background thumbnail generation also removes its retained full-size RGBA copy.

## Product integration

1. Add an authenticated binary route that accepts an entry ID.
2. Invoke `ReadEntryThumbnail` and return its bytes with the supplied media type.
3. Use the route for history-list and quick-panel images, including search results.
4. Remove the old representation-addressed route, SDK references, URL field, and list-dimension dependencies.
5. Keep original reads for explicit original-image actions.
6. Bound visible requests and retries. Cancel requests when items leave the view.
7. On lock, clear host caches and object URLs, advance the response generation, and discard stale responses.
8. For 1467, wait for confirmed unlock. Do not retry while the session remains locked.

Never fall back automatically to original-image loading on a thumbnail error. Engine publishes no HTTP URL. The host owns authentication, HTTP mapping, and frontend resource lifetime.

## Reproducible acceptance

Run from the repository root:

```bash
UC_TEST_ARTIFACTS_DIR="$PWD/target/test-artifacts/entry-thumbnails" \
  just cargo test -p uc-engine --test host_contract entry_thumbnail --locked -- --nocapture
```

The public Engine acceptance uses isolated real encrypted profiles. It emits `result.json`, `thumbnail.webp`, and `lock-result.json`. It covers inline/blob sources, inaccessible legacy derivatives, absence of new thumbnail metadata writes, removed list fields, unchanged original bytes, restart, parallel reads, invalid/dimension-limited images, lost sources, missing preview selection, deletion races, locked cache access, lock/unlock, and cold/warm reads racing with lock.

The Engine acceptance does not prove product HTTP routing, GUI memory, scrolling, object URL release, or transport response revocation. See the [execution plan](../exec-plans/completed/entry-thumbnails.md) for investigation evidence and unexecuted boundaries. On-demand generation removes background thumbnail readiness as a prerequisite.
