# Progress Log

## Session: 2026-08-18

### Phase 9: Requirement-by-requirement completion audit

- **Status:** complete; full specification acceptance remains pending physical-device evidence
- Re-opened the active goal because the specification still has an unchecked Android two-device acceptance criterion.
- Confirmed the committed worktree is clean at `f4731a9` before the audit.
- Began mapping every numbered edge case and acceptance criterion to direct code and test evidence.
- Initial risk: the Spec claims Fresh, Same-Space, and Cross-Space supersession coverage, while the newly named tests do not yet prove all six safe/unsafe combinations directly.
- Created `acceptance_audit.md` and recorded the first direct-evidence gaps: the three source-space modes, stage-by-stage recovery, sponsor invitation binding, the full late-message matrix, and database/WAL/SHM plaintext scanning.
- Audited the core supersession rule and the atomic store implementation. The implementation contains the expected fail-closed checks and one-transaction write path, but its direct unit-test matrix is incomplete.
- Confirmed the existing encrypted-storage test scans every database-directory file; it needs the new superseded cleanup payload added before it directly proves Spec 025 at-rest coverage.
- Added two failing core tests. They showed that a join without an active JoinRequest returned `InvalidCleanupMessage`, and an incomplete Candidate was incorrectly accepted for supersession.
- Tightened the core supersession rule to require a valid active JoinRequest and complete Candidate material. The focused core filter now passes 7 tests.
- Added direct tests for every unsafe role/stage/recovery class, append-only serialized enum values, superseded cleanup acknowledgments, all three persisted Space-transition variants, immutable invitation consumption, and sensitive markers across SQLite/WAL/SHM.
- Added the new conflict and previously omitted variants to the analytics failure-mapping test.
- Updated the architecture-bible maintenance record for the fail-closed correction and expanded verification.
- The post-audit full workspace run reached `uc-infra` with one failure: the negative pull test observed the expected peer rejection at stream opening instead of response reading.
- The exact failed test passed alone. Source review confirmed the test helper had an overly narrow transport timing assumption, so it now accepts rejection at any request stage while retaining strict decoded-response checks for success cases.
- Re-ran `cargo test --workspace --all-targets --locked` from the beginning after the test correction; it exited successfully with no failures.
- Final package totals are 849 application, 268 core, 812 infrastructure, 196 Engine, 44 UniFFI, and 33 HarmonyOS tests passed. Four existing manual infrastructure performance tests remained ignored.
- Strict review found one important gap: an Initiated attempt whose JoinRequest delivery was already acknowledged returned `RecoveryRequired` even though it had not crossed Prepared.
- Added `explicit_join_supersedes_initiated_attempt_after_request_delivery_ack` at the application owner boundary. The first corrected test command matched zero because `--exact` was used with a short name; the rerun matched one and failed with the expected `RecoveryRequired`.
- Changed the rule to retain a valid initial request as a replay fact after delivery settlement and to use its message id for cleanup when no active outbox remains. The new test then passed 1/1 and the core admission filter passed 7/7.
- The additional application test raises the final package total expected on the next full rerun from 849 to 850.
- Second strict review found the Candidate completeness check omitted `identity_binding`. Reworked the focused core test so every other Candidate field is present; it failed by returning a superseded result, then passed after the binding became required.
- The same review narrowed the Iroh rejection helper so a failed dial cannot satisfy the negative assertion. The two rejection tests still passed 2/2 after the correction.
- Post-fix focused results: core admission 7/7, explicit joins 8/8, acknowledged-request replacement 1/1, and Iroh rejection 2/2.
- The final read-only strict review retained no blocking, important, or optional findings after both corrections. The remaining verification gap is the Android two-device physical flow.
- The final corrected `cargo test --workspace --all-targets --locked` run exited successfully. Verified package totals are 850 application, 268 core, 812 infrastructure, 196 Engine, 44 UniFFI, and 33 HarmonyOS tests passed; four manual infrastructure performance tests remained ignored, and zero-test targets were not counted.
- The Android two-device physical flow remains skipped because only one Android device is available. Automated success does not complete that acceptance item.
- Final delivery checks passed: locked workspace metadata, all-target workspace compilation, formatting, repository architecture validation, and whitespace validation.
- Created the scoped local follow-up commit without pushing. The remaining work is the skipped Android two-device physical acceptance, not an automated or repository defect.

### Phase 1: Requirements and source discovery

- **Status:** complete
- Actions taken:
  - Read the complete `to-spec` and `planning-with-files` skill instructions.
  - Ran session catchup; no unsynchronized report was returned.
  - Confirmed current branch and preserved existing documentation and planning changes.
  - Read ADR-022 and the previous ADR research notes.
  - Located the current reopen-on-repeat behavior in `DurableAdmissionTransaction::prepare_join_before_network`.
  - Mapped the core attempt model, repository port, production Diesel store, projection, and process-level serialization lock.
  - Traced the conflict through convergence, application facade, Engine error mapping, and binding transport.
  - Located the durable Prepared write boundary and all old-attempt message/recovery entry points that need fail-closed handling.
  - Reconciled the design with Spec 023 and selected the smallest data, wire-cleanup, transaction, and projection changes.
- Files created/modified:
  - `.planning/spec-025-user-initiated-join-supersession/task_plan.md`
  - `.planning/spec-025-user-initiated-join-supersession/findings.md`
  - `.planning/spec-025-user-initiated-join-supersession/progress.md`

### Phase 2: Design and phase boundaries

- **Status:** complete
- Actions taken:
  - Defined append-only core stage and terminal additions.
  - Defined one atomic repository operation for old-attempt finalization and replacement creation.
  - Fixed the public error contract at code 1295, Conflict, non-retryable.
  - Chose vertical implementation stages and explicit exit gates.
  - Defined delayed-message, cleanup, restart, invitation, and compatibility behavior.

