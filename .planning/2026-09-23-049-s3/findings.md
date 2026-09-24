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
