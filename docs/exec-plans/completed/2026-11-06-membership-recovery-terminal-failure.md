# Membership recovery terminal failure

## Status

Complete for the Engine scope on 2026-11-06. GUI integration, native devices,
remote CI, release publication, and deployment to the incident device were not run.

## Owner and scope

The membership conflict recovery use case owns recovery, terminal failure,
persistence, and restart behavior. The caller selects one device group through
the existing choice operation. Successful recovery retains its current result.
Before a branch transition exists, a stable recovery failure must persist the
existing `RePairingRequired` status. A committed transition must continue through
its existing recovery owner. The owner must never abandon it or report false completion.

The Engine choice query must expose the outcome of a persisted branch selection.
A host must distinguish `pending` from `re_pairing_required`. It must not infer
completion from a missing error or treat a failed selection as successful.
The GUI must consume the query outcome before this change can remove its pending
dialog. GUI integration and native acceptance are outside this Engine change.

## Format and resources

Reuse the existing encrypted conflict record and `RePairingRequired` status.
Do not add a persistent field, format version, dependency, task, or queue.
Keep staged recovery material and signed membership history unchanged on failure.
Add only an optional public query field for the selected outcome.

## Failure modes, recorded before implementation

1. A rejected or invalid recovery channel can leave `Selected` indefinitely.
2. A temporary unavailable channel must remain retryable, not require re-pairing.
3. Failure to persist the terminal result must remain deferred and retryable.
4. A database generation change must not write a terminal result into new state.
5. The owner must retain a persisted transition after a later stable failure.
6. Restart must retain the terminal result and avoid another network attempt.
7. Repeated selection must report re-pairing instead of pending.
8. The query must preserve the unresolved issue and report its terminal outcome.
   It must not silently complete or hide the issue.
9. A successful recovery must retain its existing completion behavior.

## Validation

Write the failure-injection workflow scenario before changing production code.
Exercise recovery, record commit, query, owner restart, maintenance retry, and
repeated selection. Use the existing membership Scenario artifact mechanism.
The scenario isolates the channel because deterministic admission rejection and
storage failure injection require control at that capability boundary. This scenario
does not certify real-device networking or GUI behavior.

Run the targeted membership recovery suite and Engine contract checks, then the
repository delivery checks. Record skipped native and GUI acceptance separately.

## Execution

- The new rejection scenario first failed with
  `stable-failure-left-selection-pending` before production code changed.
- All 23 recovery tests passed after the change. The three new workflow scenarios
  produced repeatable `result.json` and `summary.txt` artifacts. These scenarios
  isolate the channel and rebuild the owner from its saved in-memory record;
  they do not simulate a native process restart or disk failure.
- All five Engine choice contract tests passed, including the new optional query
  field and older payload decoding.
- The existing real QUIC scenarios
  `r3_rejected_removal_settles_both_sides` and
  `f2_concurrent_leaf_removals_resolve_to_selected_branch` passed through the
  `membership-e2e` group with one test process at a time. The latter includes
  five complete Engines, branch recovery, and restart. Both produced scenario
  artifacts and a nextest JUnit report. The other 68 group scenarios were skipped.
- Locked metadata, workspace all-target check, formatting, Rust style,
  repository architecture, relative documentation links, and diff checks passed.
- No new dependency, persistent format, task, queue, or member authorization was
  added. The agent did not reset the running application.
- The initial admission rejection remains unclassified beyond the recorded
  channel failure. This change repairs terminal-state propagation, not the
  unproven origin of that rejection.
