# Task Plan: Plan 049 S3 wrap-up and membership e2e test efficiency

Planning files for this task live in `.planning/2026-09-23-049-s3/` (reuse this directory when
continuing). Never create planning files in the repository root.

## Goal

Finish plan 049 slice S3 (single-owner space membership) with honest verification evidence, and make the
membership multi-engine e2e suite a first-class, efficient nextest entry so later slices get fast,
reliable feedback.

## Context

- Worktree `/Users/mark/.herdr/worktrees/engine/hp-uni-t-0010-android`, branch `hp/uni/t-0010-android`.
- Plan doc: `docs/exec-plans/active/049-single-owner-space-membership-rewrite.md` (S0–S2 done, S3 in progress,
  S4–S6 not started; S3 must not merge to main alone).
- S3 commits: `811fec0f`, `22392d17`, `9e550840` (not pushed). This session's changes are uncommitted.
- Do NOT touch/commit someone else's WIP (expect `git diff --stat` = +372/−13 on them):
  `crates/uc-engine/src/runtime/profile_recovery.rs`, `crates/uc-engine/tests/host_contract/key_loss.rs`,
  `crates/uc-infra/src/security/profile_key_recovery.rs`, `docs/design-docs/uc-engine-interface.md`,
  `docs/exec-plans/active/2026-09-23-inbound-peer-admission.md`.

## Phases

### Phase A — S3 wrap-up work done this session
- [x] F7 epoch race: wait for zero unfinished effects before reading the baseline epoch (user approved).
- [x] `.config/nextest.toml` override for `space_membership_auto_pairing_e2e` (group of 4, 60s x 10) (user approved).
- [x] Architecture check: only `MembershipOwner` constructs `MembershipRecordCommit`, with negative fixture.
- [x] Clippy clean on S3-changed production lines.
- [x] ADR-027 section on pending-decision relations, RwLock work permit, worker round order.
- [x] S3 implementation record in plan 049.
- [x] Baseline worktree and its target dir removed.
- **Status:** complete

### Phase B — Step 1: make membership e2e a first-class nextest entry
- [x] Add `membership-e2e` group to `scripts/testing/run-test-group.sh` (nextest, `--features dev-tools`,
      `--test space_membership_auto_pairing_e2e`, pass-through args).
- [x] Fix stale `binary(membership_ledger)` → `binary(membership_record)` in the `persistence-provider` group.
- [x] `docs/design-docs/testing-guide.md` §6: document the group, why nextest (one process per test, group
      concurrency, timeouts; `cargo test` runs all 51 in one process ≈27 min with distorted timing), keep the
      `cargo test` entry (dual-track rule in the guide).
- [x] Plan 049 acceptance/verification: add the explicit membership e2e command; note that
      `cargo test -p uc-engine --locked` does not compile this binary (it is `#![cfg(feature = "dev-tools")]`).
- [x] Verify: script syntax (`bash -n`), `run-test-group.sh membership-e2e -E 'test(f7_)'`,
      `persistence-provider` group, delivery checks.
- **Status:** complete

### Phase C — Remaining S3 items
- [x] `membership_history_retryable_failure_exposes_deadline_and_recovers`: wait for `Retrying` *with*
      `next_retry_at_ms` (user approved; passes).
- [ ] Investigate `offline_member_catches_multiple_removals_without_blocking_new_invitations` (group epoch wait
      timeout under load; not yet re-run alone).
- [ ] Run the remaining `uc-engine` test binaries (`cargo test -p uc-engine --locked`, host contract).
- [x] Committed (user approved): `6e77dcbf` nextest group, `914c06fa` S3 verification gaps, `8c2577e4` planning notes.
- **Status:** pending

### Phase E — Migrate the real membership e2e onto nextest + uc-testkit
- [x] Decisions (user, all recommended): one binary split into category modules + shared `harness/`;
      uc-testkit Scenario inside the harness; CI = PR smoke via nextest, nightly/manual full group.
