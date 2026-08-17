# Specification 024 implementation

## Goal

Implement `docs/specs/024-workspace-convergence-internal-boundaries.md` completely without changing public behavior, persistence, wire formats, or ownership.

## Completion criteria

- The target `admission/`, `membership/`, `projection/`, and `connectivity/` domain directories exist with one domain entry each.
- Long admission, membership-history, removal, bootstrap, projection, and connectivity flows no longer live in `convergence/mod.rs`.
- Existing public exports and callers remain stable; old source locations and temporary forwarding paths are removed.
- Tests are grouped with their domain and prove the existing success, duplicate, recovery, and stable-failure behavior.
- ADR-021, specification 024, and the architecture bible describe the final state.
- Focused tests, affected crate suites, and every repository delivery command pass with nonzero test counts.
- A strict read-only review finds no blocking issue; the final scoped commit is created on the current branch.

## Phases

| Phase | Status | Evidence |
| --- | --- | --- |
| 1. Audit existing boundaries and add structural guard | complete | Source map; guard failed on all expected old paths and flow markers |
| 2. Extract projection domain | complete | Profile 4, scope 14, device trust 6, content gate 1, workspace query 1 tests passed; all-target compile passed |
| 3. Extract admission domain | complete | Compile plus transaction 2, durable join 5, durable admission 4, recovery 2, ordering 1, cancel race 1 tests passed |
| 4. Extract membership history domain | complete | 24 focused history tests passed |
| 5. Extract removal, effects, bootstrap, and legacy upgrade | complete | Removal 8, decision 5, effects 2, legacy 17, delivery 3 tests passed |
| 6. Extract connectivity domain and regroup tests | complete | Connectivity 11 and regrouped convergence 95 tests passed; structural guard passed |
| 7. Remove old paths and align documentation | complete | Old-path search clean; ADR/spec/architecture and dependent specs aligned |
| 8. Full verification, strict review, and scoped commit | complete | Strict review and all delivery gates passed; scoped commit created at task close |

## Invariants

- `WorkspaceConvergence` remains the only complete workflow owner.
- `WorkspaceConvergenceState` remains the only active-Space convergence state and is persisted only through the existing encrypted store boundary.
- `DurableAdmissionTransaction` remains the only admission-attempt state owner.
- No behavior, public API, persistence schema/encoding, network frame, logging policy, or lock/network ordering changes.
- Current ordinary peer scope derives only from accepted membership history.
- No long-lived compatibility forwarding modules remain.

## Errors encountered

| Error | Attempt | Resolution |
| --- | --- | --- |
| Listed implement skill path did not exist under the repository | 1 | Read the available skill at `/Users/mark/.agents/skills/implement/SKILL.md` |
| Empty file moves were rejected by `apply_patch` | 1 | Combined each move with its required parent-module path or module documentation edit |
| `GroupUpdateDeliveryPort` import was removed while changing its path | 1 | Restored the import from the membership domain before rerunning compilation |
| First generated current-scope patch omitted the trailing blank-line boundary | 1 | Expanded the exact removal range; checked partial state before the next retry |
| Rejected generated patch had already added one module declaration | 1 | Removed the duplicate declaration; future retries inspect every touched file first |
| Exact workspace-query test filter ran zero tests | 1 | Re-run with the fully qualified module path and require one passed test |
| Space-transition recovery methods were copied but not removed from `mod.rs` | 1 | Deleted the identical old methods before rerunning compilation |
| Admission child files did not import parent-module helpers | 1 | Added explicit domain-local imports and kept cross-domain helper visibility limited to convergence |
| Adjacent generated deletion hunks shared context and partially applied | 1 | Completed the deletions as two non-overlapping patches and inspected all touched files |
| Membership extraction cut two method doc comments at range boundaries | 1 | Moved each comment to the method it documents before formatting |
| A diagnostic `zsh` glob had no nested matches | 1 | Re-ran the search against explicit admission and membership directories |
| Narrowing the admission entry hid a candidate fixture from two space-admission test modules | 1 | Re-exported that one type only for test builds at space-domain visibility; production visibility stays narrow |
| Full workspace test timed out while shutting down the restarted empty UniFFI engine | 1 | Exact test reproduced; baseline narrowly passed; stage tracing located the wait in normal iroh shutdown, whose documented outer budget is 15 seconds. Removed tracing, aligned this test's final deadline to 15 seconds, and the exact test passed in 14.30 seconds |
