# Findings: Plan 049 S3 wrap-up

## Membership e2e suite (`crates/uc-engine/tests/space_membership_auto_pairing_e2e.rs`)
- Each "device" is a full Engine: real SQLite, real MLS/signatures, iroh QUIC over loopback UDP. The pairing
  rendezvous is a local wiremock server; no internet, no relay by default. Partitions are applied by dev-tools
  inside the Engine (blocking peer endpoint ids), not by the OS. Real wall clock (no paused time); many
  sleep-polling waits.
- The file starts with `#![cfg(feature = "dev-tools")]`. Consequence: `cargo test -p uc-engine --locked`
  (plan 049 acceptance item 7) compiles it to nothing, so the acceptance list never ran it.
- CI (`.github/workflows/pr-check.yml:137`) runs only `automatic_connections::` from this binary.
- `cargo test` runs all 51 tests in one process (~27 min, distorted timing). nextest runs one process per test
  (~3–5 min with the new group of 4).
- `scripts/testing/run-test-group.sh` is the unified nextest entry (pinned nextest 0.9.145, `--profile ci`); it
  has no group for this binary yet.
- Stale reference: `persistence-provider` group filters `binary(membership_ledger)`; S3 renamed that test binary
  to `crates/uc-infra/tests/membership_record.rs`.
- Testing guide rule: keep old `cargo test` entries (dual-track), do not delete gates.

## Latest full run (repo nextest config, `--no-fail-fast`): 44/51 pass, 7 fail
- Baseline failures (also fail on `cd9537b6`): `same_device_returns_to_a_previous_space_after_switch_and_restart`
  (1216), `suspend_during_space_switch_recovery_does_not_resurrect_the_network` (1103),
  `handoff_four_device_removal_preview_matches_executed_choice`,
  `confirmed_pairing_survives_restart_removal_and_same_device_rejoin`.
- Baseline flaky: `pending_join_is_not_published_before_final_confirmation` — on baseline, 1 of 3 solo runs failed
  with the same 1211 (`device_trust_dependency` at `load_space_device_state`) after final confirmation.
- Load flaky on this branch: `membership_history_retryable_failure_exposes_deadline_and_recovers` (passes alone),
  `offline_member_catches_multiple_removals_without_blocking_new_invitations` (epoch wait timeout; not re-run alone).
- R1–R4, W1/W2, F0–F7 pass.

