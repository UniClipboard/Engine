# File-addressable receive snapshots

Status: implementation and host E2E verification complete. Issue: #368.

Base commit: `d532978d8bc9740a64460ee08b6e97a85fe4b590`.

Stable semantics: [host receive item snapshots](../../design-docs/receive-item-progress.md).

## Ownership and contract

The Application receive owner owns the complete query. The host calls
`QueryEntryReceiveProgress` or `ListEntryReceiveProgress`. The owner combines the
accepted original plan, current receive facts, saved results, and the existing
bounded progress catalog. Engine and bindings only map the complete result.

A snapshot provides typed original content positions, round counters, and the
catalog producer, accepted-attempt revision, and content revision. Saved bytes
remain separate from transferred bytes. Only materialization confirms completion.
Unknown metadata and evicted or restarted runtime facts remain unavailable.
The existing retry owner owns selective retry and preserves unselected results.
No new persistence, dependency, wire protocol, or host ledger is required.

## Implementation and acceptance

1. Add a real Engine regression scenario before production changes. Use synthetic
   files, actual network transfer, SQLite, controlled materialization, and hashes.
2. Expose the original owner's snapshot through Rust, UniFFI, and OHOS.
3. Verify late/reconnected queries, duplicate names, independent file positions,
   pending save at full bytes, failure/cancellation, selective retry, and restart.
4. Extend existing real representation and directory scenarios for typed positions
   and unknown totals. Save repeatable query evidence in an ignored artifact.
5. Run workspace compilation, formatting, architecture checks, and receive E2E
   scenarios. Record platform checks that were not executed as skipped.

## Failure boundaries

- A query can race with attempt replacement. A mismatched attempt must not borrow
  current counters. Hosts order complete snapshots by producer and revisions.
- A late event is only an invalidation signal. It never supplies query counters.
- File and representation positions have separate namespaces. Directory units
  describe one root and never claim member-level association.
- Full transferred bytes do not prove saved content. An unselected saved item
  must remain completed during retry.
- Legacy records and unavailable runtime plans must not fabricate zero counters
  or infer positions from opaque IDs.
- A restarted process does not restore runtime progress. Existing durable saved
  results remain authoritative; credentials and content never enter evidence.

## Binding evidence pipeline

The binding validators consume the artifact from the real Engine scenario. They
must reject missing checkpoints, lost optional values, changed item identities or
outcomes, merged locator namespaces, truncated byte counters/revisions, and
unknown values converted to zero. They run only when explicitly given that
artifact. Native Swift, Kotlin, and ArkTS device execution is a separate check.

## Verification adjustments

The complete receive suite exposed two contract assumptions in existing E2E
assertions. Restart now preserves durable results while clearing optional runtime
fields. A failed directory's superseded source mapping is unavailable; the
current redelivery carries the directory snapshot. The E2E checks now distinguish
these cases. Existing materialization checks also compare canonical filesystem
paths so that macOS `/var` and `/private/var` aliases identify the same file.

The host keeps an already known item failure during incomplete local cleanup.
The reverse-progress protocol can still defer its terminal acknowledgment.
The implementation preserves both existing responsibilities.

## Final verification

- The complete receive E2E group passed 61 scenarios. Two existing harness tests
  remained ignored: the explicit Collector scenario and a child-process entry.
  The process-restart scenarios invoked the child entry through their own harness.
- Both binding evidence pipelines passed. Each started a real Engine transfer,
  then checked file identities, byte counters, unknown values, outcomes, versions,
  and restart behavior through its production projection. Mixed-content,
  directory-unit, and zero-byte Engine artifacts also passed both mappings.
- Locked workspace metadata, workspace/all-target compilation, formatting, Rust
  style, direct-log-macro enforcement, repository architecture, and diff checks
  passed on macOS arm64. The evidence pipeline generated eight JSON artifacts and
  a SHA-256 manifest.
- iOS, Android, and HarmonyOS device execution, remote CI, and release publication
  were skipped. Desktop integration remains in its downstream repository.

Reproduce the Engine and binding evidence with
`scripts/testing/run-receive-item-progress-e2e.sh`. Stable contract semantics live
in the linked design document. This plan does not establish production or native
device acceptance.
