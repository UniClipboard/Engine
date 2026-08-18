# Spec 025 Acceptance Evidence Audit

## Evidence Standard

- **Direct:** a focused test exercises the named behavior and asserts the required outcome.
- **Indirect:** implementation or a broader test is consistent with the requirement but does not assert it directly.
- **Missing:** no current test was found that proves the requirement.
- Physical-device evidence remains separate from automated evidence.

## Initial Gaps To Resolve

| Requirement area | Current evidence | Initial status |
| --- | --- | --- |
| Fresh, Same-Space, Cross-Space before and after the Prepared boundary | Pre-Prepared has no persisted transition kind; all three persisted transition variants now have a direct conflict matrix | Resolved |
| Automatic recovery after restart or lost delivery | Initiated identity reload plus deferred/confirmed outbox recovery and existing Prepared-through-completion recovery tests | Resolved by combined direct evidence |
| All core supersession rejections | Full role, stage, recovery, material, active-request, and contradictory-state matrix | Resolved |
| Late Rejected, delivery acknowledgement, recovery scan, and terminal compaction | Focused old-attempt cleanup, recovery, and compacted-message tests | Resolved; Applied is outbound from the local Joiner and cannot arrive at it |
| Same-invitation sponsor consumption remains bound to the original attempt | Local new-identity assertions plus durable sponsor digest-binding test | Resolved |
| Database, WAL, SHM sensitive-marker scan for the new terminal and cleanup data | Dedicated nine-marker scan requires and inspects all three files | Resolved |
| Android two-device flow | Only one Android device was available | Skipped, blocking full spec completion |

## Confirmed Existing Direct Evidence

| Behavior | Test evidence | Status |
| --- | --- | --- |
| Initiated supersession preserves replay facts and creates isolated cleanup | `initiated_join_can_be_superseded_without_losing_replay_facts` | Direct |
| Prepared and contradictory Candidate rejection does not mutate the input | `prepared_or_recovery_bound_join_cannot_be_superseded` | Direct but incomplete state matrix |
| Atomic old/new save, one revision increment, new current projection, and superseded compaction | `commit_local_join_start_supersedes_and_creates_atomically` | Direct |
| Stale-version rollback and successful retry | `supersession_failure_recovers_whole_old_or_new_state` | Direct |
| Failure while resealing old data, creating the new key, or sealing new data rolls back | `supersession_crypto_failures_roll_back_atomically` | Direct |
| Existing admission payload, invitation, key, target access, recipient, and message markers are absent from SQLite-family files | `admission_identity_security_state_and_messages_never_reach_sqlite_in_plaintext` | Direct for the shared encrypted storage path |
| Candidate, Commit, Complete late-message handling through direct transaction and public protocol entries | six `superseded_late_` tests | Direct |
| Rejection cleanup and multiple old cleanup recovery isolation | `superseded_rejection_only_confirms_old_cleanup`, `recovery_handles_multiple_superseded_cleanups_with_one_current_join` | Direct |
| Superseded cleanup delivery acknowledgment only settles the old attempt | `superseded_delivery_acknowledgment_only_settles_old_cleanup` | Direct |
| Fresh, Same-Space, and Cross-Space stored transitions are all past the replacement boundary | `explicit_join_after_prepared_rejects_every_space_transition_mode` | Direct |
| Invitation consumption remains bound to the original attempt after compaction | `consumed_invitation_stays_bound_to_its_original_attempt` | Direct |
| Superseded terminal, cleanup, old/new request, and security markers are absent from SQLite, WAL, and SHM | `superseded_terminal_and_cleanup_never_reach_sqlite_files_in_plaintext` | Direct |

## Source Audit Notes

- `AdmissionAttemptV1::superseded_by_new_join` rejects non-Joiner roles, all terminal records, stages other than Initiated/Candidate, Prepared proof, write-ahead recovery, Space transition state/result, cleanup-pending state, and missing identity/security material.
- `DieselAdmissionAttemptStore::commit_local_join_start` reopens and checks the expected previous record inside one immediate transaction, uses checked increments, reseals the old record, creates and seals the replacement, updates metadata, and commits them together.
- The existing plaintext scan enumerates all files in the temporary database directory. The dedicated superseded-state scan separately exercises the new terminal and cleanup payload and requires the SQLite, WAL, and SHM files to be present.
- Phase 9 added a dedicated superseded-state scan and requires the SQLite main file, WAL, and SHM to all be present before scanning each for nine representative markers.
- Red tests demonstrated that the core rule accepted incomplete Candidate data and classified a missing JoinRequest only as an invalid cleanup. It now requires a valid saved initial request and complete Candidate material, while still allowing a delivered request that has not crossed Prepared.
- Fresh/Same-Space/Cross-Space are not valid distinctions before Prepared because the target transition is persisted while preparing. The direct matrix therefore proves that all three persisted transition variants reject replacement; pre-Prepared replacement remains a common atomic behavior with no Space transition to mutate.

## Acceptance Criteria Matrix

