# Progress Log

## 2026-09-23/24 — Session: continue plan 049 S3 from handoff

### Done
- Read handoff (`/var/folders/.../engine-049-s3-handoff/HANDOFF.md`) and plan 049.
- Full e2e run #1 (handoff nextest config): 44/51; new failures re-run serially — f0, f7, retryable pass alone;
  pending_join fails alone and on baseline (1/3) → pre-existing flaky.
- Added architecture check `checkMembershipRecordCommitOwnership` + negative fixture
  (`scripts/architecture/check-engine-repository.mjs`); `productionSources` gained an exclusion list.
- Clippy fixes: `owner/view.rs` sort_by_key; `RestrictedMembershipDelivery` variants boxed (ports.rs, worker.rs,
  testing/membership_nodes.rs, iroh membership_history_exchange_adapter.rs); `DeviceId` clones removed in
  `uc-engine/src/assembly/sync_engine.rs` and `relationship_store/projection.rs`.
- User approved: F7 settled-effects wait (added `wait_for_settled_effects_named`); nextest override
  (`.config/nextest.toml`, group `membership-e2e`, max-threads 4, 60s x 10, default + ci).
- ADR-027: new section on pending-decision relations; relation list includes 等待对端决定.
- Plan 049: S3 implementation record appended (status stays 进行中).
- Full e2e run #2 (repo config): 44/51, F7 passes; failures classified (see findings.md).
- Delivery checks: metadata, check, fmt, rust-style, engine-repository, diff-check all pass; uc-core and
  uc-infra tests pass; uc-application space 324 pass / 1 known baseline fail / 1 ignored by design.
- Baseline worktree removed; target `/Volumes/ExternalSSD/cargo-targets/workspaces/engine/b4c143ab749c86ee` deleted.
- Answered user: suite uses real engines over loopback QUIC; nextest already in place; recommended test
  layering after 049. User asked to record state and start step 1.

### Phase B (step 1) — complete
- `scripts/testing/run-test-group.sh`: new `membership-e2e` group; `persistence-provider` now filters
  `binary(membership_record)` (was stale `membership_ledger`, which silently selected nothing).
- `docs/design-docs/testing-guide.md` §6: group documented (what is real, why nextest, cargo test kept as
  compatibility entry, filter examples).
- Plan 049 acceptance: added `bash scripts/testing/run-test-group.sh membership-e2e` and a note that
  `cargo test -p uc-engine --locked` does not compile the dev-tools-only membership e2e file.

### Phase C — done items
- retryable_failure test now waits for Retrying with a retry deadline (user approved); passes with rejection test.
- Commits: `6e77dcbf`, `914c06fa`, `8c2577e4`. AGENTS.md has an uncommitted user edit — left alone.

### Phase E — membership e2e migration (plan 050)
- User chose: category modules in one binary; testkit at harness level; PR smoke + nightly full.
- Wrote `docs/exec-plans/active/050-membership-e2e-nextest-migration.md`; index updated (049 status fixed too).
- S1: split 6281-line root into `harness/{mod,host,rendezvous,device,membership_topology,ops}.rs` and
  `clipboard_trace, pairing, admission, topology, space_switch, membership_history` modules. Compiles with no
  warnings; rustfmt applied to this entry only; test set identical (62 incl. ignored); line-multiset diff shows
  only signature re-wrapping and new headers.
- S1 full group: 45/51 (F6 epoch race). User approved harness-level fix: branch equivalence also requires
  zero pending effects; F7's separate wait removed.
- S2: `membership-topology` nextest group (2) for `test(/^topology::/)`.
- S1+S2 full group: 47/51 in 281 s; failures are exactly the 4 baseline failures.
- S3: `harness/scenario.rs` (`TestScenario` guard, process-level current scenario, `ensure_before` with
  `#[track_caller]`, `temp_dir`, `record_event`, `stage`); DeviceHarness root is a TempDirLease; topology actions
  record events; branch-equivalence / group-epoch waits are stages; guard inserted in all 62 tests;
  `uc-testkit` dev-dependency (Cargo.lock +1 line); run-test-group sets and prints the artifact root.
- S3 full group: 47/51 in 318 s; 51 artifacts, all cleanup completed; failures = 3 baseline + offline_member
  (product_timeout on group epoch; passes alone in 58 s).
- S4: pr-check `membership-e2e-smoke` job replaces the coverage job's cargo-test automatic_connections step;
  engine-real-environment gains `membership-e2e` job + dispatch mode; manual netns job excludes that mode.
- S5: testing guide, adoption inventory, plan 034 step 6, observability README exact path.
- Clippy: fixed 4 useless `format!` introduced by the conversion; remaining warnings pre-existed.
- Delivery checks pass. Plan 050 record written; index updated.

### Test results
| Test | Result |
|------|--------|
| e2e full run #1 (handoff config) | 44/51 |
| e2e full run #2 (repo config) | 44/51, F7 pass |
| uc-core, uc-infra | pass |
| uc-application --lib space | 324 pass, 1 baseline fail, 1 ignored |
| delivery checks | pass |
| `run-test-group.sh membership-e2e -E 'test(f7_) \| test(removal_convergence)'` | 5/5 pass |
| `run-test-group.sh persistence-provider` | 51/51 pass (membership_record included) |
| check-engine-repository, git diff --check (after step 1) | pass |
| membership-e2e full after S1 | 45/51 (4 baseline, pending_join flaky, F6 race) |
| membership-e2e full after S1+S2+equivalence fix | 47/51 (only 4 baseline) |
| membership-e2e full after S3 | 47/51, 51 artifacts, cleanup completed |
