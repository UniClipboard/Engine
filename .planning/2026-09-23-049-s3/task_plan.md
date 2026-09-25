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

### Phase F — S3.a remaining failure diagnosis
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

### Phase G — Plan 049 S4 admission handoff  ← CURRENT (2026-09-25)
Decisions (user, 2026-09-25):
- Joiner: staged generation no longer holds membership record/read model (NoSpace). After `execute`
  promotes, Application reloads the Owner and commits a joiner-start Owner input (BeforeCommit), then saves the
  admission record. Replays after restart are Unchanged. Receipt-bearing history comes from the Infra staged
  target, bumped V2→V3 (V2 frozen since v1.1.0-rc.16) to carry the joiner's own activation receipt; Core
  `accept_complete` takes the updated staged target. In-flight V2 activations: staged DB already holds a record
  written by the old version; Owner only checks it matches.
- Branch recovery: checkpoint lives in the membership record, so post-promotion Owner input would break crash
  recovery. Owner precomputes the staged target record (BranchRecovered + checkpoint TargetStaged + projection)
  and Infra writes it verbatim into the staged DB.
- Owner reloads its published view after any control generation switch (latent stale-cache bug: first commit
  after branch promotion conflicts and only recovers next round).
Steps:
- [x] G1 Owner `reload()` + joiner-start draft method (idempotent) + staged-commit builder
- [x] G2 Joiner: staged target V3 with receipt; Core accept_complete takes staged target; execute returns start
      input; Application commits via Owner after promotion; drop record/relationship writes in material.rs/build
- [x] G3 Sponsor: drop Infra `apply_member_facts` if projection covers it; fault-injection tests
- [x] G4 Branch: Owner-built staged record into Infra stage/finalize; delete Infra ledger apply
- [x] G5 Architecture check: Infra must not call MembershipLedger::start/apply or build SpaceMembershipRecord
- [x] G6 Docs: ADR-027 handoff section, plan 049 S4 record; verification
- Results: units 2591/2591; uc-engine 358/358; membership-e2e run 1 50/51 (F2 load timeout, alone 3/3 at
  normal ~56 s), run 2 51/51; delivery checks pass. Nothing committed yet.
- **Status:** complete (awaiting commit approval)

### Phase H — Plan 049 S5 inbound access  ← CURRENT (2026-09-25)
Findings/decisions (user confirmed):
- "Departing peer only receives the exact removal notice" is the OUTBOUND direction (done in S3). Inbound stays:
  ledger-gated protocols reject departing peers; identity-only protocols (history exchange, branch recovery) still
  identify them (removed devices' decisions must still be recorded).
- Identity candidates = option A: effective members + departing peers + local device (exactly the projected
  member table), so rejection classification is unchanged.
- Five control DB swap paths (joiner, branch, device reset, fresh-join terminate, factory reset); only the first two
  reload the Owner. Replace explicit reload with a pool generation counter checked on every Owner read.
Steps:
- [x] H1 DbPool generation + DbExecutor::database_generation
- [x] H2 MembershipRecordStorePort::generation; Owner auto-reload; drop reload()
- [x] H3 PeerAccess (kept the ADR name; A3 asserts it) (deferred owner binding) + PeerIdentityDirectoryPort
- [x] H4 Infra resolver/gate read the directory; Engine wiring
- [x] H5 Tests (owner generation, access/directory, removal consistency), e2e group, docs
- Results: units 3025/3025; A2/A3 include-ignored 1/1, 5/5; membership-e2e 51/51; delivery checks pass.
- **Status:** complete (awaiting commit approval)

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