### Phase 3: Specification drafting

- **Status:** complete
- Actions taken:
  - Added `docs/specs/025-user-initiated-join-supersession.md` with all 11 required sections.
  - Defined six vertical implementation phases with risks and exit gates.
  - Added the Spec to the document index, ADR-022, Spec 023, and the architecture-bible related-document list.
  - Added an architecture-bible maintenance record that explicitly keeps production and device work pending.
  - Corrected the focused repository test filter and Phase 1 exit gate during review.

### Phase 4: Verification

- **Status:** complete
- Actions taken:
  - Confirmed all 11 required sections are present exactly once.
  - Confirmed every referenced workspace path and every `docs/README.md` link exists.
  - Scanned ADR-022, Spec 023, Spec 025, and the architecture bible for the superseded repeated-JoinSpace rule; no contradictory current rule was found.
  - Confirmed the new file has no whitespace errors and the working tree contains no production source changes for this task.

## Test Results

| Test | Expected | Actual | Status |
|------|----------|--------|--------|
| Session catchup | Report stale context if present | No stale context reported | Pass |
| Required section count | Exactly 11 numbered sections | 11 sections | Pass |
| Referenced paths | Every referenced path exists | No missing paths | Pass |
| Contradiction scan | No current rule says a public repeated JoinSpace reopens the old attempt | No matches | Pass |
| `cargo metadata --locked --format-version 1` | Workspace metadata resolves | Completed successfully | Pass |
| `cargo check --workspace --all-targets --locked` | Entire workspace compiles | Completed successfully | Pass |
| `cargo fmt --all -- --check` | Formatting is clean | Completed successfully | Pass |
| `node scripts/architecture/check-engine-repository.mjs` | Repository rules pass | 6 OpenMLS checks and negative fixtures passed | Pass |
| `git diff --check` | No whitespace errors | Completed successfully | Pass |
| Physical device acceptance | Android two-device flow | Not run; documentation-only task | Skipped |

## Error Log

| Timestamp | Error | Attempt | Resolution |
|-----------|-------|---------|------------|
| 2026-08-18 | No memory registry hit for ADR-022 terms | 1 | Continued from current workspace evidence. |
| 2026-08-18 | Plan update patch referenced a row in the wrong file | 1 | Re-read both files and applied a corrected patch. |
| 2026-08-18 | Full UniFFI public-contract run timed out while shutting down after the capture-only clipboard test | 1 | The exact test passed in 14.16 seconds; reran the complete serial suite to distinguish a boundary fluctuation from a remaining leak. |
| 2026-08-18 | The second full UniFFI public-contract run timed out in two different shutdowns at 15 seconds | 2 | Raised the shared test-only deadline to 30 seconds because 15 seconds was not stable under serial suite load. |
| 2026-08-18 | Passed two positional test filters to one Cargo invocation | 1 | Re-ran with the shared `_cannot_be_superseded` filter and confirmed both expected failures. |
| 2026-08-18 | Initial patch anchor for the cleanup-ack test did not match current source context | 1 | Re-read the local block and applied the same test at the exact boundary. |
| 2026-08-18 | Full workspace run failed `known_but_unadmitted_peer_is_dropped_before_serve` because the remote rejection arrived at `open_bi` | 1 | The focused rerun passed; changed the shared test helper to return transport errors from every request stage instead of panicking before the negative assertion. |
| 2026-08-18 | The first red-test command matched zero tests because `--exact` received a short name | 1 | Removed `--exact`; the rerun matched one test and failed at the intended `RecoveryRequired` assertion. |

## Implementation Follow-up

- Implemented all six Spec 025 phases and added public-entry coverage for late Candidate, Commit, and Complete messages.
- Fixed host clipboard work so session shutdown can cancel an in-flight change operation.
- Unified UniFFI public-contract shutdown deadlines at 30 seconds after two serial suite runs showed that 15 seconds was not stable.
- The two failures from the previous full workspace run pass with the final shared deadline.

## Final Verification

| Test | Actual | Status |
|------|--------|--------|
| UniFFI `public_contract` at the final shared deadline | 22 passed, 0 failed | Pass |
| `cargo test --workspace --all-targets --locked` | Completed successfully; focused admission, host shutdown, and all binding tests passed | Pass |
| `cargo metadata --locked --format-version 1` | Workspace metadata resolved | Pass |
| `cargo check --workspace --all-targets --locked` | Entire workspace compiled | Pass |
| `cargo fmt --all -- --check` | Formatting is clean | Pass |
| `node scripts/architecture/check-engine-repository.mjs` | Repository preflight and negative fixtures passed | Pass |
| `git diff --check` | No whitespace errors | Pass |
| Added production-code forbidden-pattern scan | No newly added forbidden calls | Pass |
| Final strict diff review | No blocking, important, or optional findings | Pass |
| Android two-device physical acceptance | Not run; only one Android device was available | Skipped |
| iOS physical acceptance | Not run; no online iPhone was available | Skipped |
| HarmonyOS and additional network matrix | Not run | Skipped |

## 5-Question Reboot Check

| Question | Answer |
|----------|--------|
| Where am I? | Phase 5: delivery review |
| Where am I going? | Design, draft, verify, and deliver Spec 025 |
| What's the goal? | An implementation-ready phased Spec for ADR-022 |
| What have I learned? | See `findings.md` |
| What have I done? | Drafted and verified Spec 025 and all related documentation updates |

### Phase 5: Delivery

- **Status:** complete
- Actions taken:
  - Reviewed the final specification, acceptance checklist, references, and working-tree scope.
  - Confirmed the task changes documentation only and does not claim production or physical-device completion.
  - Prepared the concise user-facing delivery summary.
