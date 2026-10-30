# ADR-035: Recoverable process and startup release owners

- Status: Proposed; not implemented or accepted as a shipped contract.
- Date: 2026-10-27.
- Related: [Engine resource-release contract](../engine-resource-release.md), [implementation specification](../../exec-plans/active/2026-10-27-process-startup-release-spec.md), [observability design](../observability.md).

## Context

A host must not hand a shared runtime lock to another process while an owner can still use a protected resource. The existing per-Engine outcome covers the documented Engine scope. It excludes process observability and host-owned files. A successful flush does not close the daily log file or stop later producers. Process observability shutdown is terminal and is not a foreground resume mechanism.

A failed startup can return no `MobileEngine`. The startup lifecycle control does not expose a complete cleanup receipt. Capability preparation can fail before its input is taken. Later failures can occur after storage or workers start. An error, a stopped worker, a timeout, or an absent object does not certify the complete resource scope.

## Decision

Use a complete owner for each distinct lifetime:

1. The process observability owner controls the installation and its covered producers, accepted work, writers, and leases. It offers one recoverable release action and one resume action to the host.
2. A startup-attempt owner exists before the first fallible resource-producing step. It retains construction and cleanup ownership until the attempt is delivered or its scoped cleanup has a terminal result. It is available even if no `MobileEngine` is returned.
3. The existing per-Engine owner remains responsible for its own documented scope. Host-owned resources retain their own owners and receipts.

Each owner returns `Pending`, `Released`, or `Failed` with the existing meanings. `Released` applies only to the owner's explicitly listed resources. It can carry an independent operational error. `Failed` means release is unconfirmed; it retains a strong owner for any live failed closer. A terminal worker cannot be restarted by polling. A wait deadline ends only the caller's wait. Accepted cleanup remains owned.

Process release fences producer admission before draining accepted work. It joins all covered work and receives positive close acknowledgements before returning `Released`. A later flush or old producer cannot reopen a released file. Resume invalidates the prior generation before it opens admission or files. An old result cannot confirm release of the new generation. The owner coordinates all Engine registrations; it cannot park shared writers while another active Engine still requires them.

The startup attempt owns its execution runtime independently of the caller's executor. On successful delivery, the Engine takes that runtime responsibility. A successful cleanup or Engine release requires the owned execution runtime to stop after its accepted work settles. A creation failure before any work is accepted remains an operation failure; the cleanup result depends on the actual resource acknowledgements.

Startup ownership covers pre-input failure, partial construction, rollback, failed delivery, and late completion. The owner records the primary startup error independently from cleanup errors and resource classification. It distinguishes a retryable live closer from a terminal failure. It never infers release from object absence or thread termination alone.

## Alternatives

- Treat flush or per-Engine suspension as process release: rejected. Neither closes every process writer or fences later work.
- Terminate and reinstall the process singleton: rejected. Terminal shutdown does not provide the required recoverable foreground path.
- Let the host combine individual writer acknowledgements: rejected. The host cannot account for internal detached work or reliably order all producers and closers.
- Return a generic startup error or rely on worker join: rejected. These observations do not provide a complete scoped cleanup result or retain a failed closer.
- Combine Engine, process, and host resources into one Engine receipt: rejected. Those resources have different lifetimes and owners. An Engine receipt cannot certify host-owned SQLite, JS, cache, diagnostic files, or the runtime lock.

## Consequences

The process owner must coordinate shared Engine registrations, background/foreground generations, telemetry and file worker lifetimes. The startup owner must retain cleanup resources across constructor and caller lifetimes. Both require explicit bounded waits and reproducible resource evidence. The host must still collect its own file acknowledgements before lock handoff.

This decision does not select a logging coexistence or exclusion policy between products or processes. It does not select Share interruption recovery or termination. A real policy choice must be presented separately. This decision does not identify the historical `0xdead10cc` lock or claim a device crash fix.