| # | Requirement | Direct evidence | Final status |
| ---: | --- | --- | --- |
| 1 | Public JoinSpace is always a new action; recovery keeps the original attempt | `explicit_join_supersedes_initiated_attempt_atomically`, `automatic_recovery_keeps_the_same_join_identity`, removed `reopen_join_start` source scan | Proved |
| 2 | Initiated/Candidate replacement is one atomic old-terminal/new-attempt result | `commit_local_join_start_supersedes_and_creates_atomically`, Initiated, delivered-Initiated, and Candidate application tests | Proved |
| 3 | Same and different invitations create all-new local identities and security material | `explicit_join_with_same_invitation_starts_new_attempt`, `explicit_join_supersedes_initiated_attempt_atomically` | Proved |
| 4 | Prepared and later return 1295 without local or network side effects | generic and three-transition Prepared tests, `unsupersedable_join_is_rejected_before_dial`, Engine/binding contract tests | Proved |
| 5 | Contradictory stage, proof, recovery, transition, and candidate identity data fail closed | four core fail-closed tests plus store validation | Proved |
| 6 | Supersession is not rejection and does not change membership history | core terminal assertions and Candidate test's unchanged source-history/transition assertions | Proved |
| 7 | Old pre-commit traffic only cleans up; Commit/Complete stop safely | ten `superseded_` tests, including protocol entries and compacted terminal | Proved |
| 8 | Multiple old cleanups coexist with one highest-ordinal current join | `recovery_handles_multiple_superseded_cleanups_with_one_current_join` | Proved |
| 9 | Invitation consumption cannot be rebound | `consumed_invitation_stays_bound_to_its_original_attempt` | Proved |
| 10 | Existing Space, identity, history, and local data remain untouched before activation | unchanged membership-history/transition assertions plus the admission store's isolated ownership; existing Fresh/Same/Cross activation tests | Proved within automated boundary |
| 11 | Every atomic failure and counter overflow leaves whole old or whole new state | version, crypto, material, delivery, and three-counter overflow tests | Proved |
| 12 | New terminal and cleanup data remain encrypted in SQLite/WAL/SHM and logs stay fixed-field | dedicated nine-marker scan, existing general encrypted-admission scan, fixed failure-reason mapping | Proved |
| 13 | WorkspaceConvergence remains the only full owner | source boundary review and architecture repository check | Proved |
| 14 | iOS, Android, HarmonyOS keep 1295/Conflict/false and unchanged success status | Engine, UniFFI, and HarmonyOS focused contract tests | Proved |
| 15 | Focused tests are nonzero and repository checks pass | exact `--list` counts, successful full workspace rerun, and final verification record | Proved |
| 16 | Android two-device physical acceptance | only one Android device available | Skipped, not proved |
| 17 | Old public reopen path and temporary branches are removed; docs synchronized | `rg reopen_join_start` only finds historical/spec text; Spec 023, Spec 025, architecture record updated | Proved |

## Edge Case Matrix

| Scenario | Evidence | Status |
| --- | --- | --- |
| No previous join | `commit_local_join_start_supersedes_and_creates_atomically` Create half | Proved |
| Same invitation | same-invitation identity test plus immutable sponsor binding test | Proved |
| Different invitation | Initiated atomic replacement test | Proved |
| Initial request delivery already acknowledged | `explicit_join_supersedes_initiated_attempt_after_request_delivery_ack` | Proved |
| Prepared saved but not acknowledged | Prepared tests do not acknowledge delivery and still return conflict | Proved |
| Candidate with proof, write-ahead, transition, or missing identity binding/material | core fail-closed matrix | Proved |
| Concurrent JoinSpace | `concurrent_explicit_joins_leave_one_current_attempt` | Proved |
| New material generation failure | `explicit_join_material_failure_keeps_previous_attempt` | Proved |
| Atomic save or crash boundary failure | version and crypto rollback tests | Proved |
| Commit succeeds but request send fails | `failed_new_request_delivery_keeps_replacement_current_for_recovery` | Proved |
| Late Candidate or rejection | candidate protocol tests and rejection cleanup test | Proved |
| Late Commit or later message | Commit/Complete direct, protocol, and compacted-terminal tests | Proved |
| Multiple historical cleanup outboxes | multiple-cleanup recovery test | Proved |
| Inbound Sponsor or other profile work | `inbound_admission_blocks_explicit_join_without_retry_loop` and repository competing-work check | Proved for shared profile exclusion |
| Old data and unknown new terminal | append-only wire-value test includes a previous-version decoder rejecting the new variant | Proved |
| Ordinal, revision, or record-version overflow | `supersession_counter_overflows_roll_back_atomically` | Proved |

## Final Automated Rerun

- `cargo test --workspace --all-targets --locked` completed with exit code 0 after the audit corrections.
- Package totals recorded in Spec 025 are 850 application, 268 core, 812 infrastructure, 196 Engine, 44 UniFFI, and 33 HarmonyOS tests passed.
- The four existing manual infrastructure performance tests remained ignored and are not counted as passed.
- Locked metadata, all-target workspace compilation, formatting, architecture validation, and whitespace validation all passed after the final corrections.
- The Android two-device item remains skipped and is not converted into automated evidence.
