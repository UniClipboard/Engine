# ADR-020 redesign plan

## Goal

Replace quorum-confirmed membership transitions with an offline-first reconciliation model that propagates signed history, requires per-device acceptance for unseen removals, and lets diverged branches continue without sharing with each other.

## Completion criteria

- [x] ADR-020 removes OpenRaft, quorum, unique-successor, and automatic winner semantics.
- [x] The accepted confidentiality and availability trade-off is explicit.
- [x] One module owns hello exchange, history comparison, removal decisions, divergence, restart recovery, and public results.
- [x] Add propagation, removal acceptance/rejection, divergence gating, and later reconciliation are implementation-ready.
- [x] Rejecting a removal never automatically removes its author or any other peer.
- [x] Presence, membership, and history relationship are modeled independently.
- [x] Directly affected ADR/spec references do not present the superseded quorum design as current.
- [x] `docs/architecture/architecture-bible.md` records the architecture change.
- [x] Required documentation verification commands pass, or blockers are evidenced.

## Implementation follow-up

The implementation and current `1.1` multi-device topology acceptance are complete in this
repository. The two official `v0.19.0` fixture-driven cases remain explicitly skipped in this local
matrix; their separate real two-device upgrade acceptance is recorded in ADR-020.

1. [complete] Replaced the old auto-applied removal state with signed membership history,
   local removal decisions, and distinct known/applied heads.
2. [complete] Replaced the old removal notice/exchange protocols with bounded hello, history,
   decision, and acknowledgement messages on the authenticated member channel.
3. [complete] Persisted per-peer history relationships and deferred security state; blocked content
   and membership propagation only for pending or diverged peer relationships.
4. [complete] Exposed one user decision and a complete convergence snapshot through `uc-engine`
   and bindings; deleted superseded state, protocols, and tests.
5. [complete] Verified restart, partition, duplicate, simultaneous-decision, branch-isolation,
   relay, and repeated removal/rejoin scenarios across three to five independent Engine instances.
   The serial matrix ran 17 cases: 15 passed and 2 official-v0.19 fixture cases were skipped.

## Phases

1. [complete] Inspect ADR-020, related ADR/spec references, repository rules, and architecture records.
2. [complete] Revise ADR-020 around membership reconciliation and per-device destructive decisions.
3. [complete] Synchronize directly affected decision/status documents and architecture record.
4. [complete] Run repository documentation and required non-behavior checks; fix all task-owned failures.

## Errors

No current `1.1` topology failure remains. The two official-v0.19 fixture cases in the local matrix
remain skipped and must not be reported as locally passed.

- The first complete topology rerun failed all 14 runnable cases before device setup because the
  v0.19 directory adopter treated a missing fresh-install data root as an inspection failure.
  A focused red test reproduced this in 0.00s; missing roots now pass while unreadable roots and
  legacy/current conflicts still fail.
- After that fix, the serial matrix completed with 9 passed, 5 failed, and 3 skipped in 594.24s.
  Remaining failures are tracked separately: two offline-sponsor history handoffs, one unaffected
  reject-branch content rejection, one fresh-instance rejoin conflict, and one shutdown timeout
  after a stale-sponsor scenario reached its behavioral assertions.
- The final serial matrix completed with 15 passed, 0 failed, and 2 skipped in 431.74s. The shutdown
  timeout was caused by an abandoned operation future retaining its in-flight registration; the
  registration now follows the future lifetime, and both the focused regression and original
  stale-sponsor topology pass.
