# Progress

## Result (2026-10-01)
- Audit: 302 production `map_err(|_| ..)` sites (the earlier "86" was a `grep -v test` undercount); 60 lost real diagnostics.
- Fixed:
  - core: `SpaceAdmissionPersistenceError`, `MembershipHistoryV2Error` and `MembershipBranchRecoveryError` now carry the
    postcard / history error as `#[source]` (they lost `Copy`); member-binding decode failures keep their source.
  - Application owners record failures: active clipboard query, space access state, peer connection refresh, diagnostics
    debug mode, settings get, upgrade detect/acknowledge, LAN interface probe (all with tests).
  - engine: JoinError records (operation, shutdown), temp dir creation, host file copy variants carry `io::Error`.
  - bindings: fixed-classification records before dropping io/serde/Join errors; `io_error_kind` exported via
    `uc_engine::observability`.
  - infra: upgrade backup store records list/delete failures; malformed search group reference is recorded once.
- Lint: `check-rust-style.mjs` scans all non-test code; every discard needs `discarded-source[category]: reason` with a
  category from `error-handling.md`. `--list-discarded` prints the inventory. `#[cfg(test)]` functions are now exempt too.
- Verification: fmt, check (default + lan-compat), all architecture checks and node tests pass. Full nextest: 4056 passed,
  68 failed = 66 upgrade-matrix `anchor-host-missing` (no baseline anchor hosts locally, not run) + 2 e2e that pass alone
  (load flake).

## Not done / follow-ups
- uniffi `receive_lifecycle_result` folds Timeout and Disconnected into one code; splitting changes the public code (decision pending).
- `.ok()` and `let _ =` discards are not covered by the lint.
- Subagent reports were partly wrong (one helper was defined but never wired); results were re-verified by compile and tests only.
