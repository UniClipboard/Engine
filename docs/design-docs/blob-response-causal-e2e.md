# Blob response recovery causal Engine E2E

## Scope and owner

This Linux-only complete Engine test extends the existing dev-tools ReceiveStreams fixture.
It uses two real Engines, QUIC, the Blob SDK, encrypted FsStore, stable operations, receiver
readback and sender delivery queries. Application remains the only recovery owner. The test
blocks one actual receive before its first read and never returns a synthetic business result.
No production capability, timer, configuration, Native code, host or runner changes.

## Failure modes recorded before implementation

1. Pairing or a metadata Blob consumes a generic next-read gate. Arm only after pairing is ready,
   stage exactly one cold file, and require the failed and successful exchanges to share the same
   digest and the successful exchange to validate exactly the expected file size.
2. Previously cached data bypasses the exchange. Use fresh profiles and a never-delivered staged
   file. Require two actual receives for that file, not a local protection hit.
3. A gate also blocks retry or is manually released before timeout. Consume it once at receive
   entry. The first gated exchange stays blocked until the production timeout ends it.
4. A called stop is mistaken for a successful stop. Record the real stop outcome and instant,
   complete receive result/source and return instant. Require cleanup success, no additional error,
   Tokio Elapsed, zero admitted/validated bytes, and stop before complete return.
5. A repeated stream ID is mistaken for the same connection. Require a new native pair in this
   controlled single-transfer run; do not compare raw stream IDs across connections.
6. A new attempt, another item or ActivePull rescues the file. Start one ResendEntry only, preserve
   original receiver entry/attempt/item identities, and never restore or retry after dispatch.
7. Timeouts are reset or readback returns late. Capture original logical deadline and actual30s
   response input. Require second receive to retain the same logical deadline. Read back the exact
   original receiver entry and observe original sender Delivered before the original deadline and
   one absolute60s action limit. Recheck time after awaited reads and delivery queries.
8. A passing receiver masks sender failure. Require Delivered for the original target, not accepted,
   pending, duplicate, restored, or later-item success.
9. Panic or failure loses evidence and leaks held work. Run the scenario as a fallible future,
   release fixture holds only during final cleanup, await both Engine shutdowns and the original
   send task, remove both temporary profiles with checked results, and export sanitized evidence
   before reporting failure. A whole-scenario watchdog
   triggers cleanup but cannot create a pass.

## Evidence contract

Write one versioned JSON artifact on pass and failure. Use a case identifier and fixed opaque
item/attempt/exchange aliases. Include relative monotonic times, actual error/source categories,
stop/finish results, retry ordering and unchanged deadline, expected/readback bytes and SHA-256,
original receiver identity checks, sender terminal state, native counts and awaited cleanup results.
Do not export paths, names, addresses, credentials, invitations, raw content or raw business IDs.
The artifact path comes only from UC_BLOB_RESPONSE_E2E_EVIDENCE, using a fixed target-directory
fallback when absent. Root owns artifact hashing and run/source/binary provenance.

Local protection35, late-positive header, additional cleanup failure, header-phase cancellation,
and terminal-reset/source-refusal controls remain NOT_RUN. They require separate seams or cases.
Existing sibling/shared-content/transient/resource cases are retained and executed separately by root.
