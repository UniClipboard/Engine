# Findings: host log profile

## Design facts
- Engine never applied `debug_mode`. The host installs its own log layers before `Engine::start`.
- `current_effective_log_profile` guessed from `RUST_LOG`, `UC_LOG_PROFILE`, stored `debug_mode` and Engine's `cfg!(debug_assertions)`. Removed.
- UniFFI and HarmonyOS N-API expose no diagnostics, debug-mode or settings types. `git diff 976b9ffe -- bindings compatibility` is empty. UniFFI builds `EngineConfig::new(..).with_profile_id(..)` only, so Mobile reports an absent profile until a binding parameter exists.
- Label lives on `EngineConfig`, not on the process observability runtime, because the runtime installs once per process.

## Pre-existing failure at the task8 base 976b9ffe (RESOLVED in follow-up A)
- `uc-engine --lib assembly::host::tests::startup_preparation_reuses_backup_after_later_import_failure` fails deterministically at `host.rs:757` on `976b9ffe` (verified by checking out that commit) and at head (`host.rs:761`). Task8 did not run the Engine lib suite. Not caused by task9.

## Task7 old-anchor smoke (a13 -> head/d4): root cause
- `scripts/testing/build-upgrade-anchors.sh` builds the anchor snapshot with `CARGO_TARGET_DIR="$shared_target"` (the current workspace's target).
- The machine-level Cargo storage guard (`~/.local/bin/cargo`, outside the repo) rejects a target directory that belongs to another workspace: "Uni 编译存储检查失败：不得使用其他工作目录的编译目录". The mbx shim reports it as "could not verify Cargo build storage: metadata probing failed".
- Reproduced on this branch with `bash scripts/testing/build-upgrade-anchors.sh a13` (build failed after 1s). Anchor cache removed with `--clean` afterwards.
- No task-local invocation fixes it: the script hardcodes the variable. A fix needs a script change (let the guard assign the snapshot its own external target directory, then reclaim it) or a guard exemption. Both change shared infrastructure, so the smoke is carried as NOT EXECUTED.
- The legacy `l0194` cells (static fixture, no old binary) do run: 3 passed with `general.debug_mode = "yes"` injected.

## Follow-up A: host.rs backup-reuse test (resolved)
- Cause: obsolete test order, not a production regression. Task8 (`976b9ffe`, `use_case.rs`) deliberately moved `apply_pending_import` before `ensure_backup`, and updated the Application tests, but not the two Engine host tests. A poisoned `pending-import.json` now fails before backup and legacy adoption, so `legacy.exists()` stayed true.
- The sibling `startup_preparation_stops_before_legacy_mutations_when_backup_fails` still passed, but for the wrong reason (the import failed first), so it no longer proved a backup denial stops legacy mutation.
- Fix (test only): the sibling no longer poisons the marker. The reuse test now fails the `profile_lifecycle_marker:v1` write (after backup and legacy adoption) and keeps every assertion: one archive, byte-identical across retries, adopted identity, no lifecycle marker. Renamed `..._after_later_lifecycle_failure`.
- Retry safety of a failing import itself is covered by task8's public host-file E2E (partial adoption retries).

## Follow-up B: old-anchor build (resolved)
- The Cargo storage guard allows only a workspace's own canonical target. The anchor snapshot is its own workspace, so the script now resolves each snapshot's target with `cargo metadata --manifest-path <snapshot>/Cargo.toml` (inherited environment, no override) and builds with that value. No guard, global config or cache change. `--clean` already reclaims these directories.
- a13 built in 263s; a13_to_head d1, d2, d3 (both inviters) and d4 passed (5/5).
