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

### Next
- Phase C: waiting on user for the retryable_failure test change and commit confirmation.

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