- [x] Execution plan: `docs/exec-plans/active/050-membership-e2e-nextest-migration.md` (S1–S5), indexed.
- [x] S1 structural split — pure motion, same 62-test set; full group 45/51 (F6 epoch race, fixed in harness).
- [x] S2 per-category nextest groups — `membership-topology` (2); full group 47/51 = only the 4 baseline failures.
- [x] S3 testkit Scenario in harness — guard line in 62 tests; full group 47/51, 51 artifacts, all cleaned.
- [x] S4 CI (pr-check `membership-e2e-smoke`; engine-real-environment `membership-e2e` job) — remote runs skipped until pushed.
- [x] S5 docs (testing-guide, test-adoption-inventory, plan 034 step 6, tests/observability/README.md).
- [x] Committed plan 050 work (user approved): c6a39348 test, 35125fcd ci, 6c6d154e docs, d06c0ef4 notes.
- **Status:** in_progress

### Phase F — S3.a remaining failure diagnosis  ← CURRENT
- [x] nextest overrides for slow tests + node_lifecycle exclusive (commit e77b7a4e4); units 2556/2557.
- [x] S3 marked complete; S3.a added to plan 049 (commit 3f23a3278).
- [x] joiner_pairing_fixture_reaches_active_settled — TEST, fixed (fixture status timing)
- [x] same_device_returns… — PRODUCT outside 049 (profile key recovery, others' WIP)
- [x] suspend_during… — PRODUCT outside 049 (resume inspects transition while locked)
- [x] handoff_four_device… — TEST, fixed (accepting side no longer lists removed device)
- [x] confirmed_pairing… — PRODUCT in 049 domain; FIXED (member_for_device prefers effective, no fallbacks)
- [x] pending_join… — PRODUCT: key epoch store lock contention during sponsor activation (diagnostics added); fix awaits user
- Rule: test problem → fix test (waits/preconditions only, no weakened business assertions, explain first);
  product problem → record root cause, ask user.
- [x] Commit test fixes, fix, diagnostics, S3.a record (user approved).
- **Status:** in_progress

### Phase D — Later (separate plan, after 049)
- [ ] Layer the tests: move membership protocol convergence scenarios (F0–F7, cross removals, offline
      catch-up) to the deterministic virtual network (plan 034) with an injected test clock; keep a small set of
      Engine loopback-QUIC smoke scenarios. Deferred because plan 049 requires F2–F7 to pass unchanged as the
      S3 regression baseline.
- [ ] Consider running the membership-e2e group in CI (today CI only runs `automatic_connections::`).
- **Status:** pending

## Decisions Made
| Decision | Rationale |
|----------|-----------|
| ADR-020 wins over old F1 expectation | User decision (previous session) |
| RemoveMember `Unavailable` → 1392 retryable | User decision (previous session) |
| Diagnostic logs added while debugging are permanent | User decision; saved in memory |
| F7 waits for settled effects before reading epoch | User approved option (b); old code had the same commit-then-wake window |
| nextest override for the e2e binary, group of 4 | User approved; limits load-induced timing errors |
| Planning files under `.planning/2026-09-23-049-s3/` | User rule: `.planning/<YYYY-MM-DD>-<name>/`, never repo root |
| Keep `cargo test` entry for the e2e binary | Testing guide requires dual-track; do not delete old gates |
| Defer test layering until after 049 | F2–F7 are the unchanged S3 regression baseline |
| E2E split: one binary, category modules, `harness/` pub(crate) | Links Engine once; nextest filters by module path |
| Split generated by script (scratchpad split_analyze.py / split_generate.py) | Mechanical, verifiable motion of 6281 lines |

## Errors Encountered
| Error | Attempt | Resolution |
|-------|---------|------------|
| `grep --include=*.rs` failed under zsh (no matches found) | 1 | Quote the glob or drop `--include` |
| Foreground `sleep 100` blocked by harness | 1 | Use `until …; do sleep 5; done` waits |
| Full clippy blocked by pre-existing `async_yields_async` deny in `application/shutdown.rs` | 1 | Run with `-A clippy::async_yields_async` for analysis only |
| First `Summary` grep matched test output | 1 | Match `^\s+Summary \[` |
