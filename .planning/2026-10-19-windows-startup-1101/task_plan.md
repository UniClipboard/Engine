# Windows startup 1101 (profile_storage_upgrade corrupt) — Engine side

Base: origin/main fe4f831076f510a2d2f74a7c7ea7c11e0ff15e05

## Owner / action / result
- Owner: `ProfileStorageUpgrade::ensure_v3` (uc-infra-profile) owns verification of the promoted V3 target
  and its failure classification. Engine only maps the stable error.
- Caller's single action: `Engine::start*` (host).
- Success: startup continues. Failure: `ProfileStorageUpgradeError` -> `StorageUpgradeFailure` -> startup failure.
- Retry: journal stays at its phase when verification fails; next `ensure_v3` repeats verification.

## Phases
1. [x] Instrumentation only: record SQLite result class on upgrade failure (no behavior change). Commit.
2. [x] E2E overlap probe (two real host processes, same profile). Capture "before" evidence (macOS, then Windows VM).
3. [x] Fix: verify path opens connections with repo-standard busy wait; lock contention -> Storage (retryable).
4. [x] E2E after: overlap, real corruption still corrupt, normal start, failed attempt then restart.
5. [x] Delivery checks, report, handoff.
