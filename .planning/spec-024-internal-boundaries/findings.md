# Findings

## Baseline

- `main` is at `80ac2e6`; `origin/main` matches.
- Existing user work contains ADR-021, specification 024, architecture-bible entries, and the prior specification-023 planning directory. No convergence source file is modified yet.
- `crates/uc-application/src/space/convergence/mod.rs` is 5,382 lines.
- Existing sibling implementation files are `admission_transaction.rs`, `group_update_delivery.rs`, `legacy_upgrade.rs`, `membership_connectivity.rs`, and `reachability.rs`; none of the four target domain directories exists.
- All convergence tests currently default to a single 8,547-line `tests.rs`, plus 908 lines of legacy-upgrade tests.

## Source ownership map

- Profile projection currently starts in `ProfileWorkspaceConvergence` near the top of `mod.rs`.
- Admission flow occupies the early `WorkspaceConvergence` implementation through local join activation and recovery.
- Shared load/persist/publish helpers and pending-effect recovery begin after admission.
- Membership history reconciliation, removal decisions, relationship effects, bootstrap, and current-scope endpoint implementations occupy the remainder.
- Connectivity is already in dedicated sibling files, so its change should be a path move plus call/import alignment rather than a behavior rewrite.
- The existing architecture checker hard-codes the current-scope implementation and three old flat paths, so it must move with the ownership change rather than silently stop checking the rule.
- The main implementation has natural contiguous ownership regions: profile projection, admission, shared state/effect recovery, public/query and history reconciliation, removal/bootstrap, then current-scope and endpoint implementations.
- `ProfileWorkspaceConvergence` is one contiguous implementation block and can move without changing its public type or callers.
- Current scope is one contiguous private helper plus the `CurrentWorkspacePeerScopePort` implementation. The workspace query also reads that helper, so it must remain visible to the convergence parent until query moves into projection.
- Projection extraction leaves only type definitions in the main module. Profile methods, device-trust/workspace queries, current scope, local removal checks, and the content gate now compile from the projection domain.

## Verification rule

- Cargo name filters may run zero tests; capture passed counts or use `--list` before relying on focused evidence.

## Strict review

- The old monolithic convergence test file and the three domain test files each contain the same 95 test cases by name.
- The only three non-identical moved test bodies differ by domain path names and formatting; no test semantics changed.
- The old flat module names are no longer exposed. The admission transaction is private to the admission domain, with only the existing caller capability and test-only fixtures re-exported at the narrowest required visibility.
- The architecture checker now requires all four domain entries, rejects all retired flat paths, and rejects representative long flows in the owner module.
- No remaining review finding prevents final verification or commit.

## Final-suite shutdown diagnosis

- The UniFFI space-management test used a five-second deadline for the final shutdown of a restarted engine, while the iroh shutdown contract documents a fifteen-second outer budget.
- The current change reproduced the deadline failure consistently; an archived `HEAD` baseline narrowly completed, showing a timing boundary rather than a membership behavior failure.
- Temporary local stage markers proved every application runtime completed and the remaining wait was the ordinary iroh node shutdown. All markers were removed before continuing.
- Changing only that test's final deadline to fifteen seconds preserved its assertions and production behavior. The exact test then passed in 14.30 seconds.
