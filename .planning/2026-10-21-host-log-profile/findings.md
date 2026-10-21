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

## Follow-up C: `space_leave::repeated_leaves_do_not_accumulate_tasks_or_descriptors` (CI #227 failure, same name as the final-2 flake)

Failure modes: (1) real leak of tasks or descriptors per leave cycle; (2) sampling noise from background reclamation; (3) a fix that only widens the threshold hides (1).

Evidence (macOS, repeated runs, `lsof` per round and a 100 ms trace after each leave):
- No leak: sockets are constant (14 established TCP, 3+1 UDP/listen), tasks constant (64 locally, 39 in CI), no stale-generation database files, no growth over 12 rounds.
- Noise source A (transient): after `FactoryResetSpace` returns, the old runtime keeps releasing descriptors asynchronously. The count falls from about 40 to a floor of about 27 over 1-2 s. The old fixed 500 ms sleep sampled in the middle of that drain.
- Noise source B (steady): once settled, the count still varies by about 3 between rounds (25-28 locally; 28-30 in CI) because the fresh runtime's SQLite pool holds 2-4 pooled connections (profile.sqlite and its wal each), depending on startup concurrency. The pool opens connections lazily (`min_idle(0)`), so this is bounded pool state, not accumulation.
- The old rule (late max <= early min + 1 over 3+3 samples) assumed +-1 noise and failed on both sources.

Fix (test only, same contract: one leaked handle per cycle must fail): wait until tasks and descriptors are unchanged for five 100 ms polls before sampling (never settling in 15 s fails the test), use 8 rounds, and judge descriptors by the least-squares trend (< 0.5 per cycle) instead of the first/last extremes. Tasks keep the original early/late rule on settled samples.
Negative controls (temporary injection, not committed): leaking one fd per cycle fails with `grew 1.00 per leave cycle`; leaking one task per cycle fails with `alive tasks grew`. Logs in the thread library `verification/leak-test/`. Three clean runs pass.
Product observation (not changed): old-runtime resources are released about 1-2 s after `FactoryResetSpace` returns.