## Root causes found
- F7 epoch race: Owner commits adopted history, then only wakes the worker; effects (MLS commit carried in the
  AddDevice event's `security_update_payload`) run later. Diagnostics can show equal heads with a stale
  `group_epoch`. Baseline `handle_history_message` also only woke maintenance — same window, not new in S3.
- `retryable_failure`: the dev-tools transport wrapper records `MembershipHistorySyncRetryableFailure` before the
  Owner commits the Deferred result (`crates/uc-engine/src/dev/space_work.rs:432`). The legacy health projection
  maps both Updating and RetryableFailure to `Retrying`
  (`crates/uc-engine/src/operations/device/space_device_update.rs:58`), so the test can see `Retrying` without
  `next_retry_at_ms`. Test-side race.
- Core `present` precedence: RetryableFailure (sync deferred) wins over Updating
  (`crates/uc-core/src/membership/ledger/present.rs:302`).

## Code facts
- Only `owner.rs` constructs `MembershipRecordCommit` in production; Infra admission repo/credentials only `load()`.
- `uc-application` has a pre-existing clippy deny (`async_yields_async`) at `application/shutdown.rs:70`.

## Migration survey: membership e2e onto the nextest/testkit architecture (2026-09-24)
- Architecture split (`docs/design-docs/testing-architecture.md`, plan 048): nextest owns process discovery,
  filtering, groups, concurrency, timeouts, retries, JUnit; `uc-testkit::Scenario` (at `tests/uc-testkit/`) owns
  per-scenario identity, budget, stage timing, event waits, temp-dir/port leases, failure classification,
  `result.json`/`summary.txt` artifacts. Scenario budget must be shorter than the nextest timeout.
- Existing adopters of uc-testkit: `uc-application/tests/file_transfer.rs`,
  `uc-infra/tests/peer_admission_identity_resolution.rs`, `uc-infra/tests/profile_storage_upgrade_crash.rs`.
- The e2e file is 6281 lines + 3 submodules (automatic_connections 620, removal_convergence 253,
  six_digit_pairing 108); 51 tests run under nextest (62 listed incl. submodule tests / skipped).
  Ad-hoc timeouts: WAIT_TIMEOUT 60s, ADMISSION_WAIT_TIMEOUT 120s, SPACE_DEVICE_UPDATE_WAIT_TIMEOUT 30s,
  SHUTDOWN_TIMEOUT 15s; waits are sleep/yield polling loops; no artifacts on failure beyond stdout.
- Plan 034 step 6 already intends: F0–F7 as explicit slow lane run by a script, scheduled/workflow_dispatch job;
  protocol matrix later moves to the Application virtual suite. `test-adoption-inventory.md:72` says keep a
  minimal real-link matrix and only push down deterministic rules.
- CI: `pr-check.yml` installs nextest 0.9.145 and runs `run-test-group.sh evidence`; the membership binary is only
  run via `cargo test ... -- automatic_connections::` (line 137). `engine-real-environment.yml` has a nightly cron
  and workflow_dispatch for the Linux netns real runner, not for this binary.
- `AGENTS.md` has an uncommitted user edit (planning-with-files location rule) — do not commit it.

## S1/S2 results and S3 design (2026-09-24)
- S1 full group: 45/51; failures = 4 baseline + pending_join (baseline flaky) + F6 (epoch race like F7,
  passes alone in 132 s). User approved fixing at harness level: `wait_for_equivalent_branch_named` now also
  requires `pending_effect_count == 0`; F7's separate settled wait removed.
- S2: `.config/nextest.toml` group `membership-topology` (max-threads 2) for `test(/^topology::/)`, listed
  before the binary-wide override (first match wins); verified with `nextest show-config test-groups`.
- testkit facts: `Scenario::finish(self, result)` is sync and consumes; artifacts are only written by finish,
  so a panic before finish leaves none. Names: lowercase/digits/single hyphens, <= 64 chars. `TempDirLease`
  records cleanup on drop; a lease alive at finish marks cleanup Pending → CleanupFailed. Budget is only used
  by `wait_for_event` / child processes. Existing adopters use
  `CARGO_MANIFEST_DIR/../../target/test-artifacts/<name>` or `UC_TEST_ARTIFACTS_DIR`.
- Tests do not share a common first line (mount_rendezvous / init_test_tracing are not universal), so the
  scenario is started by an explicit guard line per test: `let _scenario = TestScenario::start();`.
- S3 design: `harness/scenario.rs` keeps the current Scenario in a process-level Mutex (nextest runs one test
  per process; cargo test defaults to one test thread). Name from the test thread name, sanitised; seed = FNV
  hash of the full name. Drop of the guard finishes: panicking → recorded failure or generic
  product_invariant; not panicking but finish fails (e.g. cleanup) → panic. Harness timeouts record
  product_timeout with a condition before panicking; topology actions record events; main waits are stages;
  DeviceHarness root becomes a `TempDirLease`. Risk: harnesses held by detached spawned tasks at test end.

## offline_member root cause (2026-09-24)
- `cargo test -p uc-engine --locked`: all pass (lib 242 + 2 ignored, host_contract 23 + 1 ignored incl. others' WIP,
  public_contract 50, others). The membership e2e binary compiles to 0 tests without dev-tools.
- offline_member: B offline while A removes C and D; A's group update deliveries to B fail and back off
  (`space_security_store/delivery.rs`: 30 s doubling, max 1 h). After B returns and history converges, B only
  gets the update when A's backoff expires (solo run: failures at 05:29:14/15, first success 05:29:48). Under load
  a second failure pushes it to 60 s+, beyond the test's 60 s epoch wait. Not changed by S3.
- Infra already supports bypass: `due_space_group_updates(now, online_peer)` ignores backoff for `online_peer`
  (non-rejected). The Application use case passes `None` since `3bb57184` (2026-09-20 single space work owner),
  which removed `MembershipMaintenanceTrigger::PeerOnline/PeerContact` deliberately
  (`docs/exec-plans/active/2026-09-20-single-space-work-owner.md:217`: external online/contact are business-neutral
  wakeups; no step selection or backoff bypass). That also dropped the 2026-09-17 targeted history-sync bypass.
- The offline_member test lacked `init_test_tracing()`; added (logs are needed for diagnosis).

## S3.a diagnosis
- joiner_pairing_fixture_reaches_active_settled: TEST problem. `complete_pending_space_transition()` returns
  `Processing` by design since confirmation-gated admission; public `Active` is an Infra projection
  (`uc-infra/src/space/admission/display.rs`) from `is_active_settled()`. The fixture kept the pre-confirmation
  status and asserted Active. Fixed: snapshot keeps activation (Processing) + `active_settled` after final
  confirmation; assertions unchanged. 4/4 admission_recovery_scenarios pass.
- same_device_returns_to_a_previous_space_after_switch_and_restart: PRODUCT problem outside 049. Joiner restart
  right after a cross-space join fails in Engine::start with 1216 (ProfileKeyRecoveryError::Corrupt) before any
  membership log; fresh-join restart passes. Source is profile key recovery startup
  (`uc-infra/src/security/profile_key_recovery.rs::prepare_startup` path) — the file and
  `uc-engine/src/runtime/profile_recovery.rs` are someone else's WIP; already listed as pre-existing in the inbound
  peer admission plan. Hypothesis (unproven): vault root not rewrapped after cross-space switch.
- suspend_during_space_switch_recovery_does_not_resurrect_the_network: PRODUCT problem outside 049 (Engine runtime
  lifecycle). On resume, "inspect pending space transition" reads joiner activation / pending admission state while
  the space is still locked (`joiner activation state is locked`), maps to 1103 Internal non-retryable, resume fails.
  Ordering issue in resume (inspect before unlock). Pre-existing on baseline.
- handoff_four_device_removal_preview_matches_executed_choice: TEST problem. Failed at the assertion that A (which
  accepted B's removal of C) lists C as `Removed`. Under ADR-027/R1 only locally initiated removals keep a departing
  entry (spec 021 AwaitingRemovalAcknowledgement is for the remover); the accepting side drops C. Same class as the
  user-approved F1 change. Assertion changed to "target no longer listed" (stricter). Passes 2/2 alone.
- confirmed_pairing_survives_restart_removal_and_same_device_rejoin: PRODUCT problem (membership presentation;
  pre-existing, mirrored by S1). After same-device rejoin the sponsor's pairing confirmation stays None for 120 s.
  `MembershipLedger::device_views` (`uc-core/.../ledger/present.rs:161`) maps device → member with
  `history.member_for_device`, which returns the FIRST credential for that device — for a rejoined device that can be
  the old removed instance, so the device presents as Removed and the confirmation lookup (keyed by the active
  instance) finds nothing. Old query code (cd9537b6 query_device_trust/use_case.rs:198) used the same function.
  Proposed fix (needs user OK): prefer `effective_member_for_device`, fall back to `member_for_device`.
- pending_join_is_not_published_before_final_confirmation: PRODUCT side, not yet pinned. Failing run: right after the
  sponsor's final-confirmation activation (group_epoch 3, pending_group_update_count 1) the group update dispatch
  fails with error.type=storage and the device trust query fails with Dependency (retryable 1211); the joiner's session
  transition also failed once on "query transitioned device trust revision". The sponsor-side group update store read
  fails after activation; the query only inherits it. Needs a formal diagnostic log with the error source chain
  (observability rules) to tell a transient window from a store inconsistency. Pre-existing (baseline 1/3).
- confirmed_pairing FIXED: `member_for_device` now prefers the effective instance (`max_by_key(effective)`); call-site
  fallbacks removed (remove_space_member single lookup + filter; decision check uses `device_for_member`). Core
  regression test red before fix. e2e passes in 12.5 s.
- pending_join DIAGNOSED with new fixed-label diagnostics: dependency=security_update_status cause=key_epoch_repository;
  infra source=storage reason=unknown during sponsor activation writes. busy_timeout 5000 is set; the status read path
  reads then writes the encrypted index → likely deferred-transaction lock-upgrade BUSY (not provable without raw text).
- Final: nextest units 2559/2559; membership-e2e 49/51 (two non-049 product failures).

## 2026-09-24 session 4 — S3.a fixes (items 1, 2a, 2b)
- Item 1 path confirmed: `delivery.rs::load_due_updates_on` and `save_delivery_failures_on` use `conn.transaction`
  (DEFERRED) and read then write (source summary / reconcile / delete stale). Production pool is r2d2 default size
  (10 connections) in WAL mode. In WAL a deferred txn that read a snapshot and then upgrades to write after another
  connection committed gets SQLITE_BUSY_SNAPSHOT immediately; busy_timeout does not apply. All other store writes
  already use `immediate_transaction`. Fix: begin both as IMMEDIATE.
- Item 2b path confirmed: `session_supervisor/lifecycle.rs::resume` → `install_new_session(false)` →
  `has_pending_space_transition()` before `recover_space_session()`. `Locked` there comes from
  `AdmissionKeyManager::profile_key` → secure storage `get` failing (`AdmissionKeyError::SecureStorage`), not the
  space passphrase. Need the log to see why secure storage fails right after resume (harness uses in-memory storage).
- Item 2a: the other WIP in `profile_recovery.rs` adds factory reset from recovery mode; unrelated to the 1216 but
  same files. Must ask the user before editing.
- Items 2a and 2b share one root cause (evidence: 2b log shows `master key cleared` on suspend, then on resume
  `relationship operation failed (locked)`, `pending admission recovery state is locked`,
  `joiner activation state is locked` → 1103). `Locked` = `AdmissionKeyError::SecureStorage` =
  `ProfileKeyRecoveryStore::get` failing in `activate_from_backing_if_available` (`activate_existing` →
  `unwrap_file_root` fails → `Corrupt`). Engine `suspend` calls `recovery.suspend()` (drops the active vault), so the
  resume re-activates from the backing KEK. `RuntimeSpaceAccessAdapter::install_group_join`
  (`crates/uc-infra/src/space/security/access.rs` ~1010) creates a fresh keyslot (new salt), derives a NEW profile
  KEK from the passphrase and overwrites it with `store_kek`, but never rewraps the recovery vault root, unlike
  `EncryptionPassphraseChange::complete` which calls `prepare_passphrase_change` before installing and
  `finish_passphrase_change` after. 2a: restart → `prepare_startup` → `activate_or_migrate` → same unwrap failure →
  1216. Fix candidate that avoids the WIP files: give `install_group_join` the `ProfilePassphraseRecoveryPort`
  prepare/finish pair (wiring in uc-engine `assembly/wire`). Needs user decision (handoff conflict note).
- Item 1 CONFIRMED + FIXED: new Infra test `delivery_status_read_survives_concurrent_space_material_writes`
  failed before (key epoch repository failure) and passes 3/3 after switching both delivery index transactions to
  `immediate_transaction`; store tests 29/29; pending_join e2e 5/5 (was ~1 in 2–3 failing).
- Items 2a/2b refined: the KEK is replaced by `activate_prepared_control_generation` (access.rs ~696), called from
  `SpaceTransitionActivation::rebind_target` when the cross-space join activates (after final confirmation, in the
  background runtime transition). The Engine's existing `refresh_after_authentication` rewrap runs only right after
  the `JoinSpace` operation returns (`Processing`), i.e. BEFORE the KEK changes, so no rewrap happens afterwards.
  The in-memory vault stays usable until `recovery.suspend()` (2b) or process restart (2a).
  Correct fix: the KEK-replacing step does the crash-safe pending/finish rewrap (same protocol as passphrase change).
  Existing `prepare_passphrase_change` requires an active vault and errors when none; activation must also handle
  "no vault file yet" (fresh device) and "file exists but inactive" → needs new methods in profile_key_recovery.rs
  (WIP file). `install_group_join` has only test callers; `derive_master_key_for_proof` (legacy offer) also
  overwrites the KEK — out of scope, note only.
- 2a next failure ROOT CAUSE (proved): with the new classification the joiner logs
  `issue="membership_history"` (not relationships; earlier "duplicated relationship" guess was wrong) and rejects
  MembershipHistoryInvalid. Core repro test `a_device_that_rejoins_without_removal_starts_with_its_new_instance`
  (ledger/tests.rs) fails with `InvalidSnapshot`: history `effective_members()` (snapshot.members) keeps BOTH
  instances of a device that rejoined without being removed (validation.rs only makes `active_members` device-unique);
  `MembershipLedger::effective_peer_devices` = effective members minus the local INSTANCE, so the other instance of
  the local device becomes a peer equal to the local device → `validate()` InvalidSnapshot. Independent of which
  instance is local. Candidate fix (needs user approval, Core membership behaviour): define peers as effective
  members whose DEVICE differs from the local device (used by start/normalize/validate), or make `members`
  device-unique like `active_members`.
